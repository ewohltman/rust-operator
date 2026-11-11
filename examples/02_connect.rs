use kube::Client;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default()?;

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let version = client.apiserver_version().await?;

    info!(major = %version.major, minor = %version.minor, "connected to API server");

    Ok(())
}
