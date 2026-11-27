mod support;
use codex_mobile_bridge::{config::ServiceConfig, host::Host};
use serde_json::json;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

async fn wait_sync(host: &Arc<Host>, expected: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if host.status().await.unwrap()["sync"]["state"] == expected {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn status_and_stop_do_not_connect_or_start_desktop() {
    let f = support::Fixture::new();
    let mock = support::MockDesktop::start(&f).await;
    f.unpublish_runtime();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    ServiceConfig {
        port,
        bind: "127.0.0.1".into(),
        ..Default::default()
    }
    .save(&f.paths.data_home)
    .unwrap();
    let host = Host::open(f.paths.clone(), false).unwrap();
    host.start().await.unwrap();
    for _ in 0..3 {
        assert_eq!(
            host.status().await.unwrap()["sync"]["state"],
            "history-only"
        );
    }
    assert!(mock.calls.lock().await.is_empty());
    host.stop().await.unwrap();
    host.start().await.unwrap();
    assert_eq!(host.status().await.unwrap()["runtime"]["ready"], false);
    host.stop().await.unwrap();
}

#[tokio::test]
async fn update_preparation_stays_exclusive_at_every_stage_and_failures_release_it() {
    let f = support::Fixture::new();
    let host = Host::open(f.paths.clone(), false).unwrap();
    for stage in ["checking", "downloading", "verifying", "extracting"] {
        let state = json!({"phase":"checking","stage":stage,"message":"fixture progress","downloadedBytes":128,"totalBytes":null});
        *host.update_state.write().await = state.clone();
        assert!(!host.prepare_update().await.unwrap());
        assert_eq!(*host.update_state.read().await, state);
    }
    *host.update_state.write().await = json!({"phase":"idle"});
    for _ in 0..2 {
        assert!(host
            .prepare_update()
            .await
            .unwrap_err()
            .message
            .contains("源码运行"));
        let state = host.update_state.read().await;
        assert_eq!(state["phase"], "error");
        assert_eq!(state["stage"], "checking");
        assert!(state["stageTimingsMs"]["checking"].is_u64());
    }
}

#[tokio::test]
async fn desktop_connection_reports_start_failure_and_can_retry() {
    let f = support::Fixture::new();
    let _mock = support::MockDesktop::start(&f).await;
    f.unpublish_runtime();
    let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    ServiceConfig {
        port: occupied.local_addr().unwrap().port(),
        bind: "127.0.0.1".into(),
        ..Default::default()
    }
    .save(&f.paths.data_home)
    .unwrap();
    let host = Host::open(f.paths.clone(), false).unwrap();
    let binaries = f.temp.path().join("unused-binaries");
    host.connect_desktop(binaries.clone()).await;
    wait_sync(&host, "error").await;
    assert_eq!(host.status().await.unwrap()["service"]["running"], false);
    drop(occupied);
    host.start().await.unwrap();
    host.stop().await.unwrap();
    host.start().await.unwrap();
    wait_sync(&host, "history-only").await;
    host.stop().await.unwrap();
}

#[tokio::test]
async fn browsers_cannot_read_or_rotate_desktop_pairing_key() {
    let f = support::Fixture::new();
    let _mock = support::MockDesktop::start(&f).await;
    let app = codex_mobile_bridge::http::Bridge::open(
        f.paths.clone(),
        ServiceConfig {
            port: 0,
            bind: "127.0.0.1".into(),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let server = app.start().await.unwrap();
    let host = Host::open(f.paths.clone(), false).unwrap();
    let key = host.pairing().await.unwrap()["accessKey"]
        .as_str()
        .unwrap()
        .to_string();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let url = format!("http://{}", server.address);
    let response = http
        .post(format!("{url}/api/login"))
        .json(&json!({"accessKey":key}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    for forwarded in [false, true] {
        for authenticated in [false, true] {
            for (method, path) in [
                (reqwest::Method::GET, "/api/pairing"),
                (reqwest::Method::POST, "/api/pairing/rotate"),
            ] {
                let mut request = http.request(method, format!("{url}{path}"));
                if forwarded {
                    request = request.header("x-forwarded-host", "phone.example");
                }
                if authenticated {
                    request = request.header("cookie", &cookie);
                }
                let response = request.send().await.unwrap();
                assert_eq!(response.status(), 410);
                let text = response.text().await.unwrap();
                assert!(!text.contains(&key));
                assert!(!text.contains("accessKey"));
            }
        }
    }
    let root = http
        .get(&url)
        .header("user-agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64)")
        .send()
        .await
        .unwrap();
    assert_eq!(root.status(), 200);
    assert!(!root.headers().contains_key("location"));
    for path in ["/pair", "/pair/"] {
        let response = http.get(format!("{url}{path}")).send().await.unwrap();
        assert_eq!(response.status(), 302);
        assert_eq!(response.headers()["location"], "/");
    }
    assert_eq!(host.pairing().await.unwrap()["accessKey"], key);
    assert_eq!(
        http.get(format!("{url}/api/session"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    server.stop().await.unwrap();
    assert_eq!(
        Host::open(f.paths.clone(), false)
            .unwrap()
            .pairing()
            .await
            .unwrap()["accessKey"],
        key
    );
}

#[tokio::test]
async fn host_distinguishes_http_from_sync_and_rotates_all_existing_sessions() {
    let f = support::Fixture::new();
    let mock = support::MockDesktop::start(&f).await;
    f.unpublish_runtime();
    mock.reject_init.store(true, Ordering::Release);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let config = ServiceConfig {
        port,
        bind: "127.0.0.1".into(),
        auto_start: false,
        public_url: "https://phone.example".into(),
    };
    config.save(&f.paths.data_home).unwrap();
    let host = Host::open(f.paths.clone(), false).unwrap();
    assert!(!host.auto_start().await);
    host.start().await.unwrap();
    wait_sync(&host, "history-only").await;
    let state = host.status().await.unwrap();
    assert_eq!(state["service"]["running"], true);
    assert!(state["sync"]["message"].as_str().unwrap().contains("历史"));
    assert!(mock
        .calls
        .lock()
        .await
        .iter()
        .all(|call| call["method"] == "initialize"));
    let key = host.pairing().await.unwrap()["accessKey"]
        .as_str()
        .unwrap()
        .to_string();
    let http = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}");
    let response = http
        .post(format!("{url}/api/login"))
        .json(&json!({"accessKey":key}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let cookie = response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let diagnostics = host.diagnostics().await.unwrap();
    assert!(!diagnostics.contains(&key));
    assert!(diagnostics.contains("历史"));
    let old = host.app().await.unwrap();
    let mut saved = config;
    saved.auto_start = true;
    host.save_config(saved).await.unwrap();
    assert!(Arc::ptr_eq(&old, &host.app().await.unwrap()));
    let rotated = host.rotate_key().await.unwrap();
    assert_ne!(rotated["accessKey"], key);
    assert_eq!(
        http.get(format!("{url}/api/projects"))
            .header("cookie", cookie)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    mock.reject_init.store(false, Ordering::Release);
    wait_sync(&host, "history-only").await;
    host.stop().await.unwrap();
    assert_eq!(host.status().await.unwrap()["sync"]["state"], "stopped");
    let reopened = Host::open(f.paths.clone(), false).unwrap();
    assert_eq!(
        reopened.pairing().await.unwrap()["accessKey"],
        rotated["accessKey"]
    );
}

#[tokio::test]
async fn paired_phone_can_request_one_background_update_without_github_token() {
    let f = support::Fixture::new();
    let _mock = support::MockDesktop::start(&f).await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    ServiceConfig {
        port,
        bind: "127.0.0.1".into(),
        ..Default::default()
    }
    .save(&f.paths.data_home)
    .unwrap();
    let host = Host::open(f.paths.clone(), true).unwrap();
    let mut requests = host.mobile_update_requests();
    host.start().await.unwrap();
    let http = reqwest::Client::new();
    let origin = format!("http://127.0.0.1:{port}");
    let update = format!("{origin}/api/update");
    let settings = format!("{origin}/api/update-settings");
    assert_eq!(http.post(&update).send().await.unwrap().status(), 401);
    let login = http
        .post(format!("{origin}/api/login"))
        .json(&json!({"accessKey":host.pairing().await.unwrap()["accessKey"]}))
        .send()
        .await
        .unwrap();
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap();
    assert_eq!(
        http.post(&update)
            .header("cookie", cookie)
            .header("origin", "https://other.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(!requests.has_changed().unwrap());
    for first_request in [true, false] {
        let accepted = http
            .post(&update)
            .header("origin", &origin)
            .header("cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(accepted.status(), 202);
        assert_eq!(requests.has_changed().unwrap(), first_request);
        assert!(*requests.borrow_and_update());
    }
    assert!(!requests.has_changed().unwrap());
    let response = http
        .get(&settings)
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let data: serde_json::Value = response.json().await.unwrap();
    assert_eq!(data["update"]["phase"], "queued");
    assert!(data.get("configured").is_none());
    assert!(data.get("source").is_none());
    assert_eq!(data["source"], serde_json::Value::Null);
    assert!(data.get("token").is_none());
    *host.update_state.write().await =
        json!({"phase":"current","message":"当前已是最新正式版本。"});
    host.finish_mobile_update();
    let data: serde_json::Value = http
        .get(&settings)
        .header("cookie", cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(data["update"]["phase"], "current");
    host.stop().await.unwrap();
}

#[tokio::test]
async fn history_stream_keeps_native_runtime_untouched_and_rotation_closes_it() {
    let f = support::Fixture::new();
    let mock = support::MockDesktop::start(&f).await;
    f.unpublish_runtime();
    mock.reject_init.store(true, Ordering::Release);
    let app = codex_mobile_bridge::http::Bridge::open(
        f.paths.clone(),
        ServiceConfig {
            port: 0,
            bind: "127.0.0.1".into(),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let server = app.start().await.unwrap();
    let key = app.auth.lock().await.key().to_string();
    let http = reqwest::Client::new();
    let url = format!("http://{}", server.address);
    let login = http
        .post(format!("{url}/api/login"))
        .json(&json!({"accessKey":key}))
        .send()
        .await
        .unwrap();
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let mut response = http
        .get(format!("{url}/api/threads/chat/events"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(4), async {
        while !String::from_utf8_lossy(&bytes).contains("event: snapshot") {
            bytes.extend(response.chunk().await.unwrap().unwrap());
        }
    })
    .await
    .unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("event: snapshot"));
    assert!(text.contains("\"connection\":\"history\""));
    assert!(mock.calls.lock().await.is_empty());
    app.rotate_local().await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while response.chunk().await.unwrap().is_some() {}
    })
    .await
    .unwrap();
    server.stop().await.unwrap();
}
