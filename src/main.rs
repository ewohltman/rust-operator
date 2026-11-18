use futures::StreamExt;
use k8s_openapi::NamespaceResourceScope;
use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{ConfigMap, Service};
use kube::api::{ListParams, Patch};
use kube::config::KubeConfigOptions;
use kube::core::params;
use kube::runtime::controller::Action;
use kube::runtime::{Controller, watcher};
use kube::{Api, Client, Resource, ResourceExt};
use rust_operator::{
    Context, MANAGER, ReconcileError, ReconcileResult, Webpage, WebpageStatus, error_policy,
    kind_cluster_guard,
};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

#[tracing::instrument(skip(wp, ctx))]
async fn reconcile(wp: Arc<Webpage>, ctx: Arc<Context>) -> ReconcileResult<Action> {
    let Some(namespace) = wp.namespace() else {
        return Err(ReconcileError::EmptyNamespace);
    };

    let controller_ref = wp
        .controller_owner_ref(&())
        .ok_or(ReconcileError::EmptyControllerOwnerReference)?;

    let name = wp.name_any();
    let params = params::PatchParams::apply(MANAGER).force();

    info!(%namespace, %name, "apply");

    let cm = rust_operator::desired_config_map(&wp, &namespace, &name, &controller_ref);
    let dep = rust_operator::desired_deployment(&wp, &namespace, &name, &controller_ref);
    let service = rust_operator::desired_service(&namespace, &name, &controller_ref);

    let (res_cm, res_dep, res_service) = tokio::join!(
        apply(&namespace, &name, &params, cm, &ctx),
        apply(&namespace, &name, &params, dep, &ctx),
        apply(&namespace, &name, &params, service, &ctx),
    );

    res_cm?;
    res_dep?;
    res_service?;

    info!(%namespace, %name, "apply status");

    let actual_status = actual_status(&wp, &namespace, &name, &ctx).await?;
    apply_status::<Webpage>(
        &ctx,
        &namespace,
        &name,
        &params,
        serde_json::to_value(&actual_status)?,
    )
    .await?;

    info!(%namespace, %name, "success");

    if actual_status == rust_operator::desired_status(&wp) {
        Ok(Action::requeue(Duration::from_secs(300)))
    } else {
        Ok(Action::await_change())
    }
}

async fn actual_status(
    wp: &Webpage,
    namespace: &str,
    name: &str,
    ctx: &Arc<Context>,
) -> Result<WebpageStatus, ReconcileError> {
    Ok(WebpageStatus {
        ready_replicas: Api::<Deployment>::namespaced(ctx.client.clone(), namespace)
            .get(name)
            .await?
            .status
            .unwrap_or_default()
            .ready_replicas
            .unwrap_or_default(),
        observed_generation: wp.metadata.generation,
    })
}

#[tracing::instrument(skip(ctx, params, k))]
async fn apply<K>(
    namespace: &str,
    name: &str,
    params: &params::PatchParams,
    k: K,
    ctx: &Arc<Context>,
) -> Result<(), ReconcileError>
where
    K: std::fmt::Debug,
    K: std::clone::Clone,
    K: Resource<Scope = NamespaceResourceScope>
        + k8s_openapi::serde::Serialize
        + serde::de::DeserializeOwned,
    <K as kube::Resource>::DynamicType: std::default::Default,
{
    let dynamic_resource = <K as Resource>::DynamicType::default();
    let kind = K::kind(&dynamic_resource);
    info!(%namespace, %name, kind = kind.to_string(), "apply");

    let api: Api<K> = Api::namespaced(ctx.client.clone(), namespace);
    api.patch(name, params, &Patch::Apply(k)).await?;
    Ok(())
}

#[tracing::instrument(skip(ctx, params, status))]
async fn apply_status<K>(
    ctx: &Arc<Context>,
    namespace: &str,
    name: &str,
    params: &params::PatchParams,
    status: serde_json::Value,
) -> Result<(), ReconcileError>
where
    K: std::fmt::Debug,
    K: std::clone::Clone,
    K: Resource<Scope = NamespaceResourceScope>
        + k8s_openapi::serde::Serialize
        + serde::de::DeserializeOwned,
    <K as kube::Resource>::DynamicType: std::default::Default,
{
    let api: Api<K> = Api::namespaced(ctx.client.clone(), namespace);
    let data = serde_json::json!({
    "apiVersion": K::api_version(&<K as Resource>::DynamicType::default()),
    "kind":K::kind(&<K as Resource>::DynamicType::default()),
    "status": status,
    });

    api.patch_status(name, params, &Patch::Apply(data)).await?;
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default()?;

    let client = Client::try_from(match kube::Config::incluster() {
        Ok(ic) => ic,
        Err(e1) => {
            match kube::Config::from_custom_kubeconfig(
                kind_cluster_guard()?,
                &KubeConfigOptions::default(),
            )
            .await
            {
                Ok(kc) => kc,
                Err(e2) => {
                    anyhow::bail!("in cluster error: {e1}: kubeconfig error: {e2}");
                }
            }
        }
    })?;

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
        .owns(Api::<ConfigMap>::all(client.clone()), config.clone())
        .owns(Api::<Deployment>::all(client.clone()), config.clone())
        .owns(Api::<Service>::all(client.clone()), config)
        .run(reconcile, error_policy, Arc::new(Context { client }))
        .for_each(|result| {
            match result {
                Ok((obj_ref, action)) => {
                    info!(obj_ref = %obj_ref, action = ?action, "reconcile complete");
                }
                Err(err) => error!(err = %err, "reconcile error"),
            }

            futures::future::ready(())
        })
        .await;

    Ok(())
}
