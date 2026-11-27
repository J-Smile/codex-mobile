mod support;
use codex_mobile_bridge::{goals::Goals, session::Session};
use serde_json::json;

#[tokio::test]
async fn goals_are_unavailable_without_the_official_tools_contract() {
    let f = support::Fixture::new();
    let mock = support::MockDesktop::start(&f).await;
    mock.missing_desktop_tools
        .store(true, std::sync::atomic::Ordering::Release);
    let session = Session::new(f.paths.clone());
    let goals = Goals::with_session(session.clone());
    assert!(
        goals
            .request("chat", "thread/goal/get", json!({}))
            .await
            .unwrap_err()
            .before_send
    );
    for method in ["thread/goal/set", "thread/goal/clear"] {
        assert!(
            goals
                .request("chat", method, json!({"status":"active"}))
                .await
                .unwrap_err()
                .before_send
        );
        assert_eq!(mock.count(method).await, 0);
    }
    goals.close().await;
    assert!(session.connect().await.is_err());
    session.close().await;
}
#[tokio::test]
async fn goals_without_runtime_do_not_create_one() {
    let f = support::Fixture::new();
    let goals = Goals::new(f.paths.clone());
    assert!(goals.connect().await.is_err());
    assert!(std::fs::read_dir(&f.paths.runtime_home)
        .unwrap()
        .next()
        .is_none());
}
