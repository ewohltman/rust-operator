use futures::StreamExt;
use gateway_api::httproutes::HttpRoute;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Service};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use k8s_openapi::serde::Serialize;
use kube::api::{ListParams, Patch};
use kube::core::params;
use kube::runtime::controller::{Action, Error};
use kube::runtime::events::{Event, EventType, Recorder};
use kube::runtime::{Controller, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use rust_operator::{
    CONDITION_REASON_HTML_TOO_LARGE, Context, MANAGER, PermanentError, ReconcileError,
    ReconcileResult, TransientError, Webpage, WebpageStatus, new_status, ready_condition,
};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

fn namespace_name_ref(wp: &Webpage) -> ReconcileResult<(String, String, OwnerReference)> {
    let controller_ref = wp
        .controller_owner_ref(&())
        .ok_or(PermanentError::EmptyControllerOwnerReference)?;

    let namespace = ResourceExt::namespace(wp).ok_or(PermanentError::EmptyNamespace)?;
    let name = wp.name_any();

    Ok((namespace, name, controller_ref))
}

#[tracing::instrument(skip_all)]
async fn reconcile(wp: Arc<Webpage>, ctx: Arc<Context>) -> ReconcileResult<Action> {
    info!("reconcile start");

    let (namespace, name, controller_ref) = namespace_name_ref(&wp)?;

    let wp_api = Api::<Webpage>::namespaced(ctx.client.clone(), &namespace);
    let wp_status = wp_api
        .get(&name)
        .await
        .map_err(TransientError::GetWebpage)?
        .status
        .unwrap_or_default();

    let params = params::PatchParams::apply(MANAGER).force();

    if wp.spec.html.len() > rust_operator::MAX_HTML_BYTES {
        apply_status::<Webpage, WebpageStatus>(
            wp_api,
            &name,
            &params,
            &rust_operator::new_too_large_status(&wp, &wp_status),
        )
        .await?;

        let was_too_large = ready_condition(&wp_status)
            .is_some_and(|c| c.reason == CONDITION_REASON_HTML_TOO_LARGE);

        if !was_too_large {
            ctx.recorder
                .publish(
                    &Event {
                        type_: EventType::Warning,
                        reason: CONDITION_REASON_HTML_TOO_LARGE.to_string(),
                        note: Some(format!(
                            "html is {} bytes, max {}",
                            wp.spec.html.len(),
                            rust_operator::MAX_HTML_BYTES
                        )),
                        action: "Reconcile".to_string(),
                        secondary: None,
                    },
                    &wp.object_ref(&()),
                )
                .await
                .unwrap_or_else(|err| warn!(%err, "error publishing event"));
        }

        return Ok(Action::await_change());
    }

    let target_ns = rust_operator::new_namespace(&name, &controller_ref);
    let target_ns_name = target_ns.name_any();

    apply(
        Api::<Namespace>::all(ctx.client.clone()),
        &target_ns_name,
        &params,
        target_ns.clone(),
    )
    .await?;

    let ns = Api::<Namespace>::all(ctx.client.clone())
        .get(&target_ns_name)
        .await
        .map_err(TransientError::GetNamespace)?;

    let owner_ref = ns
        .controller_owner_ref(&())
        .ok_or(PermanentError::EmptyControllerOwnerReference)?;

    let results: [Result<(), ReconcileError>; 4] = tokio::join!(
        apply(
            Api::<ConfigMap>::namespaced(ctx.client.clone(), &target_ns_name),
            &name,
            &params,
            rust_operator::new_config_map(&wp, &name, &owner_ref)
        ),
        apply(
            Api::<Deployment>::namespaced(ctx.client.clone(), &target_ns_name),
            &name,
            &params,
            rust_operator::new_deployment(&wp, &name, &owner_ref)
        ),
        apply(
            Api::<Service>::namespaced(ctx.client.clone(), &target_ns_name),
            &name,
            &params,
            rust_operator::new_service(&name, &owner_ref)
        ),
        apply(
            Api::<HttpRoute>::namespaced(ctx.client.clone(), &target_ns_name),
            &name,
            &params,
            rust_operator::new_http_route(&name, &owner_ref)
        )
    )
    .into();

    for res in results {
        res?;
    }

    update_status(
        wp_api,
        &wp,
        &wp_status,
        &target_ns_name,
        &name,
        &params,
        ctx,
    )
    .await?;

    Ok(Action::requeue(Duration::from_secs(300)))
}

async fn update_status(
    wp_api: Api<Webpage>,
    wp: &Webpage,
    wp_status: &WebpageStatus,
    target_ns_name: &str,
    name: &str,
    params: &params::PatchParams,
    ctx: Arc<Context>,
) -> ReconcileResult<()> {
    let dep_api = Api::<Deployment>::namespaced(ctx.client.clone(), target_ns_name);
    let dep_status = dep_api
        .get(name)
        .await
        .map_err(TransientError::GetDeployment)?
        .status
        .unwrap_or_default();

    apply_status::<Webpage, WebpageStatus>(
        wp_api,
        name,
        params,
        &new_status(wp, wp_status, &dep_status),
    )
    .await
}

#[tracing::instrument(skip_all, fields(
target = target(api.namespace(), name),
kind = K::kind(&<K as Resource>::DynamicType::default()).to_string()
))]
async fn apply<K>(
    api: Api<K>,
    name: &str,
    params: &params::PatchParams,
    k: K,
) -> Result<(), ReconcileError>
where
    K: Resource + Clone + std::fmt::Debug + Serialize + DeserializeOwned,
    K::DynamicType: Default,
{
    info!("apply");

    api.patch(name, params, &Patch::Apply(k))
        .await
        .map_err(|source| TransientError::Patch {
            kind: K::kind(&<K as Resource>::DynamicType::default()).to_string(),
            target: target(api.namespace(), name),
            source: Box::new(source),
        })?;
    Ok(())
}

#[tracing::instrument(skip_all, fields(
target = target(api.namespace(), name),
kind = K::kind(&<K as Resource>::DynamicType::default()).to_string()
))]
async fn apply_status<K, S>(
    api: Api<K>,
    name: &str,
    params: &params::PatchParams,
    status: &S,
) -> Result<(), ReconcileError>
where
    K: std::fmt::Debug,
    K: std::clone::Clone,
    K: Resource<Scope = NamespaceResourceScope> + Serialize + DeserializeOwned,
    <K as kube::Resource>::DynamicType: std::default::Default,
    S: Serialize,
{
    info!("apply status");

    let dynamic_resource = <K as Resource>::DynamicType::default();
    let api_version = K::api_version(&dynamic_resource);
    let kind = K::kind(&dynamic_resource);
    let data = serde_json::json!({
    "apiVersion": api_version,
    "kind": kind,
    "status": serde_json::to_value(status).map_err(PermanentError::Serde)?
    });

    api.patch_status(name, params, &Patch::Apply(data))
        .await
        .map_err(|source| TransientError::PatchStatus {
            kind: kind.to_string(),
            target: target(api.namespace(), name),
            source: Box::new(source),
        })?;
    Ok(())
}

fn target(ns: Option<&str>, name: &str) -> String {
    match ns {
        Some(ns) => format!("{ns}/{name}"),
        None => name.to_string(),
    }
}

#[tracing::instrument(skip_all, fields(%err))]
pub fn error_policy(_wp: Arc<Webpage>, err: &ReconcileError, _ctx: Arc<Context>) -> Action {
    match err {
        ReconcileError::PermanentError(_) => Action::await_change(),
        ReconcileError::TransientError(_) => Action::requeue(Duration::from_secs(5)),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default(false)?;

    let client = Client::try_default().await?;
    let api = Api::<Webpage>::all(client.clone());

    match api.list(&ListParams::default().limit(1)).await {
        Ok(_) => {}
        Err(kube::Error::Api(status)) if status.is_not_found() => {
            return Err(anyhow::anyhow!(
                "crd not found. run `make crd && kubectl apply -f manifests/crd.json`"
            ));
        }
        Err(err) => return Err(anyhow::anyhow!(err)),
    }

    let config = watcher::Config {
        label_selector: Some(rust_operator::managed_by_label_selector()),
        ..Default::default()
    };

    Controller::new(api, watcher::Config::default())
        .shutdown_on_signal()
        .owns(Api::<Namespace>::all(client.clone()), config.clone())
        .owns(Api::<ConfigMap>::all(client.clone()), config.clone())
        .owns(Api::<Deployment>::all(client.clone()), config.clone())
        .owns(Api::<Service>::all(client.clone()), config.clone())
        .owns(Api::<HttpRoute>::all(client.clone()), config)
        .run(
            reconcile,
            error_policy,
            Arc::new(Context {
                recorder: Recorder::new(client.clone(), MANAGER.into()),
                client,
            }),
        )
        .for_each(|result| {
            match result {
                Ok((obj_ref, action)) => {
                    info!(obj_ref = %obj_ref, action = ?action, "reconcile complete");
                }
                Err(err) => match err {
                    Error::ObjectNotFound(obj_ref) => debug!(%obj_ref, "object not found"),
                    Error::ReconcilerFailed(err, obj_ref) => {
                        error!(%obj_ref, %err, "reconcile error");
                    }
                    Error::QueueError(err) => error!(%err, "queue error"),
                    Error::RunnerError(err) => error!(%err, "runner error"),
                },
            }

            futures::future::ready(())
        })
        .await;

    Ok(())
}
