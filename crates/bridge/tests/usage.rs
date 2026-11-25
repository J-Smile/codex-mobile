mod support;
use codex_mobile_bridge::usage::{Usage, UsageQuery};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{io::Write, path::PathBuf};

fn vector(input: u64, output: u64) -> Value {
    json!({"input_tokens":input,"output_tokens":output,"total_tokens":input+output,"cached_input_tokens":0,"cache_write_input_tokens":0,"reasoning_output_tokens":0})
}
fn meta(id: &str) -> Value {
    json!({"type":"session_meta","timestamp":"2026-11-10T00:00:00Z","payload":{"id":id,"timestamp":"2026-11-10T00:00:00Z"}})
}
fn context(id: &str, model: &str) -> Value {
    json!({"type":"turn_context","payload":{"turn_id":id,"model":model}})
}
fn token(time: &str, total: Value, last: Value) -> Value {
    json!({"type":"event_msg","timestamp":time,"payload":{"type":"token_count","info":{"total_token_usage":total,"last_token_usage":last,"last_usage_includes_reasoning":true}}})
}
fn query() -> UsageQuery {
    UsageQuery::parse(
        "all",
        "Asia/Shanghai",
        "2026-11-11T04:00:00Z".parse().unwrap(),
    )
    .unwrap()
}
fn records(f: &support::Fixture, id: &str, values: &[Value]) -> PathBuf {
    let file = f
        .paths
        .codex_home
        .join("sessions")
        .join(format!("{id}.jsonl"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(
        &file,
        values.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    Connection::open(f.paths.codex_home.join("state_5.sqlite"))
        .unwrap()
        .execute(
            "INSERT OR REPLACE INTO threads(id,title,rollout_path,tokens_used) VALUES (?,?,?,1)",
            params![id, format!("样例 {id}"), file.to_string_lossy()],
        )
        .unwrap();
    file
}
fn fixture() -> support::Fixture {
    let f = support::Fixture::new();
    Connection::open(f.paths.codex_home.join("state_5.sqlite"))
        .unwrap()
        .execute("DELETE FROM threads", [])
        .unwrap();
    f
}

#[tokio::test]
async fn usage_persists_verified_calls_and_handles_partial_lines_without_counting_twice() {
    let f = fixture();
    let first = token("2026-11-10T14:00:00Z", vector(100, 20), vector(100, 20));
    let path = records(
        &f,
        "T1",
        &[
            meta("T1"),
            context("one", "M1"),
            first.clone(),
            first,
            context("two", "M2"),
            token("2026-11-10T16:01:00Z", vector(300, 50), vector(200, 30)),
        ],
    );
    let before = std::fs::read(&path).unwrap();
    let db_before = std::fs::read(f.paths.codex_home.join("state_5.sqlite")).unwrap();
    let service = Usage::open(f.paths.codex_home.clone(), f.paths.data_home.clone()).unwrap();
    service.refresh().await.unwrap();
    let data = service.overview(&query(), None).unwrap();
    assert_eq!(data["status"], "ready");
    assert_eq!(data["lifetime"]["totalTokens"], 350);
    assert_eq!(data["today"]["totalTokens"], 230);
    assert_eq!(data["daily"][0]["metrics"]["totalTokens"], 120);
    assert_eq!(data["models"][0]["model"], "M2");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::read(f.paths.codex_home.join("state_5.sqlite")).unwrap(),
        db_before
    );
    let suffix = format!(
        "{}\n{}\n",
        context("中文调用", "模型中文"),
        token("2026-11-11T02:00:00Z", vector(310, 55), vector(10, 5))
    );
    let bytes = suffix.as_bytes();
    let split = bytes.len() - 9;
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&bytes[..split])
        .unwrap();
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        350
    );
    service.close().await;
    drop(service);
    let service = Usage::open(f.paths.codex_home.clone(), f.paths.data_home.clone()).unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        350
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&bytes[split..])
        .unwrap();
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        365
    );
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        365
    );
    service.close().await;
}

#[tokio::test]
async fn copied_history_and_rewritten_sources_reconcile_without_losing_recorded_usage() {
    let f = fixture();
    let one = vec![
        meta("T1"),
        context("one", "M1"),
        token("2026-11-10T14:00:00Z", vector(100, 20), vector(100, 20)),
    ];
    let original = records(&f, "T1", &one);
    let copy = original.with_file_name("copy.jsonl");
    std::fs::copy(&original, &copy).unwrap();
    let service = Usage::open(f.paths.codex_home.clone(), f.paths.data_home.clone()).unwrap();
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        120
    );
    let mut child_meta = meta("CHILD");
    child_meta["payload"]["timestamp"] = json!("2026-11-11T00:00:00Z");
    child_meta["payload"]["forked_from_id"] = json!("T1");
    let mut child = vec![child_meta];
    child.extend(one);
    child.extend([
        context("reset", "M2"),
        token("2026-11-11T01:00:00Z", vector(2, 3), vector(2, 3)),
        context("next", "M2"),
        token("2026-11-11T02:00:00Z", vector(7, 8), vector(5, 5)),
    ]);
    records(&f, "CHILD", &child);
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        130
    );
    assert_eq!(
        service.overview(&query(), Some("CHILD")).unwrap()["lifetime"]["totalTokens"],
        10
    );
    std::fs::remove_file(&original).unwrap();
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        130
    );
    records(
        &f,
        "T1",
        &[
            meta("T1"),
            context("replacement", "M1"),
            token("2026-11-11T01:00:00Z", vector(1, 2), vector(1, 2)),
        ],
    );
    service.refresh().await.unwrap();
    assert_eq!(
        service.overview(&query(), None).unwrap()["lifetime"]["totalTokens"],
        133
    );
    let page = service.threads(&query(), 1, "", None).unwrap();
    assert_eq!(page["items"][0]["threadId"], "T1");
    let next = service
        .threads(&query(), 1, "", page["nextCursor"].as_str())
        .unwrap();
    assert_eq!(next["items"][0]["threadId"], "CHILD");
    service.close().await;
}

#[tokio::test]
async fn missing_history_and_cumulative_baselines_are_not_reported_as_zero_or_new_usage() {
    let f = fixture();
    let absent = records(&f, "MISSING", &[meta("MISSING")]);
    std::fs::remove_file(absent).unwrap();
    records(
        &f,
        "BASE",
        &[
            meta("BASE"),
            context("new", "M1"),
            token("2026-11-11T01:00:00Z", vector(1000, 200), vector(10, 20)),
            context("next", "M1"),
            token("2026-11-11T02:00:00Z", vector(1005, 207), vector(5, 7)),
        ],
    );
    let service = Usage::open(f.paths.codex_home.clone(), f.paths.data_home.clone()).unwrap();
    service.refresh().await.unwrap();
    let data = service.overview(&query(), None).unwrap();
    assert_eq!(data["lifetime"]["totalTokens"], 12);
    assert_eq!(data["status"], "partial");
    assert_eq!(data["coverage"]["gapEvents"], 1);
    let missing = service.overview(&query(), Some("MISSING")).unwrap();
    assert_eq!(missing["dataState"], "unavailable");
    assert!(missing["lifetime"].is_null());
    service.close().await;
}
