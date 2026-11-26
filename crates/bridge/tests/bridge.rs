mod support;
use codex_mobile_bridge::{
    auth::{check_origin, pairing_request, Auth},
    config::ServiceConfig,
    desktop::Desktop,
    history::History,
    http::Bridge,
};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use support::{Fixture, MockDesktop};

fn desktop(f: &Fixture) -> Arc<Desktop> {
    Desktop::new(
        f.paths.clone(),
        Arc::new(History::new(f.paths.codex_home.clone())),
    )
}

#[tokio::test]
async fn history_mode_rejects_all_native_mutations_and_old_clients_without_rpc() {
    let f = Fixture::new();
    let mock = MockDesktop::start(&f).await;
    let server = server(&f).await;
    let http = reqwest::Client::new();
    let cookie = login(&server, &http).await;
    let url = format!("http://{}", server.address);
    let state: Value = http
        .get(format!("{url}/api/session"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(state["capabilities"]
        .as_object()
        .unwrap()
        .values()
        .all(|value| *value == false));
    for (method, route, body) in [
        (
            "POST",
            "threads",
            json!({"prompt":"draft","requestId":"create"}),
        ),
        (
            "POST",
            "threads/chat/messages",
            json!({"prompt":"draft","requestId":"send"}),
        ),
        (
            "POST",
            "threads/chat/queue",
            json!({"action":"send","messageId":"row","revision":"old"}),
        ),
        ("POST", "threads/chat/settings", json!({"model":"model"})),
        ("POST", "threads/chat/interrupt", json!({"turnId":"turn"})),
        ("POST", "threads/chat/resume", json!({"turnId":"turn"})),
        (
            "POST",
            "threads/chat/interactions/request",
            json!({"answer":"yes"}),
        ),
        ("PATCH", "threads/chat/goal", json!({"objective":"goal"})),
        ("DELETE", "threads/chat/goal", json!({})),
        ("POST", "threads/chat/open", json!({})),
    ] {
        let old = http
            .request(method.parse().unwrap(), format!("{url}/api/{route}"))
            .header("cookie", &cookie)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(old.status(), 426, "{route}");
        let current = http
            .request(method.parse().unwrap(), format!("{url}/api/{route}"))
            .header("cookie", &cookie)
            .header("x-codex-mobile-contract", "2")
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(current.status(), 503, "{route}");
        assert_eq!(
            current.json::<Value>().await.unwrap()["outcome"],
            "not-dispatched"
        );
    }
    let history: Value = http
        .get(format!("{url}/api/threads/chat/history"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(history["thread"]["id"], "chat");
    assert_eq!(history["capabilities"]["send"], false);
    assert!(mock.calls.lock().await.is_empty());
    server.stop().await.unwrap();
}

#[test]
fn sessions_survive_restart_and_revoke_individually() {
    let f = Fixture::new();
    let mut auth = Auth::open(f.paths.data_home.clone()).unwrap();
    let key = auth.key().to_string();
    let one = auth.login(&key).unwrap();
    let two = auth.login(&key).unwrap();
    let headers = |token: &str| {
        let mut h = axum::http::HeaderMap::new();
        h.insert("cookie", format!("cm_session={token}").parse().unwrap());
        h
    };
    let mut reopened = Auth::open(f.paths.data_home.clone()).unwrap();
    let (owner, _) = reopened.session(&headers(&one)).unwrap();
    assert!(reopened.session(&headers(&two)).is_ok());
    reopened.logout(&owner).unwrap();
    assert_eq!(reopened.session(&headers(&one)).unwrap_err().status, 401);
    let (owner, _) = reopened.session(&headers(&two)).unwrap();
    let rotated = reopened.rotate(&owner).unwrap();
    assert_ne!(rotated, key);
    assert!(reopened.login(&key).is_err());
    assert!(reopened.session(&headers(&two)).is_ok());
    let disk = std::fs::read_to_string(f.paths.data_home.join("sessions.json")).unwrap();
    assert!(!disk.contains(&two));
    assert!(!disk.contains(&rotated));
}

#[test]
fn pairing_requires_private_direct_request_and_same_origin() {
    let mut h = axum::http::HeaderMap::new();
    h.insert("host", "127.0.0.1:4319".parse().unwrap());
    assert!(pairing_request(&h, "127.0.0.1".parse().unwrap()));
    h.insert("x-forwarded-for", "192.168.1.1".parse().unwrap());
    assert!(!pairing_request(&h, "127.0.0.1".parse().unwrap()));
    h.remove("x-forwarded-for");
    h.insert("origin", "http://evil.example".parse().unwrap());
    assert_eq!(check_origin(&h).unwrap_err().status, 403);
    h.insert("origin", "http://127.0.0.1:4319".parse().unwrap());
    assert!(check_origin(&h).is_ok());
}

#[tokio::test]
async fn rejected_initialization_and_wrong_home_never_load_or_send_a_chat() {
    let f = Fixture::new();
    let mock = MockDesktop::start(&f).await;
    mock.reject_init.store(true, Ordering::Release);
    let client = desktop(&f);
    assert!(client
        .session
        .connect()
        .await
        .err()
        .unwrap()
        .message
        .contains("fixture initialize rejected"));
    assert_eq!(mock.count("codexMobile/attach").await, 1);
    assert_eq!(mock.count("initialize").await, 0);
    assert_eq!(mock.calls.lock().await.len(), 1);
    mock.reject_init.store(false, Ordering::Release);
    mock.wrong_home.store(true, Ordering::Release);
    assert!(client
        .session
        .connect()
        .await
        .err()
        .unwrap()
        .message
        .contains("其他 Codex 目录"));
    mock.wrong_home.store(false, Ordering::Release);
    client.session.connect().await.unwrap();
    let one = client.session.connect().await.unwrap();
    let two = client.session.connect().await.unwrap();
    assert!(Arc::ptr_eq(&one, &two));
    assert_eq!(mock.count("codexMobile/attach").await, 3);
    assert_eq!(mock.count("initialize").await, 0);
    assert!(client.last_native_error.read().await.is_none());
    assert_eq!(mock.count("thread/resume").await, 0);
    assert_eq!(mock.count("turn/start").await, 0);
    assert_eq!(mock.count("mcpServer/tool/call").await, 0);
    client.close().await;
}

async fn server(f: &Fixture) -> codex_mobile_bridge::http::Server {
    Bridge::open(
        f.paths.clone(),
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
async fn login(server: &codex_mobile_bridge::http::Server, client: &reqwest::Client) -> String {
    let key = server.app.auth.lock().await.key().to_string();
    let response = client
        .post(format!("http://{}/api/login", server.address))
        .json(&json!({"accessKey":key}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}

#[tokio::test]
async fn history_refresh_reports_schema_failure_without_native_recovery() {
    let f = Fixture::new();
    let mock = MockDesktop::start(&f).await;
    let server = server(&f).await;
    let client = reqwest::Client::new();
    let cookie = login(&server, &client).await;
    let mut response = client
        .get(format!("http://{}/api/threads/chat/events", server.address))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let initial = response.chunk().await.unwrap().unwrap();
    assert!(String::from_utf8_lossy(&initial).contains("event: snapshot"));
    rusqlite::Connection::open(f.paths.codex_home.join("thread_history_1.sqlite"))
        .unwrap()
        .execute_batch("DROP TABLE thread_turns")
        .unwrap();
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(8), async {
        while let Some(chunk) = response.chunk().await.unwrap() {
            bytes.extend(chunk);
        }
    })
    .await
    .unwrap();
    assert!(String::from_utf8_lossy(&bytes).contains("event: desktop-error"));
    assert!(mock.calls.lock().await.is_empty());
    server.stop().await.unwrap();
}

#[tokio::test]
async fn stop_cancels_a_download_when_the_client_stops_reading() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let f = Fixture::new();
    let size = 64 * 1024 * 1024;
    std::fs::File::create(f.paths.web_root.join("large.bin"))
        .unwrap()
        .set_len(size)
        .unwrap();
    let server = server(&f).await;
    let mut socket = tokio::net::TcpStream::connect(server.address)
        .await
        .unwrap();
    socket
        .write_all(b"GET /large.bin HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut initial = [0; 4096];
    let read = socket.read(&mut initial).await.unwrap();
    assert!(String::from_utf8_lossy(&initial[..read]).starts_with("HTTP/1.1 200"));
    tokio::time::timeout(Duration::from_secs(40), server.stop())
        .await
        .unwrap()
        .unwrap();
    let mut remaining = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(3), socket.read_to_end(&mut remaining))
        .await
        .unwrap();
    assert!((remaining.len() as u64) < size);
}

#[tokio::test]
async fn update_settings_report_running_versions_without_waiting_for_app_server() {
    let f = Fixture::new();
    let mock = MockDesktop::start(&f).await;
    let mut agent = mock.user_agent.lock().await;
    *agent =
        "Codex Desktop/0.171.0-alpha.13 (Mac OS 27.0.1; arm64) dumb (codex-mobile; 0.3.12)".into();
    let server = server(&f).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/api/update-settings", server.address);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
    let cookie = login(&server, &client).await;
    assert_eq!(mock.count("initialize").await, 0);
    let read = || async {
        let response = client
            .get(&url)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        response.json::<Value>().await.unwrap()
    };
    // 握手尚未返回时也能查看 Mobile 版本及升级设置。
    let settings = tokio::time::timeout(Duration::from_secs(1), read())
        .await
        .unwrap();
    assert_eq!(settings["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        settings["appServer"],
        json!({"connected":false,"native":false,"controls":false,"version":null})
    );
    drop(agent);
    let connection = server.app.session.connect().await.unwrap();
    let settings = read().await;
    assert_eq!(
        settings["appServer"],
        json!({"connected":true,"native":false,"controls":false,"version":"0.171.0-alpha.13"})
    );
    assert!(settings.get("token").is_none());
    assert!(settings.get("configured").is_none());
    assert!(settings.get("source").is_none());
    // 旧网页不能再保存或删除凭据；拒绝后正常状态读取仍可用。
    for method in [reqwest::Method::POST, reqwest::Method::DELETE] {
        let response = client
            .request(method, &url)
            .header("cookie", &cookie)
            .header("x-codex-mobile-contract", "2")
            .json(&json!({"token":"synthetic-unused"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 405);
        assert_eq!(read().await["appServer"], settings["appServer"]);
    }
    assert!(!f.paths.data_home.join("github-token").exists());

    mock.reject_init.store(true, Ordering::Release);
    connection.close();
    assert_eq!(
        read().await["appServer"],
        json!({"connected":false,"native":false,"controls":false,"version":null})
    );
    *mock.user_agent.lock().await = "codex_cli_rs/0.172.0 (Mac OS 27.0.1; arm64)".into();
    mock.reject_init.store(false, Ordering::Release);
    assert!(server.app.session.connect().await.is_err());
    assert_eq!(
        read().await["appServer"],
        json!({"connected":false,"native":false,"controls":false,"version":null})
    );
    server.stop().await.unwrap();
}

#[test]
fn browser_context_has_no_private_pipe_fallback() {
    let source = include_str!("../src/context.rs");
    for private_entrypoint in ["open_pipe", "codex-browser-use", "getTabs"] {
        assert!(
            !source.contains(private_entrypoint),
            "browser context must use the official MCP channel: {private_entrypoint}"
        );
    }
}

#[tokio::test]
async fn file_preview_enforces_chat_scope_streams_ranges_and_blocks_documents() {
    let f = Fixture::new();
    let workspace = f.temp.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let file = workspace.join("骑行 演示.html");
    let html = "<!doctype html><title>骑行</title><button>暂停</button>";
    std::fs::write(&file, html).unwrap();
    std::fs::write(workspace.join("not-linked.html"), html).unwrap();
    std::fs::write(workspace.join("script.js"), "test").unwrap();
    std::fs::write(workspace.join("clip.wav"), b"0123456789").unwrap();
    std::fs::write(workspace.join("paper.tex"), "source").unwrap();
    for name in ["paper.pdf", "report.docx", "table.xlsx", "slides.pptx"] {
        std::fs::write(workspace.join(name), b"document").unwrap();
    }
    let output = f
        .paths
        .codex_home
        .join("visualizations/2026/11/19/chat/demo.html");
    std::fs::create_dir_all(output.parent().unwrap()).unwrap();
    std::fs::write(&output, html).unwrap();
    let output = output.to_str().unwrap();
    let other_output = f.paths.codex_home.join("attachments/other/note.txt");
    std::fs::create_dir_all(other_output.parent().unwrap()).unwrap();
    std::fs::write(&other_output, "other").unwrap();
    let other_output = other_output.to_str().unwrap();
    std::fs::write(f.temp.path().join("outside.html"), html).unwrap();
    std::fs::File::create(workspace.join("large.html"))
        .unwrap()
        .set_len(5 * 1024 * 1024 + 1)
        .unwrap();
    let absolute = file.to_string_lossy().replace('\\', "/");
    let absolute = if cfg!(windows) {
        format!("/{absolute}")
    } else {
        absolute
    };
    let absolute_pdf = workspace.join("paper.pdf");
    let absolute_pdf = absolute_pdf.to_str().unwrap();
    let text = format!("[演示](<{absolute}>) [相对](骑行%20演示.html) [外部](../outside.html) [缺失](missing.html) [代码](script.js#L2) [过大](large.html) [音频](clip.wav) [源码](paper.tex) [PDF](paper.pdf) [文档](report.docx) [表格](table.xlsx) [演示](slides.pptx) [绝对](<{absolute_pdf}>) [其他](<{other_output}>) visualize{}", json!({"path":output}));
    let state = rusqlite::Connection::open(f.paths.codex_home.join("state_5.sqlite")).unwrap();
    state
        .execute(
            "UPDATE threads SET cwd=? WHERE id='chat'",
            [workspace.to_str().unwrap()],
        )
        .unwrap();
    let history =
        rusqlite::Connection::open(f.paths.codex_home.join("thread_history_1.sqlite")).unwrap();
    history
        .execute(
            "INSERT INTO thread_items VALUES ('chat','turn-3','preview',32,?,'agentMessage',33)",
            [json!({"id":"preview","type":"agentMessage","text":text}).to_string()],
        )
        .unwrap();
    let server = server(&f).await;
    let client = reqwest::Client::new();
    let preview = |chat: &str, path: &str| {
        client
            .get(format!(
                "http://{}/api/threads/{chat}/attachment",
                server.address
            ))
            .query(&[("itemId", "preview"), ("preview", "file"), ("path", path)])
    };
    assert_eq!(
        preview("chat", &absolute).send().await.unwrap().status(),
        401
    );
    let cookie = login(&server, &client).await;
    for path in [absolute.as_str(), "骑行%20演示.html", output] {
        let response = preview("chat", path)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "private, no-store");
        assert_eq!(
            response.headers()["content-type"],
            "text/plain; charset=utf-8"
        );
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.text().await.unwrap(), html);
    }
    for (path, expected) in [("script.js#L2", "test"), ("paper.tex", "source")] {
        let response = preview("chat", path)
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.text().await.unwrap(), expected);
    }
    for path in ["paper.pdf", "report.docx", "table.xlsx", "slides.pptx"] {
        assert_eq!(
            preview("chat", path)
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            415
        );
        let response = preview("chat", path)
            .query(&[("download", "1")])
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 415);
        assert!(!response.headers().contains_key("content-disposition"));
    }
    let legacy = client
        .get(format!(
            "http://{}/api/threads/chat/attachment",
            server.address
        ))
        .query(&[("itemId", "preview"), ("path", absolute_pdf)])
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(legacy.status(), 415);
    let part = preview("chat", "clip.wav")
        .header("cookie", &cookie)
        .header("range", "bytes=2-5")
        .send()
        .await
        .unwrap();
    assert_eq!(part.status(), 206);
    assert_eq!(part.headers()["content-range"], "bytes 2-5/10");
    assert_eq!(part.text().await.unwrap(), "2345");
    let head = client
        .head(preview("chat", "clip.wav").build().unwrap().url().clone())
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"], "10");
    assert!(head.bytes().await.unwrap().is_empty());
    for (chat, path, status) in [
        ("other", absolute.as_str(), 403),
        ("chat", "not-linked.html", 403),
        ("chat", "../outside.html", 403),
        ("chat", "missing.html", 404),
        ("chat", other_output, 403),
        ("chat", "large.html", 413),
    ] {
        assert_eq!(
            preview(chat, path)
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap()
                .status(),
            status,
            "{path}"
        );
    }
    server.stop().await.unwrap();
}

#[test]
fn history_pages_are_read_only_and_keep_full_items_available() {
    let f = Fixture::new();
    let history = History::new(f.paths.codex_home.clone());
    let first = history.page("chat", None, 2).unwrap();
    assert_eq!(first["turns"][0]["id"], "turn-2");
    assert_eq!(first["turns"][1]["items"][0]["detailVersion"], "history:32");
    assert!(first.to_string().len() < 2500);
    let second = history
        .page("chat", first["nextCursor"].as_str(), 2)
        .unwrap();
    assert_eq!(second["turns"][0]["id"], "turn-1");
    assert!(second["nextCursor"].is_null());
    assert!(history
        .page("other", first["nextCursor"].as_str(), 2)
        .is_err());
    assert_eq!(
        history.item("chat", "command-3").unwrap().unwrap()["aggregatedOutput"],
        "完整输出🙂\n".repeat(1000)
    );
    assert!(history
        .state()
        .unwrap()
        .execute_batch("CREATE TABLE forbidden(id TEXT)")
        .is_err());
}

#[tokio::test]
async fn session_ping_requires_authentication_but_does_not_read_codex_state() {
    let f = Fixture::new();
    let server = server(&f).await;
    let client = reqwest::Client::new();
    let cookie = login(&server, &client).await;
    let url = format!("http://{}/api/session", server.address);
    assert_eq!(
        client
            .get(format!("{url}?ping=1"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let legacy: Value = client
        .get(&url)
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(legacy["defaultThreadId"], "chat");
    std::fs::remove_file(f.paths.codex_home.join("state_5.sqlite")).unwrap();
    let ping = client
        .get(format!("{url}?ping=1"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(ping.status(), 204);
    assert!(ping.bytes().await.unwrap().is_empty());
}
