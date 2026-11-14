use rust_operator::WebpageSpec;

fn main() -> anyhow::Result<()> {
    let wp = rust_operator::Webpage::new(
        "example",
        WebpageSpec {
            html: "<!doctype html></html>".to_string(),
            replicas: 1,
        },
    );

    println!("{}", serde_json::to_string_pretty(&wp)?);

    Ok(())
}
