mod support;

use codex_mobile_bridge::{
    history::History,
    state::{chat_creation_input, summarize_item, summarize_snapshot},
};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use support::Fixture;

fn seed() -> Value {
    serde_json::from_str(include_str!(
        "../../../test/fixtures/official-create-thread-seed.json"
    ))
    .unwrap()
}

#[test]
fn official_creation_seed_stays_intact_in_live_summary() {
    let item = seed();
    assert_eq!(chat_creation_input(&item), Some("Codex Mobile Rust HTTP isolated settings and queue test. Reply only READY. Do not call any tools."));
    assert_eq!(summarize_item(&item), item);
    let snapshot = summarize_snapshot(json!({"turns":[{
        "id":"seed-turn","items":[item.clone(),{"id":"answer","type":"agentMessage","text":"READY"}]
    }]}));
    assert_eq!(snapshot["turns"][0]["items"][0], item);
    assert_eq!(snapshot["turns"][0]["items"].as_array().unwrap().len(), 2);
    assert_eq!(
        snapshot["turns"][0]["items"][0]["type"],
        "functionCallOutput"
    );
}

#[test]
fn official_followup_input_preserves_user_text_and_original_item_identity() {
    let mut item = seed();
    item["name"] = json!("send_message_to_thread");
    assert!(chat_creation_input(&item).is_some());
    assert_eq!(summarize_item(&item), item);
}

#[test]
fn unrelated_or_unrecognized_tool_outputs_remain_details() {
    let original = seed();
    let changes = [
        json!({"name":"read_file"}),
        json!({"namespace":"other_server"}),
        json!({"type":"dynamicToolCall"}),
        json!({"output":"ordinary create_thread tool result"}),
        json!({"output":format!("Example: {}",original["output"].as_str().unwrap())}),
        json!({"output":"<codex_delegation><input>request</input></codex_delegation>"}),
        json!({"output":"<codex_delegation><source_thread_id> </source_thread_id><input>request</input></codex_delegation>"}),
        json!({"output":"<codex_delegation><source_thread_id>control</source_thread_id><input> </input></codex_delegation>"}),
    ];
    for change in changes {
        let mut item = original.clone();
        item.as_object_mut()
            .unwrap()
            .extend(change.as_object().unwrap().clone());
        assert!(chat_creation_input(&item).is_none(), "{change}");
        let summary = summarize_item(&item);
        assert_eq!(summary["id"], original["id"]);
        assert_eq!(summary["type"], item["type"]);
        assert_eq!(summary["detailsAvailable"], true);
        assert!(summary.get("output").is_none());
    }
}

#[test]
fn persisted_history_keeps_creation_seed_visible_across_older_pages() {
    let fixture = Fixture::new();
    let connection =
        Connection::open(fixture.paths.codex_home.join("thread_history_1.sqlite")).unwrap();
    let original = seed();
    let mut unrelated = original.clone();
    unrelated["id"] = json!("unrelated-output");
    unrelated["output"] = json!("ordinary tool result");
    let mut followup = original.clone();
    followup["id"] = json!("followup-input");
    followup["name"] = json!("send_message_to_thread");
    for (offset, item) in [(0, &original), (1, &unrelated), (2, &followup)] {
        connection
            .execute(
                "INSERT INTO thread_items VALUES ('chat','turn-1',?,?,?,'functionCallOutput',?)",
                params![
                    item["id"].as_str().unwrap(),
                    10 + offset,
                    item.to_string(),
                    12 + offset
                ],
            )
            .unwrap();
    }
    let history = History::new(fixture.paths.codex_home.clone());
    let latest = history.page("chat", None, 1).unwrap();
    let older = history
        .page("chat", latest["nextCursor"].as_str(), 2)
        .unwrap();
    let items = older["turns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|turn| turn["items"].as_array().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        items
            .iter()
            .filter(|item| item["id"] == original["id"])
            .count(),
        1
    );
    assert_eq!(
        *items
            .iter()
            .find(|item| item["id"] == original["id"])
            .unwrap(),
        &original
    );
    let fallback = items
        .iter()
        .find(|item| item["id"] == "unrelated-output")
        .unwrap();
    assert_eq!(
        *items
            .iter()
            .find(|item| item["id"] == "followup-input")
            .unwrap(),
        &followup
    );
    assert_eq!(fallback["detailsAvailable"], true);
    assert!(fallback.get("output").is_none());
    assert_eq!(
        history
            .item("chat", original["id"].as_str().unwrap())
            .unwrap(),
        Some(original)
    );
}
