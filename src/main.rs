use futures::StreamExt;
use kube::api::ListParams;
use kube::config::KubeConfigOptions;
use kube::runtime::Controller;
use kube::runtime::controller::Action;
use kube::runtime::finalizer::{Event as FinalizerEvent, finalizer};
use kube::runtime::watcher::Config;
use kube::{Api, Client, ResourceExt};
use rust_operator::{Webpage, kind_cluster_guard};
use std::sync::Arc;
use std::time::Duration;
use tracing::{error, info};

// Define a unique DNS-style finalizer name
const FINALIZER_NAME: &str = "demo.ephemeral-roles.net/cleanup";

type ReconcileResult<T, E = ReconcileError> = Result<T, E>;

/// Errors returned by this module.
#[derive(thiserror::Error, Debug)]
enum ReconcileError {
    #[error(transparent)]
    Api(#[from] kube::Error),

    #[error("namespace not set")]
    EmptyNamespace,

    #[error("finalizer error: {0}")]
    FinalizerError(String),
}

struct Context {
    client: Client,
}

#[tracing::instrument(skip(wp, ctx))]
async fn reconcile(wp: Arc<Webpage>, ctx: Arc<Context>) -> ReconcileResult<Action> {
    let Some(namespace) = wp.namespace() else {
        return Err(ReconcileError::EmptyNamespace);
    };

    let api: Api<Webpage> = Api::namespaced(ctx.client.clone(), &namespace);

    finalizer(&api, FINALIZER_NAME, wp, |event| async {
        match event {
            FinalizerEvent::Apply(resource) => apply(&resource, ctx.clone()),
            FinalizerEvent::Cleanup(resource) => cleanup(&resource, ctx.clone()),
        }
    })
    .await
    .map_err(|e| ReconcileError::FinalizerError(e.to_string()))
}

#[tracing::instrument(skip(wp, _ctx))]
fn apply(wp: &Arc<Webpage>, _ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    // 1. Reconcile your normal state here
    // The finalizer is already attached to `wp`
    let Some(namespace) = wp.namespace() else {
        return Err(ReconcileError::EmptyNamespace);
    };
    let name = wp.name_any();

    info!(%namespace, %name, "success");

    Ok(Action::requeue(Duration::from_secs(300)))
}

#[tracing::instrument(skip(wp, _ctx))]
fn cleanup(wp: &Arc<Webpage>, _ctx: Arc<Context>) -> Result<Action, ReconcileError> {
    // 2. Clean up external resources
    // Must be idempotent in case cleanup fails partway and retries
    let Some(namespace) = wp.namespace() else {
        return Err(ReconcileError::EmptyNamespace);
    };
    let name = wp.name_any();

    info!(%namespace, %name, "success");

    // Returning Ok removes the finalizer and allows K8s to delete the resource
    Ok(Action::await_change())
}

#[allow(clippy::needless_pass_by_value)]
#[tracing::instrument(skip(_wp, _ctx))]
fn error_policy(_wp: Arc<Webpage>, err: &ReconcileError, _ctx: Arc<Context>) -> Action {
    Action::requeue(Duration::from_secs(5))
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

    Controller::new(api, Config::default())
        .shutdown_on_signal()
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
