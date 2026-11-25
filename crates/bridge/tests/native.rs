//! 显式启用的官方已登录桌面验收，不在常规测试中向用户聊天发消息。
use codex_mobile_bridge::{config::ServiceConfig, http::Bridge, paths::Paths};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

#[tokio::test]
#[ignore = "需要已登录官方 Windows 桌面，仅验证 Host 连接与停止，不发送消息"]
async fn official_native_host_connect_and_stop() {
    let artifact = PathBuf::from(std::env::var("MOBILE_ACCEPTANCE_DIRECTORY").unwrap());
    let data = artifact.join(format!("host-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data).unwrap();
    let paths = Paths {
        codex_home: dirs::home_dir().unwrap().join(".codex"),
        runtime_home: data.join("runtime"),
        web_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/mobile"),
        data_home: data.clone(),
    };
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    ServiceConfig {
        port,
        bind: "127.0.0.1".into(),
        ..Default::default()
    }
    .save(&data)
    .unwrap();
    let host = codex_mobile_bridge::host::Host::open(paths, false).unwrap();
    host.connect_desktop(data.join("不存在的启动组件")).await;
    let status = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = host.status().await.unwrap();
            if status["sync"]["state"] != "connecting" {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status["sync"]["state"], "connected", "{status}");
    assert_eq!(status["runtime"]["ready"], true);
    assert_eq!(
        host.app().await.unwrap().session.capabilities()["native"],
        true
    );
    host.stop().await.unwrap();
    assert_eq!(host.status().await.unwrap()["sync"]["state"], "stopped");
    std::fs::write(
        artifact.join("native-host-result.json"),
        json!({"connected":true,"noDesktopLaunchRequired":true,"stoppedCleanly":true}).to_string(),
    )
    .unwrap();
}

async fn assert_native_snapshot(mut response: reqwest::Response, thread: &str) {
    assert!(response.status().is_success(), "{}", response.status());
    let snapshot: Value = tokio::time::timeout(Duration::from_secs(10), async {
        let mut frame = Vec::new();
        loop {
            let chunk = response.chunk().await.unwrap().expect("SSE 已断开");
            frame.extend_from_slice(&chunk);
            if let Some(end) = frame.windows(2).position(|bytes| bytes == b"\n\n") {
                let event = std::str::from_utf8(&frame[..end]).unwrap();
                assert!(event.starts_with("event: snapshot\n"));
                let data = event
                    .lines()
                    .find_map(|line| line.strip_prefix("data: "))
                    .unwrap();
                break serde_json::from_str(data).unwrap();
            }
        }
    })
    .await
    .expect("未收到原生 SSE 初始快照");
    assert_eq!(snapshot["thread"]["id"], thread);
    assert_eq!(snapshot["syncMode"], "desktop-poll");
    assert_eq!(snapshot["connection"], "live");
}

#[tokio::test]
#[ignore = "需要已登录官方 Windows 桌面及用户授权的专用测试聊天"]
async fn official_native_http_send_and_reconnect() {
    let thread = std::env::var("MOBILE_ACCEPTANCE_THREAD").expect("明确授权的测试聊天 ID");
    let artifact = PathBuf::from(std::env::var("MOBILE_ACCEPTANCE_DIRECTORY").unwrap());
    let data = artifact.join(format!("service-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data).unwrap();
    let paths = Paths {
        codex_home: dirs::home_dir().unwrap().join(".codex"),
        runtime_home: data.join("runtime"),
        web_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/mobile"),
        data_home: data,
    };
    let app = Bridge::open(
        paths,
        ServiceConfig {
            port: 0,
            bind: "127.0.0.1".into(),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    app.desktop.probe().await.unwrap();
    assert_eq!(app.session.capabilities()["native"], true);
    assert_eq!(app.session.capabilities()["interrupt"], false);
    let server = app.start().await.unwrap();
    let url = format!("http://{}", server.address);
    let http = reqwest::Client::new();
    let key = app.pairing().await["accessKey"]
        .as_str()
        .unwrap()
        .to_owned();
    let login = http
        .post(format!("{url}/api/login"))
        .json(&json!({"accessKey":key}))
        .send()
        .await
        .unwrap();
    assert!(login.status().is_success());
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let initial = http
        .get(format!("{url}/api/threads/{thread}/events"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_native_snapshot(initial, &thread).await;
    if std::env::var_os("MOBILE_ACCEPTANCE_PREVIEW").is_some() {
        std::fs::write(
            artifact.join("preview.json"),
            json!({"url":url,"key":key,"threadId":thread}).to_string(),
        )
        .unwrap();
        println!("验收预览服务已就绪，等待结束标记。");
        while !artifact.join("preview-stop").exists() {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        server.stop().await.unwrap();
        return;
    }
    if std::env::var_os("MOBILE_ACCEPTANCE_CREATE").is_some() {
        let project_path = std::env::var("MOBILE_ACCEPTANCE_PROJECT")
            .expect("明确授权的测试项目路径")
            .replace('\\', "/");
        let projects: Value = http
            .get(format!("{url}/api/projects?threadId={thread}"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let project = projects["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| {
                p["path"]
                    .as_str()
                    .is_some_and(|path| path.replace('\\', "/").eq_ignore_ascii_case(&project_path))
            })
            .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let input = json!({"sourceThreadId":thread,"projectId":project["projectId"],"requestId":id,"title":"Mobile 新建验收",
            "prompt":"只回复“手机新建验收通过”。不要读取或修改文件，不运行命令，不调用工具，不创建其他聊天，不启动子智能体。"});
        let mut created = String::new();
        for _ in 0..2 {
            let response = http
                .post(format!("{url}/api/threads"))
                .header("cookie", &cookie)
                .header("x-codex-mobile-contract", "2")
                .json(&input)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let value: Value = response.json().await.unwrap();
            std::fs::write(artifact.join("http-create-receipt.json"), value.to_string()).unwrap();
            assert!(status.is_success(), "{status}: {value}");
            let id = value["threadId"].as_str().unwrap();
            if created.is_empty() {
                created = id.to_owned();
            } else {
                assert_eq!(created, id);
            }
            assert_eq!(value["firstTurnAccepted"], false);
        }
        let mut done = false;
        for _ in 0..45 {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let mut stream = http
                .get(format!("{url}/api/threads/{created}/events"))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
            let text =
                String::from_utf8_lossy(&stream.chunk().await.unwrap().unwrap()).into_owned();
            if text.contains("手机新建验收通过") && text.contains("\"firstTurnPending\":false")
            {
                done = true;
                break;
            }
        }
        assert!(done, "正式 ID 的首轮未通过手机接口确认");
        std::fs::write(artifact.join("http-create-result.json"),json!({"threadId":created,"createdOnce":true,"firstTurnObserved":true,"officialReplyRead":true}).to_string()).unwrap();
        server.stop().await.unwrap();
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let marker = format!("HTTP手机通道验收{}", &id[..8]);
    let input = json!({"requestId":id,"mode":"now","prompt":format!("只回复“{marker}通过”。不要读取或修改文件，不运行命令，不调用工具，不新建聊天，不启动子智能体。")});
    for _ in 0..2 {
        let response = http
            .post(format!("{url}/api/threads/{thread}/messages"))
            .header("cookie", &cookie)
            .header("x-codex-mobile-contract", "2")
            .json(&input)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let value: Value = response.json().await.unwrap();
        assert!(status.is_success(), "{status}: {value}");
        assert_eq!(value["accepted"], true);
        assert_eq!(value["queued"], false);
    }
    let mut seen = false;
    for _ in 0..45 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let response = http
            .get(format!("{url}/api/threads/{thread}/history"))
            .header("cookie", &cookie)
            .send()
            .await
            .unwrap();
        let value: Value = response.json().await.unwrap();
        let matching = value["turns"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|t| t.to_string().contains(&marker))
            .collect::<Vec<_>>();
        if matching
            .iter()
            .any(|t| t["status"] == "completed" && t.to_string().contains(&format!("{marker}通过")))
        {
            assert_eq!(matching.len(), 1, "同 requestId 不重复发送");
            seen = true;
            break;
        }
    }
    assert!(seen, "官方首轮响应未在手机历史接口读取到");
    let reconnect = http
        .get(format!("{url}/api/threads/{thread}/events"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_native_snapshot(reconnect, &thread).await;
    server.stop().await.unwrap();
    std::fs::write(artifact.join("http-send-result.json"), json!({"send":true,"sameRequestNoDuplicate":true,"officialReplyRead":true,"reconnect":true}).to_string()).unwrap();
}
