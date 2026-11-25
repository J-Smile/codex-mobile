use codex_mobile_bridge::{
    operations::{Claim, Operations},
    BridgeError,
};
use serde_json::json;

#[test]
fn creation_receipt_does_not_accept_first_turn_and_never_recreates() {
    let directory = tempfile::tempdir().unwrap();
    let store = Operations::open(directory.path()).unwrap();
    store
        .claim("owner", "create", "hash", "native", "new:project", "create")
        .unwrap();
    store
        .finish(
            "owner",
            "create",
            &Ok(json!({"status":"created","threadId":"chat","firstTurnAccepted":false})),
        )
        .unwrap();
    assert!(store.creation_pending("chat", None).unwrap());
    let Claim::Saved(Ok(receipt)) = store
        .claim(
            "owner",
            "create",
            "hash",
            "new-native",
            "new:project",
            "create",
        )
        .unwrap()
    else {
        panic!("不能重复创建");
    };
    assert_eq!(receipt["accepted"], false);
    assert_eq!(receipt["threadId"], "chat");
    assert!(!store
        .creation_pending("chat", Some("official-first-turn"))
        .unwrap());
    drop(store);
    let reopened = Operations::open(directory.path()).unwrap();
    assert!(!reopened.creation_pending("chat", None).unwrap());
}

#[test]
fn crash_after_claim_never_replays_even_on_a_new_runtime() {
    let directory = tempfile::tempdir().unwrap();
    let store = Operations::open(directory.path()).unwrap();
    assert!(matches!(
        store
            .claim("owner", "id", "hash", "runtime-one", "chat", "message")
            .unwrap(),
        Claim::Dispatch
    ));
    drop(store);
    let store = Operations::open(directory.path()).unwrap();
    assert!(matches!(
        store
            .claim("owner", "id", "hash", "runtime-two", "chat", "message")
            .unwrap(),
        Claim::Saved(Err(_))
    ));
    assert_eq!(store.pending("owner").unwrap().len(), 1);
    assert!(store
        .claim(
            "owner",
            "new-id",
            "new-content",
            "runtime-two",
            "chat",
            "message"
        )
        .is_err());
    assert!(store.pending("other").unwrap().is_empty());
    assert!(store.close_unknown("other", "id").is_err());
    store.close_unknown("owner", "id").unwrap();
    assert!(store.pending("owner").unwrap().is_empty());
    assert!(matches!(
        store
            .claim("owner", "id", "hash", "runtime-two", "chat", "message")
            .unwrap(),
        Claim::Saved(Err(_))
    ));
}

#[test]
fn second_service_cannot_recover_an_active_operation_store() {
    let directory = tempfile::tempdir().unwrap();
    let store = Operations::open(directory.path()).unwrap();
    store
        .claim("owner", "id", "hash", "runtime", "chat", "message")
        .unwrap();
    assert!(Operations::open(directory.path()).is_err());
    assert_eq!(store.pending("owner").unwrap()[0]["phase"], "dispatching");
    assert!(store.close_unknown("owner", "id").is_err());
    store.interrupted().unwrap();
    assert_eq!(store.pending("owner").unwrap()[0]["phase"], "unknown");
}

#[test]
fn retry_of_an_unsent_operation_cannot_bypass_a_later_unknown_result() {
    let directory = tempfile::tempdir().unwrap();
    let store = Operations::open(directory.path()).unwrap();
    store
        .claim("owner", "first", "hash-one", "runtime", "chat", "message")
        .unwrap();
    store
        .finish(
            "owner",
            "first",
            &Err(BridgeError::unavailable("offline").before_send()),
        )
        .unwrap();
    store
        .claim("owner", "second", "hash-two", "runtime", "chat", "message")
        .unwrap();
    store.interrupted().unwrap();
    assert!(store
        .claim("owner", "first", "hash-one", "runtime", "chat", "message")
        .is_err());
    store.close_unknown("owner", "second").unwrap();
    assert!(store.close_unknown("owner", "second").is_err());
    assert!(matches!(
        store
            .claim("owner", "first", "hash-one", "runtime", "chat", "message")
            .unwrap(),
        Claim::Dispatch
    ));
}

#[test]
fn only_proven_unsent_requests_retry_and_receipts_omit_content() {
    let directory = tempfile::tempdir().unwrap();
    let store = Operations::open(directory.path()).unwrap();
    store
        .claim("owner", "id", "hash", "runtime", "chat", "message")
        .unwrap();
    store
        .finish(
            "owner",
            "id",
            &Err(BridgeError::unavailable("offline").before_send()),
        )
        .unwrap();
    assert!(matches!(
        store
            .claim("owner", "id", "hash", "runtime", "chat", "message")
            .unwrap(),
        Claim::Dispatch
    ));
    store
        .finish(
            "owner",
            "id",
            &Ok(json!({"status":"queued","messageId":"id","input":"secret prompt"})),
        )
        .unwrap();
    let Claim::Saved(Ok(result)) = store
        .claim("owner", "id", "hash", "runtime", "chat", "message")
        .unwrap()
    else {
        panic!("missing receipt")
    };
    assert_eq!(result["status"], "queued");
    assert!(result.get("input").is_none());
    assert!(store
        .claim("owner", "id", "changed", "runtime", "chat", "message")
        .is_err());
}
