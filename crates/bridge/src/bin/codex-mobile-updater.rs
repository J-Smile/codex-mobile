#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plan = std::env::args_os()
        .nth(1)
        .ok_or("missing update handoff file")?;
    codex_mobile_bridge::updates::apply_handoff(std::path::Path::new(&plan)).await?;
    Ok(())
}
