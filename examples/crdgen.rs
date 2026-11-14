use kube::core::CustomResourceExt;

fn main() -> anyhow::Result<()> {
    let crd = rust_operator::Webpage::crd();

    println!("{}", serde_json::to_string_pretty(&crd)?);

    Ok(())
}
