use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::runtime::{WatchStreamExt, watcher};
use kube::{Api, Client};
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default()?;

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let pods = Api::<Pod>::all(client);

    let mut stream = std::pin::pin!(
        kube::runtime::watcher(pods, watcher::Config::default())
            .default_backoff()
            .touched_objects()
    );

    while let Some(result) = stream.next().await {
        match result {
            Ok(pod) => info!(name = %pod.name_any(), "pod applied"),
            Err(err) => error!(err = %err, ""),
        }
    }

    Ok(())
}
