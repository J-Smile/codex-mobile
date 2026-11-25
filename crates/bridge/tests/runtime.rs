mod support;
use codex_mobile_bridge::{
    runtime::{self, read_runtime},
    session::Session,
};
use serde_json::json;

#[test]
fn legacy_live_runtime_is_rejected_without_rewriting_or_stopping_it() {
    let f = support::Fixture::new();
    let bytes =
        json!({"pid":std::process::id(),"url":"ws://127.0.0.1:8000","cliPath":"old"}).to_string();
    let path = f.paths.runtime_home.join("runtime.json");
    std::fs::write(&path, &bytes).unwrap();
    assert!(read_runtime(&f.paths.runtime_home).is_err());
    assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
    assert!(runtime::process_alive(std::process::id()));
}
#[test]
fn old_transport_or_missing_broker_credentials_are_rejected_without_migration() {
    let f = support::Fixture::new();
    let record = f.runtime("ws://127.0.0.1:8000");
    let path = record.path(&f.paths.runtime_home);
    for field in ["contract", "authToken", "missingToken"] {
        let mut value = serde_json::to_value(&record).unwrap();
        match field {
            "contract" => value["contract"] = json!("local-desktop-0.171.0-alpha.13-ws-v2"),
            "authToken" => value["authToken"] = json!("invalid"),
            _ => {
                value.as_object_mut().unwrap().remove("authToken");
            }
        }
        let bytes = value.to_string();
        std::fs::write(&path, &bytes).unwrap();
        assert!(read_runtime(&f.paths.runtime_home).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
    record.publish(&f.paths.runtime_home).unwrap();
    assert_eq!(
        read_runtime(&f.paths.runtime_home).unwrap().unwrap(),
        record
    );
}
#[test]
fn ownership_rejects_reused_pid_retirement_and_competing_instances() {
    let f = support::Fixture::new();
    let mut old = f.runtime("ws://127.0.0.1:8000");
    assert_eq!(read_runtime(&f.paths.runtime_home).unwrap().unwrap(), old);
    old.desktop.started = "reused-pid".into();
    old.publish(&f.paths.runtime_home).unwrap();
    assert!(read_runtime(&f.paths.runtime_home).is_err());
    old.desktop = codex_mobile_bridge::process_identity::ProcessIdentity::current().unwrap();
    old.state = "retired".into();
    old.publish(&f.paths.runtime_home).unwrap();
    assert!(read_runtime(&f.paths.runtime_home).is_err());
    old.state = "ready".into();
    old.publish(&f.paths.runtime_home).unwrap();
    let next = f.runtime("ws://127.0.0.1:8001");
    assert!(read_runtime(&f.paths.runtime_home).is_err());
    std::fs::remove_file(old.path(&f.paths.runtime_home)).unwrap();
    assert_eq!(read_runtime(&f.paths.runtime_home).unwrap().unwrap(), next);
}
#[tokio::test]
async fn connected_socket_cannot_write_after_owner_is_retired() {
    let f = support::Fixture::new();
    let mock = support::MockDesktop::start(&f).await;
    let session = Session::new(f.paths.clone());
    let connection = session.connect().await.unwrap();
    let mut runtime = read_runtime(&f.paths.runtime_home).unwrap().unwrap();
    runtime.state = "retired".into();
    runtime.publish(&f.paths.runtime_home).unwrap();
    let error = connection
        .rpc("turn/start", json!({"threadId":"chat","input":[]}))
        .await
        .unwrap_err();
    assert!(error.before_send);
    assert_eq!(mock.count("turn/start").await, 0);
    assert!(session.connect().await.is_err());
    session.close().await;
}
#[test]
fn only_explicit_local_initialize_can_publish_a_connection() {
    for capability in [json!(false), json!(null)] {
        assert!(!runtime::local_initialize(
            &json!({"id":"init","method":"initialize","params":{"capabilities":{"explicitGatewayOauth":capability}}})
        ));
    }
    assert!(runtime::local_initialize(
        &json!({"id":"init","method":"initialize","params":{"capabilities":{"explicitGatewayOauth":true}}})
    ));
    assert!(!runtime::supported_initialize(
        &json!({"userAgent":"Codex Desktop/0.172.0 (new)"})
    ));
}
#[test]
fn only_primary_stdio_invocations_are_eligible_for_brokering() {
    let args = ["-c", "fixture=\"quoted value\"", "app-server", "--stdio"].map(str::to_string);
    assert!(runtime::main_app_server(&args));
    for helper in [
        vec!["--version"],
        vec!["app-server", "generate-json-schema"],
        vec!["app-server", "--listen=stdio://"],
    ] {
        assert!(!runtime::main_app_server(
            &helper.into_iter().map(str::to_string).collect::<Vec<_>>()
        ));
    }
}
#[tokio::test]
async fn each_launch_keeps_its_original_cli_binding() {
    let f = support::Fixture::new();
    let cli = support::process::child(f.temp.path(), "helper-cli");
    let other = support::process::child(f.temp.path(), "other-cli");
    let source = std::path::Path::new(env!("CARGO_BIN_EXE_codex-mobile-proxy"));
    let old = runtime::prepare_proxy(&f.paths.runtime_home, source, &cli).unwrap();
    let new = runtime::prepare_proxy(&f.paths.runtime_home, source, &other).unwrap();
    assert_ne!(old, new);
    let output = tokio::process::Command::new(old)
        .arg("--version")
        .env_remove("CODEX_MOBILE_ORIGINAL_CLI")
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(42));
    assert!(cli.with_extension("launched").exists());
    assert!(!other.with_extension("launched").exists());
    assert!(read_runtime(&f.paths.runtime_home).unwrap().is_none());
}
