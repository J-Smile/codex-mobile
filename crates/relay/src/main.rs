use codex_mobile_relay::Accounts;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::var_os("CODEX_MOBILE_RELAY_DB")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/data/accounts.sqlite3"));
    let accounts = Accounts::open(&path)?;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, name] if command == "create-account" => {
            println!("{}", serde_json::to_string(&accounts.create(name)?)?);
            return Ok(());
        }
        [command, name] if command == "remove-account" => {
            if !accounts.remove(name)? {
                return Err("账号不存在。".into());
            }
            return Ok(());
        }
        [] => {}
        _ => {
            return Err(
                "用法：codex-mobile-relay [create-account <名称> | remove-account <名称>]".into(),
            )
        }
    }
    let origin = std::env::var("CODEX_MOBILE_WEB_ORIGIN")?;
    let router = codex_mobile_relay::router(origin, accounts);
    let addr = std::env::var("CODEX_MOBILE_RELAY_BIND").unwrap_or_else(|_| "127.0.0.1:8080".into());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    println!("Codex Mobile relay listening on {}", listener.local_addr()?);
    axum::serve(listener, router).await?;
    Ok(())
}
