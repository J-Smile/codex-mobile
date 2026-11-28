use crate::control_regression::desktop;
use crate::test_support as support;

use codex_mobile_bridge::desktop::{Desktop, QueueChange};
use serde_json::{json, Value};
use std::sync::Arc;
use std::{sync::atomic::Ordering, time::Duration};
use support::{official_thread, Fixture, MockDesktop};

const THREAD: &str = "00000000-0000-4000-8000-000000000038";
const TURN: &str = "turn-00000000-0000-4000-8000-000000000038";

async fn live(client: &Arc<Desktop>) -> tokio::sync::watch::Receiver<crate::desktop::LiveView> {
    let stream = client.subscribe(THREAD);
    client.snapshot(THREAD).await.unwrap();
    stream
}

fn rows() -> Vec<Value> {
    vec![
        json!({"id":"queued-before","clientUserMessageId":"client-before","input":[{"type":"text","text":"前一条","text_elements":[]}]}),
        json!({"id":"queued-selected","clientUserMessageId":"client-selected","input":[
            {"type":"text","text":"已编辑的正文🙂","text_elements":[{"byteRange":{"start":0,"end":3},"placeholder":"原始标记"}]},
            {"type":"localImage","path":"/fixture/image.png"},
            {"type":"skill","name":"fixture-skill","path":"/fixture/SKILL.md"}
        ]}),
        json!({"id":"queued-after","clientUserMessageId":"client-after","input":[{"type":"text","text":"后一条","text_elements":[]}]}),
    ]
}

fn send_change(queue: &Value) -> QueueChange {
    QueueChange::parse(json!({
        "action":"send",
        "revision":queue["revision"],
        "messageId":"queued-selected"
    }))
    .unwrap()
}

async fn seed(mock: &MockDesktop, active: bool) -> Vec<Value> {
    mock.native_threads
        .lock()
        .await
        .push(support::native_thread(THREAD, json!("project")));
    mock.set_thread(THREAD, official_thread(THREAD, active))
        .await;
    let rows = rows();
    mock.queues.lock().await.insert(THREAD.into(), rows.clone());
    rows
}

async fn assert_no_interrupt_or_replay(mock: &MockDesktop) {
    assert_eq!(mock.count("turn/start").await, 0);
    assert_eq!(mock.count("turn/interrupt").await, 0);
    assert_eq!(mock.count("thread/queue/add").await, 0);
}

#[tokio::test]
async fn official_queue_start_rejects_active_and_pending_threads_without_consumption() {
    for active in [true, false] {
        let fixture = Fixture::new();
        let mock = MockDesktop::start_control_fixture(&fixture).await;
        let original = seed(&mock, active).await;
        if !active {
            mock.pending_turn_threads.lock().await.insert(THREAD.into());
        }
        let client = desktop(&fixture).await;
        let _stream = live(&client).await;
        let before = client.queue(THREAD).await.unwrap();
        let connection = client.session.connect().await.unwrap();
        let error = connection
            .rpc(
                "thread/queue/start",
                json!({"threadId":THREAD,"queuedSubmissionId":"queued-selected"}),
            )
            .await
            .unwrap_err();
        assert!(error.rpc_error.is_some());
        assert_eq!(
            error.message,
            "thread already has an active or pending turn"
        );
        assert_eq!(mock.queues.lock().await[THREAD], original);
        assert_eq!(client.queue(THREAD).await.unwrap(), before);
        assert_eq!(
            mock.threads.lock().await[THREAD]["thread"]["turns"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_no_interrupt_or_replay(&mock).await;
        client.close().await;
    }
}

#[tokio::test]
async fn active_queue_send_steers_original_input_and_consumes_only_selected_row() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, true).await;
    let global_state = fixture.queue();
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    let queue = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap();

    assert_ne!(queue["revision"], before["revision"]);
    assert_eq!(
        mock.queues.lock().await[THREAD],
        vec![original[0].clone(), original[2].clone()]
    );
    assert_eq!(queue["messages"][0]["id"], "queued-before");
    assert_eq!(queue["messages"][1]["id"], "queued-after");
    let calls = mock.calls.lock().await;
    let steer = calls
        .iter()
        .find(|call| call["method"] == "turn/steer")
        .unwrap();
    assert_eq!(
        steer["params"],
        json!({
            "threadId":THREAD,
            "expectedTurnId":TURN,
            "clientUserMessageId":original[1]["clientUserMessageId"],
            "input":original[1]["input"]
        })
    );
    let mutations = calls
        .iter()
        .filter_map(|call| {
            let method = call["method"].as_str()?;
            matches!(method, "turn/steer" | "thread/queue/delete").then_some(method)
        })
        .collect::<Vec<_>>();
    assert_eq!(mutations, ["turn/steer", "thread/queue/delete"]);
    drop(calls);
    let threads = mock.threads.lock().await;
    let turns = threads[THREAD]["thread"]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0]["id"], TURN);
    assert_eq!(turns[0]["status"], "inProgress");
    assert_eq!(
        turns[0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["clientId"] == "client-selected")
            .count(),
        1
    );
    drop(threads);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    assert_no_interrupt_or_replay(&mock).await;
    assert_eq!(fixture.queue(), global_state);
    client.close().await;
}

#[tokio::test]
async fn idle_queue_send_uses_atomic_official_start() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, false).await;
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap();
    assert_eq!(
        mock.queues.lock().await[THREAD],
        vec![original[0].clone(), original[2].clone()]
    );
    assert_eq!(mock.count("thread/queue/start").await, 1);
    assert_eq!(mock.count("turn/steer").await, 0);
    assert_eq!(mock.count("thread/queue/delete").await, 0);
    let threads = mock.threads.lock().await;
    let turns = threads[THREAD]["thread"]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 2);
    assert_eq!(turns[1]["items"][0]["content"], original[1]["input"]);
    assert_eq!(
        turns[1]["items"][0]["id"],
        original[1]["clientUserMessageId"]
    );
    drop(threads);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}

#[tokio::test]
async fn rejected_active_queue_send_preserves_revision_and_rows() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    mock.fail_next.store(true, Ordering::Release);
    let error = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap_err();
    assert!(error.rpc_error.is_some());
    assert_eq!(error.message, "fixture rejected");
    assert_eq!(client.queue(THREAD).await.unwrap(), before);
    assert_eq!(mock.queues.lock().await[THREAD], original);
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 0);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    assert_no_interrupt_or_replay(&mock).await;
    client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap();
    assert_eq!(mock.count("turn/steer").await, 2);
    assert_eq!(mock.count("thread/queue/delete").await, 1);
    assert_eq!(mock.queues.lock().await[THREAD].len(), 2);
    client.close().await;
}

#[tokio::test]
async fn rejected_queue_cleanup_retry_does_not_steer_the_message_twice() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    mock.reject_queue_delete.store(true, Ordering::Release);
    let error = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap_err();
    assert!(error.message.contains("消息已发送"));
    assert!(error.message.contains("fixture queue delete rejected"));
    assert!(!error.rpc_error.is_some());
    assert_eq!(client.queue(THREAD).await.unwrap(), before);
    assert_eq!(mock.queues.lock().await[THREAD], original);

    let edit = QueueChange::parse(json!({
        "action":"edit","revision":before["revision"],
        "messageId":"queued-selected","prompt":"不应再次提交"
    }))
    .unwrap();
    assert_eq!(
        client.update_queue(THREAD, edit).await.unwrap_err().status,
        409
    );
    assert_eq!(mock.count("thread/queue/update").await, 0);
    let queue = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap();
    assert_eq!(queue["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        mock.queues.lock().await[THREAD],
        vec![original[0].clone(), original[2].clone()]
    );
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 2);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    let threads = mock.threads.lock().await;
    let turns = threads[THREAD]["thread"]["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["clientId"] == "client-selected")
            .count(),
        1
    );
    drop(threads);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}

#[tokio::test]
async fn reopened_bridge_uses_official_client_id_receipt_to_retry_cleanup_only() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    mock.reject_queue_delete.store(true, Ordering::Release);
    assert!(client
        .update_queue(THREAD, send_change(&before))
        .await
        .is_err());
    client.close().await;
    drop(stream);
    drop(client);

    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let edit = QueueChange::parse(json!({
        "action":"edit","revision":before["revision"],
        "messageId":"queued-selected","prompt":"重建后也不应再提交"
    }))
    .unwrap();
    assert_eq!(
        client.update_queue(THREAD, edit).await.unwrap_err().status,
        409
    );
    assert_eq!(mock.count("thread/queue/update").await, 0);
    assert_eq!(mock.queues.lock().await[THREAD], original);
    let queue = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap();
    assert_eq!(queue["messages"].as_array().unwrap().len(), 2);
    assert_eq!(
        mock.queues.lock().await[THREAD],
        vec![original[0].clone(), original[2].clone()]
    );
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 2);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}

#[tokio::test]
async fn uncertain_steer_waits_for_official_receipt_and_never_blindly_resends() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let mut stream = live(&client).await;
    let original_owner = stream.borrow().owner.clone();
    let before = client.queue(THREAD).await.unwrap();
    mock.drop_next.store(true, Ordering::Release);
    let error = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap_err();
    assert!(!error.before_send);
    assert!(!error.rpc_error.is_some());

    // 模拟官方通知到达前，重连读取暂时尚未包含已接受消息。
    let accepted = {
        let mut threads = mock.threads.lock().await;
        let items = threads.get_mut(THREAD).unwrap()["thread"]["turns"][0]["items"]
            .as_array_mut()
            .unwrap();
        let index = items
            .iter()
            .position(|item| item["clientId"] == "client-selected")
            .unwrap();
        items.remove(index)
    };
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let view = stream.borrow().clone();
            if view.owner.is_some() && view.owner != original_owner && view.error.is_none() {
                break;
            }
            stream.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let error = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap_err();
    assert_eq!(error.status, 409);
    assert!(error.message.contains("仍待确认"));
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 0);
    assert_eq!(mock.queues.lock().await[THREAD], original);
    mock.threads.lock().await.get_mut(THREAD).unwrap()["thread"]["turns"][0]["items"]
        .as_array_mut()
        .unwrap()
        .push(accepted.clone());
    mock.broadcasts
        .send(json!({"method":"item/started","params":{
            "threadId":THREAD,"turnId":TURN,"item":accepted
        }}))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let observed = stream.borrow().snapshot.as_ref().is_some_and(|snapshot| {
                snapshot["turns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|turn| {
                        turn["items"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|item| item["clientId"] == "client-selected")
                    })
            });
            if observed {
                break;
            }
            stream.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap();
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 1);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}

#[tokio::test]
async fn stale_queue_send_never_dispatches_or_removes_changed_input() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let mut original = seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    original[1]["input"][0]["text"] = json!("另一设备的新正文");
    mock.queues
        .lock()
        .await
        .insert(THREAD.into(), original.clone());
    let error = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(mock.queues.lock().await[THREAD], original);
    assert_eq!(mock.count("turn/steer").await, 0);
    assert_eq!(mock.count("thread/queue/delete").await, 0);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}

#[tokio::test]
async fn concurrent_queue_send_delivers_selected_message_once() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    let (first, second) = tokio::join!(
        client.update_queue(THREAD, send_change(&before)),
        client.update_queue(THREAD, send_change(&before)),
    );
    assert!(first.is_ok() ^ second.is_ok());
    assert_eq!(first.err().or(second.err()).unwrap().status, 409);
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 1);
    assert_eq!(mock.queues.lock().await[THREAD].len(), 2);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}

#[tokio::test]
async fn changed_active_turn_rejects_steering_without_consuming_queue() {
    let fixture = Fixture::new();
    let mock = MockDesktop::start_control_fixture(&fixture).await;
    let original = seed(&mock, true).await;
    let client = desktop(&fixture).await;
    let _stream = live(&client).await;
    let before = client.queue(THREAD).await.unwrap();
    // 官方已进入新轮次，手机尚未收到通知时，必须由 expectedTurnId 拒绝旧请求。
    mock.threads.lock().await.get_mut(THREAD).unwrap()["thread"]["turns"][0]["id"] =
        json!("new-turn");
    let error = client
        .update_queue(THREAD, send_change(&before))
        .await
        .unwrap_err();
    assert!(error.rpc_error.is_some());
    assert_eq!(mock.queues.lock().await[THREAD], original);
    assert_eq!(mock.count("turn/steer").await, 1);
    assert_eq!(mock.count("thread/queue/delete").await, 0);
    assert_eq!(mock.count("thread/queue/start").await, 0);
    assert_no_interrupt_or_replay(&mock).await;
    client.close().await;
}
