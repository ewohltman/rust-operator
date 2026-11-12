use kube::config;
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
        .with_file(true)
        .with_line_number(true)
        .with_target(false)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .finish()
}
