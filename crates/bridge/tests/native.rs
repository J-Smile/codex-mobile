//! 显式启用的官方已登录桌面验收，不在常规测试中向用户聊天发消息。
use codex_mobile_bridge::{
    config::ServiceConfig,
    http::{Bridge, Server},
    paths::Paths,
};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    time::Duration,
};

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

async fn assert_native_snapshot(mut response: reqwest::Response, thread: &str) -> Value {
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
    snapshot
}

fn completed_answer(turn: &Value, expected: &str) -> bool {
    turn["status"] == "completed"
        && turn["items"].as_array().is_some_and(|items| {
            items.iter().any(|item| {
                item["type"] == "agentMessage"
                    && item["text"]
                        .as_str()
                        .is_some_and(|text| text.contains(expected))
            })
        })
}

#[test]
fn completed_answer_requires_model_text_not_input_tools_or_unfinished_turns() {
    let expected = "合成回答标记";
    assert!(completed_answer(
        &json!({"status":"completed","items":[{"type":"agentMessage","text":expected}]}),
        expected
    ));
    for turn in [
        json!({"status":"completed","items":[{"type":"userMessage","text":expected}]}),
        json!({"status":"completed","items":[{"type":"functionCallOutput","output":expected}]}),
        json!({"status":"inProgress","items":[{"type":"agentMessage","text":expected}]}),
        json!({"status":"failed","items":[{"type":"agentMessage","text":expected}]}),
        json!({"status":"completed","items":[{"type":"agentMessage","id":expected,"text":"其他回答"}]}),
    ] {
        assert!(!completed_answer(&turn, expected), "{turn}");
    }
}

async fn start_native_http(artifact: &Path) -> (Server, reqwest::Client, String, String) {
    let data = std::env::var_os("MOBILE_ACCEPTANCE_DATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| artifact.join(format!("service-{}", uuid::Uuid::new_v4())));
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
    (server, http, cookie, key)
}

#[tokio::test]
#[ignore = "需要已登录官方 Windows 桌面与授权聊天，仅验证文件引用读取，不发送消息"]
async fn official_native_context_references_use_scoped_project() {
    let thread = std::env::var("MOBILE_ACCEPTANCE_THREAD").unwrap();
    let artifact = PathBuf::from(std::env::var("MOBILE_ACCEPTANCE_DIRECTORY").unwrap());
    let (server, http, cookie, _) = start_native_http(&artifact).await;
    let projects = server.app.desktop.scoped_projects(&thread).await.unwrap();
    let root = std::fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap();
    let project = projects
        .iter()
        .find(|project| {
            project["path"]
                .as_str()
                .and_then(|path| std::fs::canonicalize(path).ok())
                .as_ref()
                == Some(&root)
        })
        .expect("当前官方目录必须包含验收项目");
    let project_id = project["projectId"].as_str().unwrap();
    let url = format!("http://{}/api/context", server.address);
    for params in [
        vec![("threadId", thread.as_str()), ("query", "package.json")],
        vec![
            ("sourceThreadId", thread.as_str()),
            ("projectId", project_id),
            ("query", "package.json"),
        ],
    ] {
        let response = http
            .get(&url)
            .header("cookie", &cookie)
            .query(&params)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let result: Value = response.json().await.unwrap();
        assert_eq!(result["mode"], "references");
        assert_eq!(result["plugins"], json!([]));
        assert_eq!(result["browserTabs"], json!([]));
        assert!(result["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["description"] == "package.json"));
    }
    server.stop().await.unwrap();
    std::fs::write(artifact.join("native-context-result.json"), json!({"existingThreadFiles":true,"scopedProjectFiles":true,"pluginAndBrowserCalls":false,"messagesSent":0,"stoppedCleanly":true}).to_string()).unwrap();
}

#[tokio::test]
#[ignore = "需要已登录官方 Windows 桌面及用户授权的专用测试聊天"]
async fn official_native_http_send_and_reconnect() {
    let thread = std::env::var("MOBILE_ACCEPTANCE_THREAD").expect("明确授权的测试聊天 ID");
    let artifact = PathBuf::from(std::env::var("MOBILE_ACCEPTANCE_DIRECTORY").unwrap());
    let selection = [
        ("model", "MOBILE_ACCEPTANCE_MODEL"),
        ("thinking", "MOBILE_ACCEPTANCE_THINKING"),
    ]
    .into_iter()
    .filter_map(|(field, variable)| {
        std::env::var(variable)
            .ok()
            .map(|value| (field.into(), json!(value)))
    })
    .collect::<serde_json::Map<String, Value>>();
    let (server, http, cookie, key) = start_native_http(&artifact).await;
    let url = format!("http://{}", server.address);
    let initial = http
        .get(format!("{url}/api/threads/{thread}/events"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    let initial = assert_native_snapshot(initial, &thread).await;
    assert_eq!(initial["firstTurnPending"], false);
    assert_eq!(initial["capabilities"]["send"], true);
    std::fs::write(
        artifact.join("http-initial-observation.json"),
        serde_json::to_vec_pretty(&initial).unwrap(),
    )
    .unwrap();
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
        let mut input = json!({"sourceThreadId":thread,"projectId":project["projectId"],"requestId":id,"title":"Mobile 新建验收",
            "prompt":"只回复“手机新建验收通过”。不要读取或修改文件，不运行命令，不调用工具，不创建其他聊天，不启动子智能体。"});
        input.as_object_mut().unwrap().extend(selection.clone());
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
            let stream = http
                .get(format!("{url}/api/threads/{created}/events"))
                .header("cookie", &cookie)
                .send()
                .await
                .unwrap();
            let snapshot = assert_native_snapshot(stream, &created).await;
            std::fs::write(
                artifact.join("http-create-observation.json"),
                serde_json::to_vec_pretty(&snapshot).unwrap(),
            )
            .unwrap();
            if let Some(failed) = snapshot["turns"]
                .as_array()
                .unwrap()
                .iter()
                .find(|turn| turn["status"] == "failed")
            {
                assert_eq!(
                    snapshot["firstTurnPending"], false,
                    "已失败的正式首轮仍在等待确认"
                );
                assert_eq!(
                    snapshot["capabilities"]["send"], true,
                    "首轮失败后应允许用户明确提交新消息"
                );
                let error = failed["error"].clone();
                server.stop().await.unwrap();
                panic!("官方已创建聊天，但首轮失败：{error}");
            }
            if snapshot["firstTurnPending"] == false
                && snapshot["turns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|turn| completed_answer(turn, "手机新建验收通过"))
            {
                done = true;
                break;
            }
        }
        assert!(done, "正式 ID 的首轮未通过手机接口确认");
        std::fs::write(artifact.join("http-create-result.json"),json!({"threadId":created,"createdOnce":true,"firstTurnObserved":true,"officialReplyRead":true,"selection":selection}).to_string()).unwrap();
        server.stop().await.unwrap();
        return;
    }
    let id = uuid::Uuid::new_v4().to_string();
    let marker = format!("HTTP手机通道验收{}", &id[..8]);
    let mut input = json!({"requestId":id,"mode":"now","prompt":format!("只回复“{marker}通过”。不要读取或修改文件，不运行命令，不调用工具，不新建聊天，不启动子智能体。")});
    input.as_object_mut().unwrap().extend(selection.clone());
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
            .any(|turn| completed_answer(turn, &format!("{marker}通过")))
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
    std::fs::write(artifact.join("http-send-result.json"), json!({"send":true,"sameRequestNoDuplicate":true,"officialReplyRead":true,"reconnect":true,"selection":selection}).to_string()).unwrap();
}

fn image_read_evidence(item: &Value, path: &Path) -> bool {
    match item["type"].as_str() {
        Some("imageView") => item["path"].as_str().is_some_and(|actual| {
            actual.replace('\\', "/") == path.to_string_lossy().replace('\\', "/")
        }),
        Some("functionCall" | "mcpToolCall") => {
            let call = item.to_string();
            call.contains("view_image")
                && call.contains(path.file_name().unwrap().to_str().unwrap())
        }
        _ => false,
    }
}

#[test]
fn image_read_evidence_accepts_official_items_and_rejects_text_or_wrong_paths() {
    let path = Path::new("C:/fixture/images/uploaded-id.png");
    for item in [
        json!({"type":"imageView","path":"C:\\fixture\\images\\uploaded-id.png"}),
        json!({"type":"functionCall","name":"view_image","arguments":{"path":path}}),
        json!({"type":"mcpToolCall","tool":"view_image","arguments":{"path":path}}),
    ] {
        assert!(image_read_evidence(&item, path), "{item}");
    }
    for item in [
        json!({"type":"imageView","path":"C:/another/images/uploaded-id.png"}),
        json!({"type":"imageView","path":"C:/fixture/images/other-id.png"}),
        json!({"type":"agentMessage","text":format!("view_image {}",path.display())}),
        json!({"type":"functionCallOutput","output":format!("view_image {}",path.display())}),
        json!({"type":"functionCall","name":"view_image","arguments":{"path":"other-id.png"}}),
        json!({"type":"mcpToolCall","tool":"view_image","arguments":{"path":"other-id.png"}}),
    ] {
        assert!(!image_read_evidence(&item, path), "{item}");
    }
}

#[tokio::test]
#[ignore = "需显式授权的专用聊天、两张含独有标记的合成图片及model/thinking；实际发送三轮"]
async fn official_native_http_images_and_model_are_read_by_the_actual_turn() {
    let thread = std::env::var("MOBILE_ACCEPTANCE_THREAD").expect("明确授权的专用测试聊天 ID");
    let artifact = PathBuf::from(std::env::var("MOBILE_ACCEPTANCE_DIRECTORY").unwrap());
    let images = [
        (
            PathBuf::from(std::env::var_os("MOBILE_ACCEPTANCE_IMAGE").expect("第一张合成图片")),
            std::env::var("MOBILE_ACCEPTANCE_IMAGE_EXPECTED").expect("第一张图中独有标记"),
        ),
        (
            PathBuf::from(
                std::env::var_os("MOBILE_ACCEPTANCE_IMAGE_SECOND").expect("第二张合成图片"),
            ),
            std::env::var("MOBILE_ACCEPTANCE_IMAGE_SECOND_EXPECTED").expect("第二张图中独有标记"),
        ),
    ];
    for (path, expected) in &images {
        assert!(path.is_file(), "合成图片不存在：{}", path.display());
        assert!(!expected.trim().is_empty(), "图中预期标记不能为空");
    }
    assert_ne!(images[0].1, images[1].1, "两张图必须包含不同标记");
    let model = std::env::var("MOBILE_ACCEPTANCE_MODEL").expect("显式选择模型 ID");
    let thinking = std::env::var("MOBILE_ACCEPTANCE_THINKING").expect("显式选择思考强度");
    let (server, http, cookie, _) = start_native_http(&artifact).await;
    let url = format!("http://{}", server.address);
    let config_path = server.app.paths.codex_home.join("config.toml");
    let config_before = std::fs::read(&config_path).unwrap();
    let mut evidence = Vec::new();
    let cases = [
        ("image-only",vec![0],""),
        ("text-and-image",vec![1],"请使用读图工具查看附件，只输出图中的独有文字标记。不要读取其他文件、运行命令、修改文件、创建聊天或启动子智能体。"),
        ("multiple-images",vec![0,1],"请使用读图工具分别查看两张附件，按顺序输出每张图中的独有文字标记。不要读取其他文件、运行命令、修改文件、创建聊天或启动子智能体。"),
    ];
    for (name, indices, prompt) in cases {
        let mut uploaded = Vec::<String>::new();
        for (position, index) in indices.iter().enumerate() {
            let response = http
                .post(format!("{url}/api/images"))
                .header("cookie", &cookie)
                .header("x-codex-mobile-contract", "2")
                .header("x-file-name", format!("acceptance-image-{position}.png"))
                .body(std::fs::read(&images[*index].0).unwrap())
                .send()
                .await
                .unwrap();
            let status = response.status();
            let image: Value = response.json().await.unwrap();
            assert_eq!(status, 201, "{image}");
            uploaded.push(image["id"].as_str().unwrap().to_owned());
        }
        let request_id = uuid::Uuid::new_v4().to_string();
        let input = json!({"requestId":request_id,"mode":"now","prompt":prompt,"images":uploaded,"model":model,"thinking":thinking});
        assert!(
            indices
                .iter()
                .all(|index| !input.to_string().contains(&images[*index].1)),
            "预期答案不能进入请求"
        );
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
            let receipt: Value = response.json().await.unwrap();
            std::fs::write(
                artifact.join(format!("http-image-{name}-receipt.json")),
                receipt.to_string(),
            )
            .unwrap();
            assert!(status.is_success(), "{status}: {receipt}");
            assert_eq!(receipt["accepted"], true);
        }
        let turn: Value = tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let response = http
                    .get(format!("{url}/api/threads/{thread}/history"))
                    .header("cookie", &cookie)
                    .send()
                    .await
                    .unwrap();
                assert!(response.status().is_success());
                let history: Value = response.json().await.unwrap();
                let matching = history["turns"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|turn| {
                        turn["items"].as_array().unwrap().iter().any(|item| {
                            let input = if item["type"] == "userMessage" {
                                Some(item.to_string())
                            } else {
                                codex_mobile_bridge::state::chat_creation_input(item)
                                    .map(|text| text.into_owned())
                            };
                            input.is_some_and(|input| uploaded.iter().all(|id| input.contains(id)))
                        })
                    })
                    .collect::<Vec<_>>();
                assert!(matching.len() <= 1, "同requestId不得产生多个图片轮次");
                if let Some(turn) = matching
                    .first()
                    .filter(|turn| turn["status"] == "completed")
                {
                    break (*turn).clone();
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        })
        .await
        .expect("图片轮次未在180秒内完成；不会自动重发");
        let mut viewed = vec![false; uploaded.len()];
        for item in turn["items"].as_array().unwrap() {
            let raw = server
                .app
                .history
                .item(&thread, item["id"].as_str().unwrap())
                .unwrap()
                .unwrap();
            for (index, id) in uploaded.iter().enumerate() {
                let path = server.app.paths.data_home.join("images").join(id);
                viewed[index] |= image_read_evidence(&raw, &path);
            }
        }
        let rollout: String = server
            .app
            .history
            .state()
            .unwrap()
            .query_row(
                "SELECT rollout_path FROM threads WHERE id=?",
                [&thread],
                |row| row.get(0),
            )
            .unwrap();
        let context = BufReader::new(std::fs::File::open(rollout).unwrap())
            .lines()
            .map(|line| serde_json::from_str::<Value>(&line.unwrap()).unwrap())
            .filter(|record| {
                record["type"] == "turn_context" && record["payload"]["turn_id"] == turn["id"]
            })
            .map(|record| record["payload"].clone())
            .last()
            .expect("实际轮次缺少turn_context，不能以线程设置冒充模型生效");
        let recognized = indices
            .iter()
            .map(|index| completed_answer(&turn, &images[*index].1))
            .collect::<Vec<_>>();
        evidence.push(json!({"case":name,"requestId":request_id,"turnId":turn["id"],"recognized":recognized,
            "imageReadToolObserved":viewed,"model":context["model"],"thinking":context["effort"],"duplicateRequestCreatedOneTurn":true}));
        std::fs::write(
            artifact.join("http-images-result.json"),
            serde_json::to_vec_pretty(&evidence).unwrap(),
        )
        .unwrap();
        assert!(
            recognized.iter().all(|recognized| *recognized),
            "{name}未从官方回答读出全部图中标记"
        );
        assert!(
            viewed.iter().all(|viewed| *viewed),
            "{name}缺少读取上传图片的官方工具调用证据"
        );
        assert_eq!(context["model"], model, "{name}当前轮次模型不符");
        assert_eq!(context["effort"], thinking, "{name}当前轮次思考强度不符");
        assert!(
            std::fs::read(&config_path).unwrap() == config_before,
            "消息级选项不得改写全局配置"
        );
    }
    let reconnect = http
        .get(format!("{url}/api/threads/{thread}/events"))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_native_snapshot(reconnect, &thread).await;
    server.stop().await.unwrap();
}
