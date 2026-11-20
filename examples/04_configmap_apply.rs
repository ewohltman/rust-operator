use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::Patch;
use kube::core::params;
use kube::{Api, Client};
use tracing::{debug, info};

async fn delete(config_maps: &Api<ConfigMap>, name: &str) -> anyhow::Result<()> {
    match config_maps
        .delete(name, &params::DeleteParams::default())
        .await
    {
        Ok(either::Either::Left(_)) => info!(name = %name, "deleting"),
        Ok(either::Either::Right(_)) => info!(name = %name, "deleted"),
        Err(kube::Error::Api(status)) if status.is_not_found() => {
            debug!(name = %name, "already deleted");
        }
        Err(err) => {
            return Err(err.into());
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rust_operator::init_tracing_default(false)?;

    let client = Client::try_from(rust_operator::kind_cluster_guard()?)?;
    let config_maps = Api::<ConfigMap>::namespaced(client, "default");
    let params = params::PatchParams::apply("learn-kube").force();

    let name = "example-cm";
    let patch = Patch::Apply(&serde_json::json!({
        "apiVersion": "v1",
        "kind": "ConfigMap",
        "metadata": {
            "name": name
        },
        "data": {
            "Hello": "World"
        }
    }));

    info!(name = %name, "applying config map");
    config_maps.patch(name, &params, &patch).await?;

    info!(name = %name, "deleting config map");
    delete(&config_maps, name).await?;

    info!(name = %name, "deleting config map again");
    delete(&config_maps, name).await?;

    Ok(())
}
