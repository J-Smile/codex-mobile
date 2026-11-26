use crate::test_support as support;
use codex_mobile_bridge::{
    app_server::AppServerConnection,
    config::MessageModel,
    desktop::{Desktop, QueueChange},
    history::History,
    session::Session,
};
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::{sync::Arc, time::Duration};
use support::{Fixture, MockDesktop};

async fn desktop(f: &Fixture) -> Arc<Desktop> {
    let session = Session::new(f.paths.clone());
    session.enable_shared_fixture();
    session.connect().await.unwrap();
    Desktop::with_session(
        f.paths.clone(),
        Arc::new(History::new(f.paths.codex_home.clone())),
        session,
    )
}
// 模拟官方实例先创建并登记聊天；手机只通过原连接读取和控制。
async fn create(desktop: &Arc<Desktop>, f: &Fixture, mock: &MockDesktop) -> String {
    let created = desktop
        .session
        .connect()
        .await
        .unwrap()
        .rpc(
            "thread/start",
            json!({"cwd":f.temp.path(),"historyMode":"legacy"}),
        )
        .await
        .unwrap();
    let id = created["thread"]["id"].as_str().unwrap().to_owned();
    let mut row = support::native_thread(&id, json!("project"));
    row["cwd"] = json!(f.temp.path());
    mock.native_threads.lock().await.push(row);
    id
}
async fn snapshot_matching(
    desktop: &Arc<Desktop>,
    id: &str,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    tokio::time::timeout(Duration::from_secs(4), async {
        let mut updates = desktop.subscribe(id);
        loop {
            let snapshot = desktop.snapshot(id).await.unwrap();
            if predicate(&snapshot) {
                return snapshot;
            }
            updates.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn official_instance_authorizes_registered_routes_and_reconnects_without_recreating() {
    let f = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&f).await;
    let desktop = desktop(&f).await;
    assert!(desktop.authorize("chat").await.unwrap_err().before_send);
    assert!(
        desktop
            .authorize("../escape")
            .await
            .unwrap_err()
            .before_send
    );
    let id = create(&desktop, &f, &mock).await;
    desktop.authorize(&id).await.unwrap();
    assert_eq!(
        desktop.metadata(&id).await.unwrap()["cwd"],
        json!(f.temp.path())
    );
    let connection = desktop.session.connect().await.unwrap();
    assert!(AppServerConnection::connect(&connection.url).await.is_err());
    let instance = connection.runtime_id();
    desktop.close().await;
    desktop.session.close().await;
    let next = Session::new(f.paths.clone());
    next.enable_shared_fixture();
    assert_eq!(next.connect().await.unwrap().runtime_id(), instance);
    assert_eq!(mock.count("thread/start").await, 1);
    assert_eq!(mock.count("turn/interrupt").await, 0);
    next.close().await;
    mock.wrong_home.store(true, Ordering::Release);
    let wrong = Session::new(f.paths.clone());
    wrong.enable_shared_fixture();
    assert!(wrong.connect().await.is_err());
    wrong.close().await;
}

#[tokio::test]
async fn controls_preserve_request_identity_and_queue_atomically_with_attachments() {
    let f = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&f).await;
    let desktop = desktop(&f).await;
    let id = create(&desktop, &f, &mock).await;
    let sent = desktop
        .send_message(
            &id,
            "合成内容",
            "synthetic-message-1",
            &[],
            &MessageModel {
                model: Some("test-model".into()),
                thinking: Some("high".into()),
            },
        )
        .await
        .unwrap();
    let turn = sent["turn"]["id"].as_str().unwrap();
    snapshot_matching(&desktop, &id, |s| {
        s["turns"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["id"] == turn)
    })
    .await;
    let start = mock
        .calls
        .lock()
        .await
        .iter()
        .find(|c| c["method"] == "turn/start")
        .unwrap()
        .clone();
    assert_eq!(start["params"]["model"], "test-model");
    assert_eq!(start["params"]["effort"], "high");
    for (request_id, method, params, answer) in [
        (
            json!(71),
            "item/tool/requestUserInput",
            json!({"questions":[{"id":"color","header":"颜色","question":"选择颜色","options":[{"label":"蓝","description":"合成"}]}]}),
            json!({"answers":{"color":["蓝"]}}),
        ),
        (
            json!("approval"),
            "item/commandExecution/requestApproval",
            json!({"command":"echo synthetic","reason":"验收"}),
            json!({"decision":"decline"}),
        ),
    ] {
        let mut params = params;
        params["threadId"] = json!(id);
        params["turnId"] = json!(turn);
        mock.server_request(json!({"id":request_id,"method":method,"params":params}))
            .await;
        let snapshot = snapshot_matching(&desktop, &id, |s| {
            !s["interactions"].as_array().unwrap().is_empty()
        })
        .await;
        let key = snapshot["interactions"][0]["id"].as_str().unwrap();
        assert_eq!(
            desktop.reply(&id, key, &answer).await.unwrap()["resolved"],
            true
        );
        assert!(
            desktop
                .reply(&id, key, &answer)
                .await
                .unwrap_err()
                .before_send
        );
        let calls = mock.calls.lock().await;
        let replies: Vec<_> = calls
            .iter()
            .filter(|c| c["id"] == request_id && c.get("method").is_none())
            .collect();
        assert_eq!(replies.len(), 1);
    }
    let image = json!({"localPath":f.temp.path().join("synthetic.png")});
    desktop
        .enqueue(&id, "队列一", "synthetic-queue-one", &[image])
        .await
        .unwrap();
    desktop
        .enqueue(&id, "队列二", "synthetic-queue-two", &[])
        .await
        .unwrap();
    let mut queue = desktop.queue(&id).await.unwrap();
    let one = queue["messages"][0]["id"].clone();
    let two = queue["messages"][1]["id"].clone();
    let original = queue["revision"].clone();
    queue=desktop.update_queue(&id,QueueChange::parse(json!({"revision":queue["revision"],"action":"edit","messageId":one,"prompt":"修改内容"})).unwrap()).await.unwrap();
    assert_eq!(queue["messages"][0]["attachmentCount"], 1);
    assert_eq!(
        desktop
            .update_queue(
                &id,
                QueueChange::parse(json!({"revision":original,"action":"delete","messageId":one}))
                    .unwrap()
            )
            .await
            .unwrap_err()
            .status,
        409
    );
    queue = desktop
        .update_queue(
            &id,
            QueueChange::parse(
                json!({"revision":queue["revision"],"action":"reorder","messageIds":[two,one]}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(queue["messages"][0]["id"], two);
    assert_eq!(
        desktop
            .interrupt(&id, "stale-turn")
            .await
            .unwrap_err()
            .status,
        409
    );
    desktop.interrupt(&id, turn).await.unwrap();
    snapshot_matching(&desktop, &id, |s| {
        s["turns"].as_array().unwrap().last().unwrap()["status"] == "interrupted"
    })
    .await;
    assert_eq!(
        desktop.resume_turn(&id, turn).await.unwrap_err().status,
        409
    );
    queue = desktop
        .update_queue(
            &id,
            QueueChange::parse(
                json!({"revision":queue["revision"],"action":"send","messageId":two}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(mock.count("thread/queue/start").await, 1);
    assert_eq!(mock.count("turn/start").await, 1);
    assert_eq!(queue["messages"].as_array().unwrap().len(), 1);
    desktop
        .update_queue(
            &id,
            QueueChange::parse(
                json!({"revision":queue["revision"],"action":"delete","messageId":one}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert!(desktop.queue(&id).await.unwrap()["messages"]
        .as_array()
        .unwrap()
        .is_empty());
    let active = snapshot_matching(&desktop, &id, |s| {
        s["turns"].as_array().unwrap().last().unwrap()["status"] == "inProgress"
    })
    .await;
    let last = active["turns"].as_array().unwrap().last().unwrap()["id"]
        .as_str()
        .unwrap();
    desktop.interrupt(&id, last).await.unwrap();
    snapshot_matching(&desktop, &id, |s| {
        s["turns"].as_array().unwrap().last().unwrap()["status"] == "interrupted"
    })
    .await;
    desktop.resume_turn(&id, last).await.unwrap();
    assert_eq!(mock.count("turn/start").await, 2);
    assert_eq!(mock.count("turn/resume").await, 0);
    desktop.close().await;
    desktop.session.close().await;
}

#[tokio::test]
async fn reconnect_revokes_old_request_keys_without_submitting_to_a_new_generation() {
    let f = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&f).await;
    let desktop = desktop(&f).await;
    let id = create(&desktop, &f, &mock).await;
    desktop.snapshot(&id).await.unwrap();
    mock.server_request(json!({"id":99,"method":"item/fileChange/requestApproval","params":{"threadId":id,"turnId":"synthetic-turn","reason":"合成审批"}})).await;
    let snapshot = snapshot_matching(&desktop, &id, |s| {
        !s["interactions"].as_array().unwrap().is_empty()
    })
    .await;
    let old = snapshot["interactions"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let generation = desktop.session.connect().await.unwrap().id.clone();
    let mut updates = desktop.subscribe(&id);
    mock.disconnect();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            if desktop.session.connect().await.unwrap().id != generation {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let view = updates.borrow().clone();
            if view
                .owner
                .as_ref()
                .is_some_and(|owner| owner != &generation)
                && view.error.is_none()
            {
                assert!(view.snapshot.unwrap()["interactions"]
                    .as_array()
                    .unwrap()
                    .is_empty());
                break;
            }
            updates.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(
        desktop
            .reply(&id, &old, &json!({"decision":"accept"}))
            .await
            .unwrap_err()
            .before_send
    );
    assert_eq!(
        mock.calls
            .lock()
            .await
            .iter()
            .filter(|call| call["id"] == 99 && call.get("method").is_none())
            .count(),
        0
    );
    desktop.close().await;
    desktop.session.close().await;
}

#[tokio::test]
async fn queue_rejects_unowned_threads_and_malformed_edit_before_writing() {
    let f = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&f).await;
    let desktop = desktop(&f).await;
    assert!(desktop.queue("chat").await.unwrap_err().before_send);
    assert_eq!(mock.count("thread/queue/list").await, 0);
    let id = create(&desktop, &f, &mock).await;
    mock.queues
        .lock()
        .await
        .insert(id.clone(), vec![json!({"id":"broken","input":null})]);
    let queue = desktop.queue(&id).await.unwrap();
    let failure = desktop.update_queue(&id, QueueChange::parse(json!({
        "revision":queue["revision"],"action":"edit","messageId":"broken","prompt":"保留原附件"
    })).unwrap()).await.unwrap_err();
    assert!(failure.before_send);
    assert_eq!(mock.count("thread/queue/update").await, 0);
    desktop.close().await;
    desktop.session.close().await;
}
