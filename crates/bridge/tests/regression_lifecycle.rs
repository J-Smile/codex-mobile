mod support;

use codex_mobile_bridge::{
    auth::Auth,
    config::ServiceConfig,
    http::{Bridge, Server},
    operations::{Claim, Operations},
    BridgeError,
};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;
use support::Fixture;

async fn start(fixture: &Fixture) -> Server {
    Bridge::open(
        fixture.paths.clone(),
        ServiceConfig {
            port: 0,
            bind: "127.0.0.1".into(),
            ..Default::default()
        },
        false,
    )
    .unwrap()
    .start()
    .await
    .unwrap()
}

async fn login(server: &Server, client: &Client, key: &str) -> String {
    let response = client
        .post(format!("http://{}/api/login", server.address))
        .json(&json!({"accessKey": key}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn pending(server: &Server, client: &Client, cookie: &str) -> Value {
    let response = client
        .get(format!("http://{}/api/operations", server.address))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    response.json().await.unwrap()
}

async fn lookup(server: &Server, client: &Client, cookie: &str, ids: &str) -> Value {
    let response = client
        .get(format!("http://{}/api/operations", server.address))
        .query(&[("ids", ids)])
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

// AUTH-006、OPS-008：组合验证实际 HTTP、认证持久化和操作账本的生命周期。
#[tokio::test]
async fn local_sessions_restart_logout_and_key_rotation_preserve_operation_facts() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new();
        let client = Client::builder().no_proxy().build().unwrap();
        let key = Auth::open(fixture.paths.data_home.clone())
            .unwrap()
            .key()
            .to_owned();
        let owner = format!("local:{}", codex_mobile_bridge::digest(&key));
        let operation = "regression-operation-001";
        {
            let operations = Operations::open(&fixture.paths.data_home).unwrap();
            assert!(matches!(
                operations
                    .claim(&owner, operation, "hash", "fixture", "chat", "message")
                    .unwrap(),
                Claim::Dispatch
            ));
        }

        let server = start(&fixture).await;
        let first = login(&server, &client, &key).await;
        let second = login(&server, &client, &key).await;
        assert_ne!(first, second);
        let saved = pending(&server, &client, &first).await;
        assert_eq!(saved["operations"][0]["id"], operation);
        assert_eq!(saved["operations"][0]["phase"], "unknown");
        assert_eq!(pending(&server, &client, &second).await, saved);

        let logout = client
            .post(format!("http://{}/api/logout", server.address))
            .header("cookie", &first)
            .send()
            .await
            .unwrap();
        assert_eq!(logout.status(), 200);
        assert_eq!(
            client
                .get(format!("http://{}/api/operations", server.address))
                .header("cookie", &first)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        assert_eq!(pending(&server, &client, &second).await, saved);
        server.stop().await.unwrap();

        let server = start(&fixture).await;
        assert_eq!(pending(&server, &client, &second).await, saved);
        let close = client
            .post(format!(
                "http://{}/api/operations/{operation}",
                server.address
            ))
            .header("cookie", &second)
            .json(&json!({"confirm": "close-unknown"}))
            .send()
            .await
            .unwrap();
        assert_eq!(close.status(), 200);
        assert_eq!(
            pending(&server, &client, &second).await["operations"],
            json!([])
        );
        let closed = lookup(&server, &client, &second, operation).await;
        assert_eq!(closed["results"][0]["phase"], "unknown");
        assert_eq!(closed["results"][0]["closed"], true);

        let rotated = server.app.rotate_local().await.unwrap();
        assert_ne!(rotated, key);
        assert_eq!(
            client
                .get(format!("http://{}/api/session?ping=1", server.address))
                .header("cookie", &second)
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let third = login(&server, &client, &rotated).await;
        assert_eq!(
            pending(&server, &client, &third).await["operations"],
            json!([])
        );
        assert_eq!(
            lookup(&server, &client, &third, operation).await["results"],
            json!([{"id": operation, "phase": "missing"}])
        );
        server.stop().await.unwrap();

        // 结案和轮换都不把原未知操作变为成功/未发送，也不清空去重事实。
        let operations = Operations::open(&fixture.paths.data_home).unwrap();
        assert!(operations.pending(&owner).unwrap().is_empty());
        let Claim::Saved(Err(error)) = operations
            .claim(&owner, operation, "hash", "new-instance", "chat", "message")
            .unwrap()
        else {
            panic!("已结案的未知操作不能被重启或轮换变成可重发");
        };
        assert_eq!(error.outcome.as_deref(), Some("unknown"));
    })
    .await
    .unwrap();
}

// OPS-002、OPS-003、OPS-005、OPS-006、OPS-007：终态查询通过认证HTTP边界核对。
#[tokio::test]
async fn operation_lookup_returns_terminal_facts_with_bounded_ids_and_owner_isolation() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let fixture = Fixture::new();
        let client = Client::builder().no_proxy().build().unwrap();
        let key = Auth::open(fixture.paths.data_home.clone())
            .unwrap()
            .key()
            .to_owned();
        let owner = format!("local:{}", codex_mobile_bridge::digest(&key));
        let accepted = "regression-accepted";
        let unsent = "regression-unsent";
        let foreign = "regression-foreign";
        let created = "regression-created";
        {
            let operations = Operations::open(&fixture.paths.data_home).unwrap();
            for (id, target, kind, result) in [
                (
                    accepted,
                    "chat",
                    "message",
                    Ok(json!({"status": "accepted", "messageId": "confirmed", "input": "not-in-receipt"})),
                ),
                (
                    unsent,
                    "chat",
                    "message",
                    Err(BridgeError::invalid("fixture rejected").before_send()),
                ),
                (
                    created,
                    "new:project",
                    "create",
                    Ok(json!({"status": "created", "threadId": "created-chat", "firstTurnAccepted": false})),
                ),
            ] {
                operations
                    .claim(&owner, id, "hash", "fixture", target, kind)
                    .unwrap();
                operations.finish(&owner, id, &result).unwrap();
            }
            operations
                .claim("another-owner", foreign, "hash", "fixture", "private", "message")
                .unwrap();
        }
        let server = start(&fixture).await;
        let url = format!("http://{}/api/operations", server.address);
        assert_eq!(
            client
                .get(&url)
                .query(&[("ids", accepted)])
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
        let cookie = login(&server, &client, &key).await;
        let legacy = pending(&server, &client, &cookie).await;
        assert_eq!(legacy["operations"], json!([]));
        assert!(legacy.get("results").is_none());
        let result = lookup(
            &server,
            &client,
            &cookie,
            &format!("{accepted},{unsent},{created},{foreign},regression-missing"),
        )
        .await;
        assert_eq!(result["operations"], json!([]));
        let results = result["results"].as_array().unwrap();
        assert_eq!(results.len(), 5);
        assert_eq!(results[0]["phase"], "accepted");
        assert_eq!(results[0]["receipt"]["messageId"], "confirmed");
        assert!(results[0]["receipt"].get("input").is_none());
        assert_eq!(results[1]["phase"], "not-dispatched");
        assert!(results[1]["receipt"].is_null());
        assert_eq!(results[2]["kind"], "create");
        assert_eq!(results[2]["receipt"]["threadId"], "created-chat");
        assert_eq!(results[2]["receipt"]["firstTurnAccepted"], false);
        assert_eq!(results[3], json!({"id": foreign, "phase": "missing"}));
        assert_eq!(results[4], json!({"id": "regression-missing", "phase": "missing"}));

        let owner_override: Value = client
            .get(&url)
            .query(&[("ids", foreign), ("owner", "another-owner")])
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(owner_override["results"][0]["phase"], "missing");

        let hundred = (0..100)
            .map(|index| format!("regression-missing-{index}"))
            .collect::<Vec<_>>()
            .join(",");
        assert_eq!(
            lookup(&server, &client, &cookie, &hundred).await["results"]
                .as_array()
                .unwrap()
                .len(),
            100
        );
        for invalid in [
            String::new(),
            "short".into(),
            "x".repeat(101),
            format!("{accepted},"),
            "regression-invalid!".into(),
            format!("{hundred},regression-over-limit"),
        ] {
            assert_eq!(
                client
                    .get(&url)
                    .query(&[("ids", invalid)])
                    .header("cookie", &cookie)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                400
            );
        }
        server.stop().await.unwrap();
    })
    .await
    .unwrap();
}
