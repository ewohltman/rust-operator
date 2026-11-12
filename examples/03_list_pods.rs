use k8s_openapi::api::core::v1::Pod;
use kube::ResourceExt;
use kube::core::params;
use kube::{Api, Client};
use tracing::info;

fn parse_list_params<I>(mut args: I) -> params::ListParams
where
    I: Iterator<Item = String>,
{
    for arg in args.by_ref() {
        if arg == "--selector" {
            break;
        }
    }

    match args.next() {
        Some(arg) => params::ListParams::default().labels(&arg),
        None => params::ListParams::default(),
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default()?;

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let pods = Api::<Pod>::all(client);
    let params = parse_list_params(std::env::args());
    let pod_list = pods.list(&params).await?;

    pod_list.iter().for_each(|pod| {
        let namespace = pod.namespace().unwrap_or_default();
        let name = pod.name_any();
        let phase = pod
            .status
            .as_ref()
            .and_then(|status| status.phase.as_deref())
            .unwrap_or("");
        let node = pod
            .spec
            .as_ref()
            .and_then(|spec| spec.node_name.as_deref())
            .unwrap_or("");

        info!(namespace = %namespace, name = %name, phase = %phase, node = %node, "found pod");
    });

    Ok(())
}
