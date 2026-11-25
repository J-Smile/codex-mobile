use codex_mobile_bridge::{config::ServiceConfig, http::Bridge, paths::Paths};
use std::path::PathBuf;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "codex_mobile_bridge=info".into()),
        )
        .init();
    let mut args = std::env::args().skip(1);
    let mut web_root = PathBuf::from("dist");
    while let Some(arg) = args.next() {
        if arg == "--web-root" {
            web_root = PathBuf::from(args.next().ok_or("--web-root requires a directory")?);
        } else {
            return Err(format!("未知参数：{arg}").into());
        }
    }
    let paths = Paths::from_env(web_root)?;
    let config = ServiceConfig::load(&paths.data_home)?;
    let app = Bridge::open(paths, config, false)?;
    let server = app.start().await?;
    println!(
        "Codex Mobile Rust: http://localhost:{}",
        server.address.port()
    );
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
    }
    #[cfg(windows)]
    tokio::signal::ctrl_c().await?;
    server.stop().await?;
    Ok(())
}
