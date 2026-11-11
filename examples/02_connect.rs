use kube::Client;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .compact()
            .with_file(true)
            .with_line_number(true)
            .with_target(false)
            .with_env_filter(
                EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
            )
            .finish(),
    )?;

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let version = client.apiserver_version().await?;

    info!(major = %version.major, minor = %version.minor, "connected to API server");

    Ok(())
}
