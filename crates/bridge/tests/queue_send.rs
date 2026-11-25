mod support;
use codex_mobile_bridge::{
    desktop::{Desktop, QueueChange},
    history::History,
};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn unverified_queue_actions_never_steer_consume_or_recreate_rows() {
    let f = support::Fixture::new();
    let mock = support::MockDesktop::start(&f).await;
    let desktop = Desktop::new(
        f.paths.clone(),
        Arc::new(History::new(f.paths.codex_home.clone())),
    );
    for action in [
        json!({"action":"send","messageId":"queued"}),
        json!({"action":"delete","messageId":"queued"}),
        json!({"action":"edit","messageId":"queued","prompt":"changed"}),
        json!({"action":"reorder","messageIds":["queued"]}),
    ] {
        let mut change = action;
        change["revision"] = json!("a".repeat(64));
        assert!(
            desktop
                .update_queue("chat", QueueChange::parse(change).unwrap())
                .await
                .unwrap_err()
                .before_send
        );
    }
    assert!(
        desktop
            .send_message("chat", "message", "id", &[], &Default::default())
            .await
            .unwrap_err()
            .before_send
    );
    assert!(mock.calls.lock().await.is_empty());
    desktop.close().await;
}
