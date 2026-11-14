use kube::CustomResource;
use kube::KubeSchema;
use kube::config;
use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

/// Errors returned by this module.
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error(transparent)]
    TracingSetup(#[from] tracing::subscriber::SetGlobalDefaultError),

    #[error(transparent)]
    Read(#[from] config::KubeconfigError),

    #[error("current context not set")]
    CurrentContextNotSet,

    #[error("current context not kind: {0}")]
    CurrentContextNotKind(String),
}

/// Defines a Result type for this module.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Returns a Kube config if current context is set to a kind cluster.
///
/// # Errors
///
/// Returns an `Err` if the file cannot be read, current context is not set, or
/// if the current context does not start with 'kind-'.
#[tracing::instrument]
pub fn kind_cluster_guard() -> Result<config::Kubeconfig> {
    let config = config::Kubeconfig::read()?;
    match &config.current_context {
        Some(ctx) => {
            if !ctx.starts_with("kind-") {
                return Err(Error::CurrentContextNotKind(ctx.clone()));
            }

            Ok(config)
        }
        None => Err(Error::CurrentContextNotSet),
    }
}

/// Sets global tracing subscriber with defaults.
///
/// # Errors
///
/// Returns an `Err` if the global tracing subscriber could not be set.
pub fn init_tracing_default() -> Result<()> {
    init_tracing(new_default_tracing_subscriber())
}

/// Sets global tracing subscriber with the given subscriber.
///
/// # Errors
///
/// Returns an `Err` if the global tracing subscriber could not be set.
fn init_tracing<S>(subscriber: S) -> Result<()>
where
    S: tracing::Subscriber + Send + Sync + 'static,
{
    Ok(tracing::subscriber::set_global_default(subscriber)?)
}

/// Returns a new tracing subscriber with standardized config.
#[must_use]
fn new_default_tracing_subscriber() -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::fmt()
        .compact()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish()
}

/// A static web page served by nginx.
#[derive(CustomResource, Deserialize, Serialize, Clone, Debug, KubeSchema, Eq, PartialEq)]
#[kube(
    group = "demo.ephemeral-roles.net",
    version = "v1alpha1",
    kind = "Webpage",
    namespaced
)]
#[kube(status = "WebpageStatus", shortname = "wp", doc = "An HTML webpage.")]
#[serde(rename_all = "camelCase")]
pub struct WebpageSpec {
    /// The HTML document to serve as `index.html`.
    #[schemars(length(max = 102_400))]
    #[x_kube(validation = Rule::new("self.lowerAscii().startsWith('<!doctype html>')").message("not a full HTML document"))]
    pub html: String,
    /// How many nginx pods to run at the same time.
    #[serde(default = "default_replicas")]
    #[schemars(range(min = 0, max = 10))]
    pub replicas: i32,
}

/// Observed state of a `Webpage`, written only by the operator.
#[derive(Deserialize, Serialize, Clone, Debug, Default, KubeSchema, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WebpageStatus {
    /// Ready pods, as reported by the Deployment.
    pub ready_replicas: i32,
    /// The `metadata.generation` this status was computed from.
    pub observed_generation: Option<i64>,
}

fn default_replicas() -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::core::CustomResourceExt;

    #[test]
    fn test_crd_spec_change() {
        let new = serde_json::to_value(Webpage::crd()).unwrap();
        let current: serde_json::Value =
            serde_json::from_reader(std::fs::File::open("manifests/crd.json").unwrap()).unwrap();

        pretty_assertions::assert_eq!(new, current, "crd changed. run `make crd` to regenerate.");
    }
}
