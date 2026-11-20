use futures::StreamExt;
use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::runtime::{WatchStreamExt, reflector, watcher};
use kube::{Api, Client};
use std::collections::BTreeMap;
use tracing::{error, info};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default(false)?;

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let pods = Api::<Pod>::all(client);
    let (cr, cw) = reflector::store();

    let mut watcher_handle = tokio::spawn(async move {
        let watcher = kube::runtime::watcher(pods, watcher::Config::default());
        let rf = reflector(cw, watcher);

        rf.default_backoff()
            .touched_objects()
            .for_each(|result| async move {
                match result {
                    Ok(pod) => info!(name = pod.name_any(), "pod change"),
                    Err(err) => error!(err = %err, "stream error"),
                }
            })
            .await;
    });

    cr.wait_until_ready().await?;
    let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));

    loop {
        tokio::select! {
            x = &mut watcher_handle => {
                if let Err(err) = x {
                    anyhow::bail!(err);
                }

                anyhow::bail!("watcher terminated");
            },
            _ = interval.tick() => {
                let mut phases: BTreeMap<String, usize> = BTreeMap::new();

                for pod in cr.state() {
                    if let Some(status) = &pod.status
                        && let Some(phase) = &status.phase
                    {
                        *phases.entry(phase.clone()).or_default() += 1;
                    }
                }

                for (phase, count) in &phases {
                    info!(phase = %phase, count = %count, "it's not a phase mom!");
                }
            }
        }
    }
}
