use kube::config;

/// Errors returned by the kind cluster guard.
#[derive(thiserror::Error, Debug)]
pub enum Error {
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
