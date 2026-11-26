use crate::{
    app_server::AppServerConnection,
    config::ServiceConfig,
    http::{Bridge, Server},
    native_test_support::{NativePipe, CHAT, CREATED, OTHER},
    test_support::Fixture,
};
use futures_util::{Stream, StreamExt};
use reqwest::{Client, RequestBuilder};
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

async fn start(
    fixture: &Fixture,
) -> (
    Server,
    Arc<NativePipe>,
    Arc<AppServerConnection>,
    Client,
    String,
) {
    let app = Bridge::open(
        fixture.paths.clone(),
        ServiceConfig {
            port: 0,
            bind: "127.0.0.1".into(),
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let (peer, connection) = NativePipe::start(&fixture.paths).await;
    app.session.install_native(connection.clone());
    let server = app.start().await.unwrap();
    let client = Client::builder().no_proxy().build().unwrap();
    let key = server.app.auth.lock().await.key().to_owned();
    let response = client
        .post(format!("http://{}/api/login", server.address))
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
        .to_owned();
    (server, peer, connection, client, cookie)
}

fn post(server: &Server, client: &Client, cookie: &str, path: &str) -> RequestBuilder {
    client
        .post(format!("http://{}/api/{path}", server.address))
        .header("cookie", cookie)
        .header("x-codex-mobile-contract", "2")
}

async fn result(request: RequestBuilder) -> (u16, Value) {
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap())
}

async fn lookup(server: &Server, client: &Client, cookie: &str, id: &str) -> Value {
    let response = client
        .get(format!("http://{}/api/operations", server.address))
        .header("cookie", cookie)
        .query(&[("ids", id)])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    response.json::<Value>().await.unwrap()["results"][0].clone()
}

async fn next_snapshot<S>(stream: &mut S, buffer: &mut String) -> Value
where
    S: Stream<Item = std::result::Result<bytes::Bytes, reqwest::Error>> + Unpin,
{
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            while let Some(end) = buffer.find("\n\n") {
                let event = buffer[..end].to_owned();
                buffer.drain(..end + 2);
                if event.lines().any(|line| line == "event: snapshot") {
                    let data = event
                        .lines()
                        .filter_map(|line| line.strip_prefix("data: "))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return serde_json::from_str(&data).unwrap();
                }
            }
            let bytes = stream.next().await.expect("SSE意外结束").unwrap();
            buffer.push_str(std::str::from_utf8(&bytes).unwrap());
        }
    })
    .await
    .unwrap()
}

// INTERACT-008：原生答案接受与官方问题结束分别记录；重复、刷新、重启不重发。
#[tokio::test]
async fn native_async_answer_delivery_survives_restart_without_resolving_the_question() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    peer.reads.lock().await.get_mut(CHAT).unwrap()["turns"] = json!([{
        "id":"question-turn","startedAt":100,"status":"completed","items":[{
            "id":"question-item","type":"agentMessage","questions":[{"title":"选择合成答案","options":["甲","乙"]}]
        }]
    }]);
    let read = peer.reads.lock().await[CHAT].clone();
    let history_url = format!("http://{}/api/threads/{CHAT}/history", server.address);
    let (_, snapshot) = result(client.get(&history_url).header("cookie", &cookie)).await;
    assert_eq!(snapshot["capabilities"]["asyncReplies"], true);
    assert_eq!(snapshot["capabilities"]["interactions"], false);
    let question = snapshot["interactions"][0].clone();
    assert_eq!(question["replySupported"], true);
    let request = question["id"].as_str().unwrap();
    let item = question["questions"][0]["id"].as_str().unwrap();
    let path = format!(
        "threads/{CHAT}/interactions/{}",
        percent_encoding::utf8_percent_encode(request, percent_encoding::NON_ALPHANUMERIC)
    );
    let body = json!({"answers":{item:["乙"]}});
    assert_eq!(
        client
            .post(format!("http://{}/api/{path}", server.address))
            .json(&body)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let (status, receipt) = result(post(&server, &client, &cookie, &path).json(&body)).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(receipt["deliveryStatus"], "accepted");
    assert_eq!(receipt["resolved"], false);
    let (_, duplicate) = result(post(&server, &client, &cookie, &path).json(&body)).await;
    assert_eq!(duplicate["deliveryStatus"], "accepted");
    assert_eq!(peer.count("send_message_to_thread").await, 1);
    let calls = peer.calls.lock().await;
    let send = calls
        .iter()
        .find(|call| call["params"]["tool"] == "send_message_to_thread")
        .unwrap();
    assert_eq!(send["params"]["threadId"], CHAT);
    assert_eq!(send["params"]["arguments"]["threadId"], CHAT);
    let prompt = send["params"]["arguments"]["prompt"].as_str().unwrap();
    let answer: Value = serde_json::from_str(
        prompt
            .strip_prefix("<send_user_message_question_reply>\n")
            .unwrap()
            .strip_suffix("\n</send_user_message_question_reply>")
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        answer,
        json!([{"questionItemId":item,"question":"选择合成答案","answer":"乙"}])
    );
    drop(calls);
    let (_, snapshot) = result(client.get(&history_url).header("cookie", &cookie)).await;
    assert_eq!(snapshot["interactions"][0]["id"], request);
    assert_eq!(snapshot["interactions"][0]["deliveryStatus"], "accepted");
    assert_eq!(snapshot["interactions"][0]["replySupported"], false);
    let (status, _) = result(
        post(
            &server,
            &client,
            &cookie,
            &format!("threads/{CHAT}/interactions/approval-id"),
        )
        .json(&json!({"decision":"accept"})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(peer.count("send_message_to_thread").await, 1);
    server.stop().await.unwrap();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    peer.reads.lock().await.insert(CHAT.into(), read);
    let (_, snapshot) = result(
        client
            .get(format!(
                "http://{}/api/threads/{CHAT}/history",
                server.address
            ))
            .header("cookie", &cookie),
    )
    .await;
    assert_eq!(snapshot["interactions"][0]["deliveryStatus"], "accepted");
    let (_, receipt) = result(post(&server, &client, &cookie, &path).json(&body)).await;
    assert_eq!(receipt["deliveryStatus"], "accepted");
    assert_eq!(receipt["resolved"], false);
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn native_async_answer_unknown_receipt_is_not_replayed_and_partial_answers_are_rejected() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    peer.reads.lock().await.get_mut(CHAT).unwrap()["turns"] = json!([{
        "id":"question-turn","startedAt":100,"status":"completed","items":[{
            "id":"question-item","type":"agentMessage","questions":[{"title":"合成问题","options":[]}]
        }]
    }]);
    let url = format!("http://{}/api/threads/{CHAT}/history", server.address);
    let (_, snapshot) = result(client.get(&url).header("cookie", &cookie)).await;
    let request = snapshot["interactions"][0]["id"].as_str().unwrap();
    let item = snapshot["interactions"][0]["questions"][0]["id"]
        .as_str()
        .unwrap();
    let path = format!(
        "threads/{CHAT}/interactions/{}",
        percent_encoding::utf8_percent_encode(request, percent_encoding::NON_ALPHANUMERIC)
    );
    let invalid = json!({"answers":{},"skipped":[item]});
    let (status, failure) = result(post(&server, &client, &cookie, &path).json(&invalid)).await;
    assert_eq!(status, 400);
    assert_eq!(failure["outcome"], "not-dispatched");
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    // 明确未投递后允许修正答案；收到未知回执后仍禁止再次投递。
    *peer.send_reply.lock().await = Some(
        json!({"success":true,"contentItems":[{"type":"inputText","text":"unrecognized receipt"}]}),
    );
    let body = json!({"answers":{item:["合成答案"]}});
    for _ in 0..2 {
        let (status, error) = result(post(&server, &client, &cookie, &path).json(&body)).await;
        assert_ne!(status, 200);
        assert_eq!(error["outcome"], "unknown");
    }
    assert_eq!(peer.count("send_message_to_thread").await, 1);
    let (_, snapshot) = result(client.get(&url).header("cookie", &cookie)).await;
    assert_eq!(snapshot["interactions"][0]["deliveryStatus"], "unknown");
    assert_eq!(snapshot["interactions"][0]["replySupported"], false);
    server.stop().await.unwrap();
}

// CONTROL-012：继续绑定最后失败/中断轮，活跃聊天和旧轮不能触发新消息。
#[tokio::test]
async fn native_continue_validates_latest_turn_and_does_not_repeat_after_restart() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    let path = format!("threads/{CHAT}/resume");
    for (name, status, thread_status, requested, expected) in [
        ("completed", "completed", json!("idle"), "completed", 409),
        ("running", "inProgress", json!("active"), "running", 409),
        (
            "new-active",
            "failed",
            json!({"type":"active"}),
            "new-active",
            409,
        ),
        ("latest", "failed", json!("idle"), "older", 409),
        ("failed", "failed", json!("idle"), "failed", 200),
        ("stopped", "interrupted", json!("idle"), "stopped", 200),
    ] {
        {
            let mut reads = peer.reads.lock().await;
            let read = reads.get_mut(CHAT).unwrap();
            read["thread"]["status"] = thread_status;
            read["turns"] = json!([{"id":name,"startedAt":100,"status":status,"items":[]}]);
        }
        let (status, receipt) =
            result(post(&server, &client, &cookie, &path).json(&json!({"turnId":requested}))).await;
        assert_eq!(status, expected, "{name}: {receipt}");
        if expected == 409 {
            assert_eq!(receipt["outcome"], "not-dispatched");
        }
    }
    let (_, duplicate) =
        result(post(&server, &client, &cookie, &path).json(&json!({"turnId":"stopped"}))).await;
    assert_eq!(duplicate["status"], "accepted");
    assert_eq!(peer.count("send_message_to_thread").await, 2);
    let calls = peer.calls.lock().await;
    let prompts = calls
        .iter()
        .filter(|call| call["params"]["tool"] == "send_message_to_thread")
        .map(|call| call["params"]["arguments"]["prompt"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(prompts[0].contains("failed"));
    assert!(prompts[1].contains("stopped"));
    drop(calls);
    server.stop().await.unwrap();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    let (status, receipt) =
        result(post(&server, &client, &cookie, &path).json(&json!({"turnId":"stopped"}))).await;
    assert_eq!(status, 200, "{receipt}");
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    server.stop().await.unwrap();
}

// OPS-003：真实提交入口的前置失败必须保持未投递，不创建未知操作。
// MANAGED-001/002 迁移：创建统一交给 App Tools，首轮确认与创建身份仍分开，重启不重建。
#[tokio::test]
async fn official_creation_keeps_durable_identity_across_restart_without_separate_model_turn() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    let input = json!({"sourceThreadId":CHAT,"projectId":"project","prompt":"合成首轮","requestId":"official-create-restart-001"});
    let (status, created) = result(post(&server, &client, &cookie, "threads").json(&input)).await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["threadId"], CREATED);
    assert_eq!(created["firstTurnAccepted"], false);
    let (_, again) = result(post(&server, &client, &cookie, "threads").json(&input)).await;
    assert_eq!(again, created);
    assert_eq!(peer.count("create_thread").await, 1);
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    assert!(peer
        .calls
        .lock()
        .await
        .iter()
        .all(|call| matches!(call["method"].as_str(), Some("tools/list" | "tools/call"))));
    server.stop().await.unwrap();
    let (restarted, next_peer, _, client, cookie) = start(&fixture).await;
    let (_, again) = result(post(&restarted, &client, &cookie, "threads").json(&input)).await;
    assert_eq!(again["threadId"], CREATED);
    assert_eq!(next_peer.count("create_thread").await, 0);
    assert_eq!(next_peer.count("send_message_to_thread").await, 0);
    restarted.stop().await.unwrap();
}

// 官方创建包含首条指令，回执无法确认时原创建身份阻止重建，不拆出另一条模型请求。
#[tokio::test]
async fn lost_official_creation_receipt_remains_queryable_and_never_recreates_or_replays() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    *peer.create_reply.lock().await = Some(
        json!({"success":true,"contentItems":[{"type":"inputText","text":"unrecognized receipt"}]}),
    );
    let input = json!({"sourceThreadId":CHAT,"projectId":"project","prompt":"合成首轮","requestId":"official-create-lost-001"});
    let (status, failure) = result(post(&server, &client, &cookie, "threads").json(&input)).await;
    assert_ne!(status, 200);
    assert_eq!(failure["outcome"], "unknown");
    let (_, again) = result(post(&server, &client, &cookie, "threads").json(&input)).await;
    assert_eq!(again["outcome"], "unknown");
    let pending = lookup(&server, &client, &cookie, "official-create-lost-001").await;
    assert_eq!(pending["phase"], "unknown");
    assert_eq!(pending["target"], "new:project");
    assert_eq!(pending["kind"], "create");
    let mut changed = input.clone();
    changed["requestId"] = json!("official-create-changed-002");
    changed["prompt"] = json!("修改文字也不能绕过未决创建");
    let (status, _) = result(post(&server, &client, &cookie, "threads").json(&changed)).await;
    assert_eq!(status, 409);
    assert_eq!(peer.count("create_thread").await, 1);
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn stale_mobile_execution_is_rejected_before_dispatch_and_does_not_create_an_operation() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    let (status, failure) = result(post(&server, &client, &cookie, "threads").json(&json!({
        "execution":"mobile","sourceThreadId":CHAT,"projectId":"project","prompt":"旧页面合成请求","requestId":"stale-mobile-create-001"
    }))).await;
    assert_eq!(status, 400);
    assert_eq!(failure["outcome"], "not-dispatched");
    assert_eq!(peer.count("create_thread").await, 0);
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    assert_eq!(
        lookup(&server, &client, &cookie, "stale-mobile-create-001").await["phase"],
        "missing"
    );
    assert!(!fixture.paths.data_home.join("managed").exists());
    assert_ne!(server.app.session.capabilities()["managedCreate"], true);
    server.stop().await.unwrap();
}

#[tokio::test]
async fn http_submission_preflight_failures_never_dispatch_or_become_unknown() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    let image = "00000000000000000000000000000000.png";
    let invalid = [
        json!({"mode":"invalid"}),
        json!({"prompt":" "}),
        json!({"requestId":"short"}),
        json!({"images":"bad"}),
        json!({"images":["../private.png"]}),
        json!({"images":[image,image]}),
        json!({"images":vec![image;21]}),
        json!({"model":"invalid model"}),
        json!({"thinking":"unsupported"}),
        json!({"mode":"queue"}),
    ];
    for (index, changes) in invalid.into_iter().enumerate() {
        let id = format!("preflight-message-{index:02}");
        let mut input = json!({"prompt":"合成测试","requestId":id});
        input
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        let (status, response) = result(
            post(
                &server,
                &client,
                &cookie,
                &format!("threads/{CHAT}/messages"),
            )
            .json(&input),
        )
        .await;
        assert!(status >= 400, "{input}: {response}");
        assert_eq!(response["outcome"], "not-dispatched", "{input}: {response}");
        assert_eq!(
            lookup(&server, &client, &cookie, &id).await["phase"],
            "missing"
        );
    }
    let invalid = [
        json!({"unexpected":true}),
        json!({"title":" "}),
        json!({"title":"长".repeat(201)}),
        json!({"sourceThreadId":""}),
        json!({"projectId":""}),
        json!({"sourceThreadId":"invalid"}),
    ];
    for (index, changes) in invalid.into_iter().enumerate() {
        let id = format!("preflight-create-{index:02}");
        let mut input =
            json!({"prompt":"合成测试","requestId":id,"sourceThreadId":CHAT,"projectId":"project"});
        input
            .as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());
        let (status, response) =
            result(post(&server, &client, &cookie, "threads").json(&input)).await;
        assert!(status >= 400, "{input}: {response}");
        assert_eq!(response["outcome"], "not-dispatched");
        assert_eq!(
            lookup(&server, &client, &cookie, &id).await["phase"],
            "missing"
        );
    }
    peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["hostId"] = json!("another-host");
    for (path, input) in [
        (
            format!("threads/{CHAT}/messages"),
            json!({"prompt":"合成测试","requestId":"foreign-send-00001"}),
        ),
        (
            "threads".into(),
            json!({"prompt":"合成测试","requestId":"foreign-create-001","sourceThreadId":CHAT,"projectId":"project"}),
        ),
    ] {
        let (status, response) = result(post(&server, &client, &cookie, &path).json(&input)).await;
        assert_eq!(status, 403);
        assert_eq!(response["outcome"], "not-dispatched");
        assert_eq!(
            lookup(
                &server,
                &client,
                &cookie,
                input["requestId"].as_str().unwrap()
            )
            .await["phase"],
            "missing"
        );
    }
    peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["hostId"] = json!("local");
    // 附件读取和项目检查发生在登记操作之后，必须保存可安全重试的失败事实。
    for (path, input, expected_status) in [
        (
            format!("threads/{CHAT}/messages"),
            json!({"prompt":"合成测试","requestId":"missing-image-send","images":[image]}),
            404,
        ),
        (
            "threads".into(),
            json!({"prompt":"合成测试","requestId":"missing-image-create","sourceThreadId":CHAT,"projectId":"project","images":[image]}),
            404,
        ),
        (
            "threads".into(),
            json!({"prompt":"合成测试","requestId":"missing-project-create","sourceThreadId":CHAT,"projectId":"missing"}),
            400,
        ),
    ] {
        let (status, response) = result(post(&server, &client, &cookie, &path).json(&input)).await;
        assert_eq!(status, expected_status, "{response}");
        assert_eq!(response["outcome"], "not-dispatched");
        assert_eq!(
            lookup(
                &server,
                &client,
                &cookie,
                input["requestId"].as_str().unwrap()
            )
            .await["phase"],
            "not-dispatched"
        );
    }
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    assert_eq!(peer.count("create_thread").await, 0);
    server.stop().await.unwrap();
}

// READ-007、SEND-002：首轮从未确认转为已观察，HTTP SSE发布完整快照且不重复创建。
#[tokio::test]
async fn http_first_turn_confirmation_publishes_snapshot_without_creating_twice() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    let input = json!({"prompt":"合成首轮","requestId":"first-turn-create-001","sourceThreadId":CHAT,"projectId":"project"});
    let (status, created) = result(post(&server, &client, &cookie, "threads").json(&input)).await;
    assert_eq!(status, 200, "{created}");
    assert_eq!(created["threadId"], CREATED);
    assert_eq!(created["firstTurnAccepted"], false);
    let (status, blocked) = result(
        post(
            &server,
            &client,
            &cookie,
            &format!("threads/{CREATED}/messages"),
        )
        .json(&json!({"prompt":"不能抢发第二轮","requestId":"first-turn-blocked"})),
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(blocked["outcome"], "not-dispatched");
    let response = client
        .get(format!(
            "http://{}/api/threads/{CREATED}/events",
            server.address
        ))
        .header("cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let first = next_snapshot(&mut stream, &mut buffer).await;
    assert_eq!(first["firstTurnPending"], true);
    assert_eq!(first["capabilities"]["send"], false);
    peer.reads.lock().await.get_mut(CREATED).unwrap()["turns"] = json!([
        {"id":"native-first-turn","status":"inProgress","startedAt":1_795_579_200,"items":[]}
    ]);
    let confirmed = next_snapshot(&mut stream, &mut buffer).await;
    assert_eq!(confirmed["firstTurnPending"], false);
    assert_eq!(confirmed["capabilities"]["send"], true);
    assert_eq!(confirmed["turns"][0]["id"], "native-first-turn");
    let receipt = lookup(&server, &client, &cookie, "first-turn-create-001").await;
    assert_eq!(receipt["receipt"]["firstTurnAccepted"], true);
    assert_eq!(receipt["receipt"]["firstTurnId"], "native-first-turn");
    let (_, replay) = result(post(&server, &client, &cookie, "threads").json(&input)).await;
    assert_eq!(replay["firstTurnAccepted"], true);
    assert_eq!(peer.count("create_thread").await, 1);
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    drop(stream);
    server.stop().await.unwrap();
}

// OPS-001/002/004：HTTP并发使用实际Submissions去重；其他聊天继续发送。
#[tokio::test]
async fn http_duplicate_submissions_share_one_dispatch_and_other_target_remains_available() {
    let fixture = Fixture::new();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    peer.hold_sends.store(true, Ordering::Release);
    let input = json!({"prompt":"合成并发","requestId":"duplicate-send-001","model":"test-model","thinking":"high"});
    let first = tokio::spawn(result(
        post(
            &server,
            &client,
            &cookie,
            &format!("threads/{CHAT}/messages"),
        )
        .json(&input),
    ));
    peer.wait_calls("send_message_to_thread", 1).await;
    let duplicate = tokio::spawn(result(
        post(
            &server,
            &client,
            &cookie,
            &format!("threads/{CHAT}/messages"),
        )
        .json(&input),
    ));
    assert_eq!(
        lookup(&server, &client, &cookie, "duplicate-send-001").await["phase"],
        "dispatching"
    );
    let (status, blocked) = result(
        post(
            &server,
            &client,
            &cookie,
            &format!("threads/{CHAT}/messages"),
        )
        .json(&json!({"prompt":"同目标新消息","requestId":"another-send-00001"})),
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(blocked["outcome"], "not-dispatched");
    peer.hold_sends.store(false, Ordering::Release);
    let (status, separate) = result(
        post(
            &server,
            &client,
            &cookie,
            &format!("threads/{OTHER}/messages"),
        )
        .json(&json!({"prompt":"独立目标","requestId":"other-target-00001"})),
    )
    .await;
    assert_eq!(status, 200, "{separate}");
    peer.release_sends.add_permits(1);
    assert_eq!(first.await.unwrap().0, 200);
    assert_eq!(duplicate.await.unwrap().0, 200);
    assert_eq!(peer.count("send_message_to_thread").await, 2);
    let calls = peer.calls.lock().await;
    let sent = calls
        .iter()
        .find(|call| call["params"]["tool"] == "send_message_to_thread")
        .unwrap();
    assert_eq!(sent["params"]["arguments"]["model"], "test-model");
    assert_eq!(sent["params"]["arguments"]["thinking"], "high");
    assert_eq!(sent["params"]["callId"], "duplicate-send-001");
    drop(calls);
    assert_eq!(
        lookup(&server, &client, &cookie, "duplicate-send-001").await["phase"],
        "accepted"
    );
    server.stop().await.unwrap();
}

// OPS-003/005：官方调用已经发出时，错误或损坏回执属于unknown，重复请求不得重投。
#[tokio::test]
async fn http_official_rejection_or_malformed_receipt_remains_unknown_without_retrying() {
    for reply in [
        json!({"success":false,"contentItems":[{"type":"inputText","text":"合成拒绝"}]}),
        json!({"success":true,"contentItems":[{"type":"inputText","text":"invalid json"}]}),
        json!({"success":true,"contentItems":[{"type":"inputText","text":json!({"threadId":OTHER,"status":"accepted"}).to_string()}]}),
    ] {
        let fixture = Fixture::new();
        let (server, peer, _, client, cookie) = start(&fixture).await;
        *peer.send_reply.lock().await = Some(reply);
        let input = json!({"prompt":"合成错误回执","requestId":"unknown-result-001"});
        for _ in 0..2 {
            let (status, response) = result(
                post(
                    &server,
                    &client,
                    &cookie,
                    &format!("threads/{CHAT}/messages"),
                )
                .json(&input),
            )
            .await;
            assert_eq!(status, 503, "{response}");
            assert_eq!(response["outcome"], "unknown");
        }
        assert_eq!(
            lookup(&server, &client, &cookie, "unknown-result-001").await["phase"],
            "unknown"
        );
        assert_eq!(peer.count("send_message_to_thread").await, 1);
        server.stop().await.unwrap();
    }
}

// OPS-009：真实35秒关停等待仍在执行的提交，重启恢复unknown且永不自动重发。
#[tokio::test]
async fn http_inflight_submission_survives_client_disconnect_and_shutdown_marks_unknown() {
    tokio::time::timeout(Duration::from_secs(50), async {
        let fixture = Fixture::new();
        let (server, peer, connection, client, cookie) = start(&fixture).await;
        peer.hold_sends.store(true, Ordering::Release);
        let input = json!({"prompt":"合成关停","requestId":"shutdown-flight-001"});
        let request = tokio::spawn(result(
            post(
                &server,
                &client,
                &cookie,
                &format!("threads/{CHAT}/messages"),
            )
            .json(&input),
        ));
        peer.wait_calls("send_message_to_thread", 1).await;
        request.abort();
        let _ = request.await;
        assert_eq!(
            lookup(&server, &client, &cookie, "shutdown-flight-001").await["phase"],
            "dispatching"
        );
        let started = Instant::now();
        server.stop().await.unwrap();
        assert!(
            started.elapsed() >= Duration::from_secs(35),
            "未执行真实关停等待: {:?}",
            started.elapsed()
        );
        assert!(!connection.is_alive());
        let (server, restarted, _, client, cookie) = start(&fixture).await;
        assert_eq!(
            lookup(&server, &client, &cookie, "shutdown-flight-001").await["phase"],
            "unknown"
        );
        let (status, response) = result(
            post(
                &server,
                &client,
                &cookie,
                &format!("threads/{CHAT}/messages"),
            )
            .json(&input),
        )
        .await;
        assert_eq!(status, 409, "{response}");
        assert_eq!(response["outcome"], "unknown");
        let (status, response) = result(
            post(
                &server,
                &client,
                &cookie,
                &format!("threads/{CHAT}/messages"),
            )
            .json(&json!({"prompt":"不能绕过未决操作","requestId":"shutdown-new-00001"})),
        )
        .await;
        assert_eq!(status, 409);
        assert_eq!(response["outcome"], "not-dispatched");
        assert_eq!(peer.count("send_message_to_thread").await, 1);
        assert_eq!(restarted.count("send_message_to_thread").await, 0);
        server.stop().await.unwrap();
    })
    .await
    .unwrap();
}

// CONTROL-007：导航独立于全局目录能力，成功必须有官方肯定回执。
#[tokio::test]
async fn http_desktop_navigation_requires_confirmed_receipt_and_local_ownership() {
    let fixture = Fixture::new();
    let (server, peer, connection, client, cookie) = start(&fixture).await;
    assert_eq!(server.app.session.capabilities()["catalog"], false);
    assert_eq!(server.app.session.capabilities()["desktopNavigation"], true);
    let (status, response) =
        result(post(&server, &client, &cookie, &format!("threads/{CHAT}/open")).json(&json!({})))
            .await;
    assert_eq!(status, 200, "{response}");
    assert_eq!(response["ok"], true);
    for reply in [json!({}), json!({"navigated":false})] {
        *peer.navigation_reply.lock().await = reply;
        let (status, response) = result(
            post(&server, &client, &cookie, &format!("threads/{CHAT}/open")).json(&json!({})),
        )
        .await;
        assert_eq!(status, 503, "{response}");
        assert_ne!(response["ok"], true);
    }
    peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["hostId"] = json!("foreign");
    for id in [CHAT, "invalid"] {
        let (status, _) =
            result(post(&server, &client, &cookie, &format!("threads/{id}/open")).json(&json!({})))
                .await;
        assert!(status >= 400);
    }
    let calls = peer.calls.lock().await;
    let navigations = calls
        .iter()
        .filter(|call| call["params"]["tool"] == "navigate_to_codex_page")
        .collect::<Vec<_>>();
    assert_eq!(navigations.len(), 3);
    for navigation in navigations {
        assert_eq!(navigation["params"]["threadId"], CHAT);
        assert_eq!(navigation["params"]["arguments"]["threadId"], CHAT);
        assert_eq!(navigation["params"]["hostId"], "local");
    }
    drop(calls);
    connection.close();
    assert_eq!(
        server.app.session.capabilities()["desktopNavigation"],
        false
    );
    server.stop().await.unwrap();
}

// CONTROL-008：原生连接可读取Hooks，页面只读不会执行或改写用户配置。
#[tokio::test]
async fn http_hooks_reads_user_and_project_sources_without_executing_or_writing() {
    let fixture = Fixture::new();
    let project = fixture.temp.path().join("project");
    std::fs::create_dir_all(project.join(".codex")).unwrap();
    let user_path = fixture.paths.codex_home.join("hooks.json");
    let project_path = project.join(".codex/hooks.json");
    let user_source =
        br#"{"Stop":[{"hooks":[{"type":"command","command":"synthetic-never-execute"}]}]}"#;
    let project_source = br#"{"SessionStart":[]}"#;
    std::fs::write(&user_path, user_source).unwrap();
    std::fs::write(&project_path, project_source).unwrap();
    let (server, peer, _, client, cookie) = start(&fixture).await;
    peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["cwd"] = json!(project);
    let url = format!("http://{}/api/threads/{CHAT}/hooks", server.address);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
    let (status, hooks) = result(client.get(&url).header("cookie", &cookie)).await;
    assert_eq!(status, 200, "{hooks}");
    assert_eq!(hooks["mode"], "read-only");
    assert!(hooks["pendingReview"].is_null());
    assert_eq!(hooks["sources"].as_array().unwrap().len(), 2);
    assert_eq!(hooks["issues"], json!([]));
    let sources = hooks["sources"].as_array().unwrap();
    assert!(sources
        .iter()
        .any(|source| source["path"] == json!(user_path)
            && source["config"]["Stop"][0]["hooks"][0]["command"] == "synthetic-never-execute"));
    assert!(sources
        .iter()
        .any(|source| source["path"] == json!(project_path)
            && source["config"]["SessionStart"] == json!([])));
    let (status, _) = result(
        post(&server, &client, &cookie, &format!("threads/{CHAT}/hooks"))
            .json(&json!({"config":{}})),
    )
    .await;
    assert!(status >= 400);
    assert_eq!(std::fs::read(&user_path).unwrap(), user_source);
    assert_eq!(std::fs::read(&project_path).unwrap(), project_source);
    std::fs::write(&project_path, b"invalid json").unwrap();
    let (status, hooks) = result(client.get(&url).header("cookie", &cookie)).await;
    assert_eq!(status, 200);
    assert_eq!(hooks["sources"].as_array().unwrap().len(), 1);
    assert_eq!(hooks["issues"].as_array().unwrap().len(), 1);
    assert_eq!(hooks["issues"][0]["path"], json!(project_path));
    assert_eq!(std::fs::read(&project_path).unwrap(), b"invalid json");
    assert_eq!(peer.count("send_message_to_thread").await, 0);
    assert_eq!(peer.count("create_thread").await, 0);
    assert_eq!(peer.count("navigate_to_codex_page").await, 0);
    server.stop().await.unwrap();
}

// CONTROL-009：普通引用独立于全局目录能力，项目路径只能来自已确认的来源。
#[tokio::test]
async fn http_context_references_preserve_auth_source_and_project_boundaries() {
    let fixture = Fixture::new();
    let project = fixture.temp.path().join("reference-project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("reference.txt"), "synthetic reference content").unwrap();
    let (server, peer, connection, client, cookie) = start(&fixture).await;
    peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["cwd"] = json!(project);
    peer.reads.lock().await.get_mut(OTHER).unwrap()["thread"]["hostId"] = json!("remote");
    let history =
        rusqlite::Connection::open(fixture.paths.codex_home.join("state_5.sqlite")).unwrap();
    history.execute("INSERT INTO threads (id,title,cwd,history_mode,source) VALUES (?,'隔离远端聊天',?,'paginated','vscode')", rusqlite::params![OTHER, project.to_string_lossy()]).unwrap();
    drop(history);
    peer.projects.lock().await["projects"][0]["path"] = json!(project);
    let url = format!("http://{}/api/context", server.address);
    assert_eq!(client.get(&url).send().await.unwrap().status(), 401);
    assert_eq!(server.app.session.capabilities()["contextReferences"], true);
    assert_eq!(server.app.session.capabilities()["catalog"], false);
    for params in [
        vec![("threadId", CHAT), ("query", "reference")],
        vec![
            ("sourceThreadId", CHAT),
            ("projectId", "project"),
            ("query", "reference"),
        ],
    ] {
        let (status, value) =
            result(client.get(&url).header("cookie", &cookie).query(&params)).await;
        assert_eq!(status, 200, "{value}");
        assert_eq!(value["mode"], "references");
        assert_eq!(value["plugins"], json!([]));
        assert_eq!(value["browserTabs"], json!([]));
        assert_eq!(value["files"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["files"][0]["mention"],
            format!(
                "[reference.txt](<{}>)",
                project
                    .join("reference.txt")
                    .to_string_lossy()
                    .replace('\\', "/")
            )
        );
    }
    for (params, expected_status) in [
        (vec![("projectId", "project")], 400),
        (vec![("threadId", OTHER)], 403),
        (
            vec![("sourceThreadId", CHAT), ("projectId", "unrelated")],
            400,
        ),
        (
            vec![("sourceThreadId", OTHER), ("projectId", "project")],
            403,
        ),
    ] {
        let (status, _) = result(client.get(&url).header("cookie", &cookie).query(&params)).await;
        assert_eq!(status, expected_status);
    }
    let (status, _) = result(
        client
            .get(format!("http://{}/api/new-thread-settings", server.address))
            .header("cookie", &cookie),
    )
    .await;
    assert_eq!(status, 503);
    let calls = peer.calls.lock().await;
    assert!(calls.iter().all(|call| call["method"] == "tools/list"
        || ["read_thread", "list_projects"]
            .iter()
            .any(|tool| call["params"]["tool"] == *tool)));
    drop(calls);
    connection.close();
    assert_eq!(
        server.app.session.capabilities()["contextReferences"],
        false
    );
    let (status, _) = result(
        client
            .get(&url)
            .header("cookie", &cookie)
            .query(&[("threadId", CHAT)]),
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(
        std::fs::read_to_string(project.join("reference.txt")).unwrap(),
        "synthetic reference content"
    );
    server.stop().await.unwrap();
}
