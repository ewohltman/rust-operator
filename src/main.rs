use futures::StreamExt;
use gateway_api::httproutes::HttpRoute;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Namespace, Service};
use k8s_openapi::serde::Serialize;
use kube::api::{ListParams, Patch};
use kube::core::params;
use kube::runtime::controller::{Action, Error};
use kube::runtime::{Controller, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use rust_operator::{
    Context, MANAGER, ReconcileError, ReconcileResult, Webpage, WebpageStatus, error_policy,
};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info};

#[tracing::instrument(skip_all, fields(namespace = ?wp.metadata.namespace))]
async fn reconcile(wp: Arc<Webpage>, ctx: Arc<Context>) -> ReconcileResult<Action> {
    info!("reconcile start");

    let Some(namespace) = ResourceExt::namespace(wp.as_ref()) else {
        return Err(ReconcileError::EmptyNamespace);
    };

    let controller_ref = wp
        .controller_owner_ref(&())
        .ok_or(ReconcileError::EmptyControllerOwnerReference)?;

    let name = wp.name_any();
    let params = params::PatchParams::apply(MANAGER).force();

    let ns = rust_operator::new_namespace(&name, &controller_ref);
    let Some(ns_name) = ns.metadata.clone().name else {
        return Err(ReconcileError::EmptyDesiredNamespace);
    };

    apply(
        Api::<Namespace>::all(ctx.client.clone()),
        &ns_name,
        &params,
        ns.clone(),
    )
    .await?;

    let ns = Api::<Namespace>::all(ctx.client.clone())
        .get(&ns_name)
        .await?;

    let owner_ref = ns
        .controller_owner_ref(&())
        .ok_or(ReconcileError::EmptyControllerOwnerReference)?;

    let results: [Result<(), ReconcileError>; 4] = tokio::join!(
        apply(
            Api::<ConfigMap>::namespaced(ctx.client.clone(), &ns_name),
            &name,
            &params,
            rust_operator::new_config_map(&wp, &name, &owner_ref)
        ),
        apply(
            Api::<Deployment>::namespaced(ctx.client.clone(), &ns_name),
            &name,
            &params,
            rust_operator::new_deployment(&wp, &name, &owner_ref)
        ),
        apply(
            Api::<Service>::namespaced(ctx.client.clone(), &ns_name),
            &name,
            &params,
            rust_operator::new_service(&name, &owner_ref)
        ),
        apply(
            Api::<HttpRoute>::namespaced(ctx.client.clone(), &ns_name),
            &name,
            &params,
            rust_operator::new_http_route(&name, &owner_ref)
        )
    )
    .into();

    for res in results {
        res?;
    }

    let dep_api = Api::<Deployment>::namespaced(ctx.client.clone(), &ns_name);
    let dep_status = dep_api.get(&name).await?.status.unwrap_or_default();

    let wp_api = Api::<Webpage>::namespaced(ctx.client.clone(), &namespace);
    let wp_status = wp_api.get(&name).await?.status.unwrap_or_default();

    apply_status::<Webpage, WebpageStatus>(
        wp_api,
        &name,
        &params,
        &rust_operator::new_status(&wp, &wp_status, &dep_status),
    )
    .await?;

    Ok(Action::requeue(Duration::from_secs(300)))
}

#[tracing::instrument(skip(api, params, k))]
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
    let dynamic_resource = <K as Resource>::DynamicType::default();
    let kind = K::kind(&dynamic_resource);

    if let Some(namespace) = api.namespace() {
        info!(%kind, "apply {namespace}/{name}");
    } else {
        info!(%kind, "apply {name}");
    }

    api.patch(name, params, &Patch::Apply(k)).await?;
    Ok(())
}

#[tracing::instrument(skip(api, params, status))]
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
    let dynamic_resource = <K as Resource>::DynamicType::default();
    let api_version = K::api_version(&dynamic_resource);
    let kind = K::kind(&dynamic_resource);

    if let Some(namespace) = api.namespace() {
        info!(%kind, "apply status {namespace}/{name}");
    } else {
        info!(%kind, "apply status {name}");
    }

    let data = serde_json::json!({
    "apiVersion": api_version,
    "kind": kind,
    "status": serde_json::to_value(status)?
    });

    api.patch_status(name, params, &Patch::Apply(data)).await?;
    Ok(())
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
        .run(reconcile, error_policy, Arc::new(Context { client }))
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
