//! 显式验收现有官方桌面代理；不启动 CLI、不操作窗口、不批准操作。
use crate::{
    config::ServiceConfig,
    http::{Bridge, Server},
    paths::Paths,
    runtime::read_runtime,
};
use serde_json::{json, Value};
use std::{os::windows::io::AsRawHandle, path::PathBuf, time::Duration};

async fn request(
    server: &Server,
    client: &reqwest::Client,
    cookie: &str,
    method: reqwest::Method,
    suffix: &str,
    body: Value,
) -> Value {
    let thread = std::env::var("MOBILE_ACCEPTANCE_THREAD").unwrap();
    let response = client
        .request(
            method,
            format!("http://{}/api/threads/{thread}/{suffix}", server.address),
        )
        .header("cookie", cookie)
        .header("x-codex-mobile-contract", "2")
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let result: Value = response.json().await.unwrap();
    assert!(status.is_success(), "{suffix} {status}: {result}");
    result
}

#[tokio::test]
#[ignore = "需要明确验收聊天、隔离官方桌面管道与代理；会发送合成文本并停止该合成轮次"]
async fn official_control_http_queue_interrupt_and_resume() {
    let artifact = PathBuf::from(std::env::var_os("MOBILE_ACCEPTANCE_DIRECTORY").unwrap());
    let thread = std::env::var("MOBILE_ACCEPTANCE_THREAD").unwrap();
    assert!(uuid::Uuid::parse_str(&thread).is_ok());
    let runtime_dir = artifact.join("broker-runtime");
    let runtime = read_runtime(&runtime_dir).unwrap().unwrap();
    let pipe_path = PathBuf::from(std::env::var_os("MOBILE_ACCEPTANCE_PIPE").unwrap());
    let pipe = tokio::net::windows::named_pipe::ClientOptions::new()
        .open(&pipe_path)
        .unwrap();
    let mut pid = 0;
    assert_ne!(
        unsafe {
            windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId(
                pipe.as_raw_handle(),
                &mut pid,
            )
        },
        0
    );
    assert_eq!(pid, runtime.desktop.pid);
    drop(pipe);
    let data = artifact.join(format!("controls-http-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&data).unwrap();
    let app = Bridge::open(
        Paths {
            codex_home: dirs::home_dir().unwrap().join(".codex"),
            runtime_home: runtime_dir,
            data_home: data,
            web_root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/mobile"),
        },
        ServiceConfig {
            bind: "127.0.0.1".into(),
            port: 0,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    // 走正式发现及同实例绑定，不能用测试门禁或手动注入连接掩盖启动问题。
    let control = app.session.connect().await.unwrap();
    assert_eq!(control.runtime_id(), Some(runtime.instance_id));
    assert!(control.native_tools().is_some());
    assert_eq!(app.session.capabilities()["queue"], true);
    let server = app.start().await.unwrap();
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let key = server.app.auth.lock().await.key().to_owned();
    let login = http
        .post(format!("http://{}/api/login", server.address))
        .json(&json!({"accessKey":key}))
        .send()
        .await
        .unwrap();
    assert_eq!(login.status(), 200);
    let cookie = login.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let mut events = server.app.desktop.subscribe(&thread);
    let initial = server.app.desktop.snapshot(&thread).await.unwrap();
    assert_eq!(initial["thread"]["title"], "Mobile 连接验收");
    assert!(
        matches!(
            initial["thread"]["status"]["type"].as_str(),
            Some("idle" | "notLoaded")
        ) && initial["turns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|turn| turn["status"] != "inProgress"),
        "{initial}"
    );
    request(&server,&http,&cookie,reqwest::Method::POST,"messages",json!({"prompt":"这是授权的停止与队列接口验收。请从1连续列到5000，每行写编号及固定文本“移动端合成验收”，直到收到停止。不要运行命令、读取或修改文件，不调用工具，不创建其他聊天，不启动子智能体。","requestId":format!("control-http-{}",uuid::Uuid::new_v4())})).await;
    let active = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let snapshot = events.borrow_and_update().snapshot.clone();
            if let Some(snapshot) = snapshot {
                if let Some(turn) = snapshot["turns"]
                    .as_array()
                    .and_then(|turns| turns.last())
                    .filter(|turn| turn["status"] == "inProgress")
                {
                    break turn["id"].as_str().unwrap().to_owned();
                }
            }
            events.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let queue = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::GET,
        "queue",
        json!({}),
    )
    .await;
    assert!(queue["messages"].as_array().unwrap().is_empty());
    for marker in ["甲", "乙"] {
        request(&server,&http,&cookie,reqwest::Method::POST,"messages",json!({"prompt":format!("仅回复队列验收{marker}，不要调用工具。"),"requestId":format!("queue-{}",uuid::Uuid::new_v4()),"mode":"queue"})).await;
    }
    let queue = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::GET,
        "queue",
        json!({}),
    )
    .await;
    assert_eq!(queue["messages"].as_array().unwrap().len(), 2, "{queue}");
    let first = queue["messages"][0]["id"].clone();
    let second = queue["messages"][1]["id"].clone();
    let busy_start = control
        .rpc(
            "thread/queue/start",
            json!({"threadId":thread,"queuedSubmissionId":first}),
        )
        .await;
    assert!(busy_start.is_err(), "运行中的任务不得被队列立即执行替换");
    let edited=request(&server,&http,&cookie,reqwest::Method::POST,"queue",json!({"action":"edit","revision":queue["revision"],"messageId":first,"prompt":"仅回复队列验收修改甲，不调用工具。"})).await;
    assert_eq!(
        edited["messages"][0]["prompt"],
        "仅回复队列验收修改甲，不调用工具。"
    );
    let reordered = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::POST,
        "queue",
        json!({"action":"reorder","revision":edited["revision"],"messageIds":[second,first]}),
    )
    .await;
    assert_eq!(reordered["messages"][0]["id"], second);
    let deleted = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::POST,
        "queue",
        json!({"action":"delete","revision":reordered["revision"],"messageId":second}),
    )
    .await;
    let empty = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::POST,
        "queue",
        json!({"action":"delete","revision":deleted["revision"],"messageId":first}),
    )
    .await;
    assert!(empty["messages"].as_array().unwrap().is_empty());
    let stopped = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::POST,
        "interrupt",
        json!({"turnId":active}),
    )
    .await;
    let interrupted = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let snapshot = events.borrow_and_update().snapshot.clone();
            if let Some(snapshot) = snapshot {
                if snapshot["turns"].as_array().is_some_and(|turns| {
                    turns
                        .iter()
                        .any(|turn| turn["id"] == active && turn["status"] == "interrupted")
                }) {
                    break snapshot;
                }
            }
            events.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let resumed = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::POST,
        "resume",
        json!({"turnId":active}),
    )
    .await;
    let resumed_turn = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let snapshot = events.borrow_and_update().snapshot.clone();
            if let Some(snapshot) = snapshot {
                if let Some(turn) = snapshot["turns"]
                    .as_array()
                    .and_then(|turns| turns.last())
                    .filter(|turn| turn["id"] != active && turn["status"] == "inProgress")
                {
                    break turn["id"].as_str().unwrap().to_owned();
                }
            }
            events.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    request(&server,&http,&cookie,reqwest::Method::POST,"messages",json!({"prompt":"仅回复“队列手动执行完成”，不要调用工具。","requestId":format!("queue-manual-{}",uuid::Uuid::new_v4()),"mode":"queue"})).await;
    request(
        &server,
        &http,
        &cookie,
        reqwest::Method::POST,
        "interrupt",
        json!({"turnId":resumed_turn}),
    )
    .await;
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let snapshot = events.borrow_and_update().snapshot.clone();
            if snapshot.as_ref().is_some_and(|snapshot| {
                snapshot["turns"].as_array().is_some_and(|turns| {
                    turns
                        .iter()
                        .any(|turn| turn["id"] == resumed_turn && turn["status"] == "interrupted")
                })
            }) {
                break;
            }
            events.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let remaining = request(
        &server,
        &http,
        &cookie,
        reqwest::Method::GET,
        "queue",
        json!({}),
    )
    .await;
    assert_eq!(remaining["messages"].as_array().unwrap().len(), 1);
    let executed = request(&server, &http, &cookie, reqwest::Method::POST, "queue", json!({"action":"send","revision":remaining["revision"],"messageId":remaining["messages"][0]["id"]})).await;
    assert!(executed["messages"].as_array().unwrap().is_empty());
    // 手机断开只销毁自己的连接；官方刚开始的队列轮次应继续，重连恢复同一轮。
    control.close();
    let queue_turn = tokio::time::timeout(Duration::from_secs(45), async {
        loop {
            let view = events.borrow_and_update().clone();
            std::fs::write(artifact.join("official-reconnect-view.json"), serde_json::to_vec_pretty(&json!({"previousOwner":control.id,"owner":view.owner,"error":view.error,"snapshot":view.snapshot})).unwrap()).unwrap();
            if view.owner.as_deref().is_some_and(|owner| owner != control.id) {
                if let Some(turn) = view
                    .snapshot
                    .as_ref()
                    .and_then(|snapshot| snapshot["turns"].as_array())
                    .and_then(|turns| turns.last())
                    .filter(|turn| turn["id"] != resumed_turn && turn["status"] == "completed")
                {
                    assert!(turn["items"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|item| item["type"] == "agentMessage"
                            && item["text"]
                                .as_str()
                                .is_some_and(|text| text.contains("队列手动执行完成"))));
                    break turn["id"].clone();
                }
            }
            events.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    std::fs::write(artifact.join("official-control-http-result.json"),serde_json::to_vec_pretty(&json!({"threadId":thread,"productionSessionDiscovery":true,"turnId":active,"queueAddEditReorderDelete":true,"busyQueueStartRejected":true,"stop":stopped,"interrupted":interrupted["turns"].as_array().unwrap().last().unwrap()["status"],"resume":resumed,"resumedTurnId":resumed_turn,"resumedTurnStopped":true,"queueManualStartCompleted":queue_turn,"mobileReconnectPreservesOfficialTurn":true})).unwrap()).unwrap();
    drop(events);
    server.stop().await.unwrap();
}
