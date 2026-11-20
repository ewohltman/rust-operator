use kube::ResourceExt;
use kube::api::ListParams;
use kube::core::DynamicObject;
use kube::{Api, Client};
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default(false)?;

    let Some(plural) = std::env::args().nth(1) else {
        return Err(anyhow::anyhow!("no resource provided"));
    };

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let discovery = kube::Discovery::new(client.clone())
        .run_aggregated()
        .await?;

    for group in discovery.groups() {
        for (api_resource, capabilities) in group.recommended_resources() {
            if plural == api_resource.plural {
                let api: Api<DynamicObject> =
                    Api::dynamic(client.clone(), &api_resource, &capabilities.scope);

                for obj in api.list(&ListParams::default()).await? {
                    info!(name = %obj.name_any(), version = %api_resource.version, kind = %api_resource.kind, "object found");
                }

                return Ok(());
            }
        }
    }

    Err(anyhow::anyhow!("resource '{plural}' not found"))
}
