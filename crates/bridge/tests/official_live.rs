use codex_mobile_bridge::{interactions, live::LiveState};
use serde_json::{json, Value};

fn user_message() -> Value {
    json!({"id":"user","type":"userMessage","content":[{"type":"text","text":"开始"}]})
}

fn response(items: Vec<Value>, status: &str) -> Value {
    json!({
        "thread":{"id":"chat","name":"原生聊天","cwd":"/workspace","status":{"type":if status == "inProgress" {"active"} else {"idle"},"activeFlags":[]}},
        "model":"gpt-6.1-sol","reasoningEffort":"max","cwd":"/workspace",
        "collaborationMode":{"mode":"default","settings":{"model":"gpt-6.1-sol","reasoning_effort":"max"}},
        "approvalPolicy":"never","approvalsReviewer":"user",
        "activePermissionProfile":{"id":":danger-full-access"},"sandbox":{"type":"dangerFullAccess"},
        "initialTurnsPage":{"data":[{"id":"turn","status":status,"startedAt":1794731992,"items":items,"itemsView":"full"}]}
    })
}

fn event(method: &str, params: Value) -> Value {
    let mut params = params;
    params["threadId"] = json!("chat");
    json!({"method":method,"params":params})
}

fn delta(text: &str) -> Value {
    event(
        "item/agentMessage/delta",
        json!({"turnId":"turn","itemId":"answer","delta":text}),
    )
}

fn answer(state: &LiveState) -> Value {
    state.snapshot()["turns"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|turn| turn["items"].as_array().unwrap())
        .find(|item| item["id"] == "answer")
        .unwrap()
        .clone()
}

#[test]
fn reconnect_keeps_seen_partial_until_full_completion_replaces_the_gap() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection-one",
            5,
        )
        .unwrap();
    assert!(state.apply(&delta("开头 / "), 6));
    // Full persisted history legitimately has no unfinished agentMessage.
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection-two",
            100,
        )
        .unwrap();
    assert_eq!(answer(&state)["text"], "开头 / ");
    assert!(state.apply(&delta("末尾"), 10));
    assert_eq!(answer(&state)["text"], "开头 / 末尾");
    let completed = json!({
        "id":"answer","type":"agentMessage","text":"开头 / 断线期间的内容 / 末尾",
        "phase":"final_answer","delivery":null,"questions":null
    });
    assert!(state.apply(
        &event("item/completed", json!({"turnId":"turn","item":completed})),
        11
    ));
    assert_eq!(answer(&state), completed);
    assert!(!state.apply(&delta("重复的尾巴"), 12));
    assert!(state.apply(&event("turn/completed", json!({"turn":{
        "id":"turn","status":"completed","items":[],"itemsView":"notLoaded","completedAt":1794732000
    }})), 13));
    assert_eq!(answer(&state), completed);
    assert_eq!(
        state.snapshot()["turns"][0]["completedAt"],
        1794732000000_i64
    );
    assert!(state.active_turn().is_none());
}

#[test]
fn received_snapshot_sequence_does_not_claim_to_contain_earlier_deltas() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            40,
        )
        .unwrap();
    assert!(state.apply(&delta("收到的前缀"), 20));
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            60,
        )
        .unwrap();
    assert_eq!(answer(&state)["text"], "收到的前缀");
    assert!(!state.apply(&delta("收到的前缀"), 20));
    assert!(state.apply(&delta(" + 后缀"), 50));
    assert_eq!(answer(&state)["text"], "收到的前缀 + 后缀");
}

#[test]
fn persisted_complete_item_survives_queued_start_and_delta_notifications() {
    let completed = json!({"id":"answer","type":"agentMessage","text":"最终答案"});
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message(), completed.clone()], "inProgress"),
            "connection",
            100,
        )
        .unwrap();
    assert!(!state.apply(
        &event(
            "item/started",
            json!({"turnId":"turn","item":{"id":"answer","type":"agentMessage","text":""}})
        ),
        70
    ));
    assert!(!state.apply(&delta("最终答案"), 80));
    assert_eq!(answer(&state), completed);
}

#[test]
fn first_attachment_shows_only_observed_suffix_until_the_full_item_arrives() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            100,
        )
        .unwrap();
    assert!(state.apply(&delta("已接收到的后缀"), 101));
    assert_eq!(answer(&state)["text"], "已接收到的后缀");
    let complete =
        json!({"id":"answer","type":"agentMessage","text":"之前的前缀 / 已接收到的后缀"});
    assert!(state.apply(
        &event("item/completed", json!({"turnId":"turn","item":complete})),
        102
    ));
    assert_eq!(answer(&state), complete);
}

#[test]
fn final_full_history_removes_obsolete_partial_items() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            1,
        )
        .unwrap();
    assert!(state.apply(&delta("partial"), 2));
    assert!(state.apply(
        &event(
            "item/agentMessage/delta",
            json!({"turnId":"turn","itemId":"obsolete","delta":"旧 draft"})
        ),
        3
    ));
    let complete = json!({"id":"answer","type":"agentMessage","text":"完整权威正文"});
    let final_items = vec![user_message(), complete];
    state
        .hydrate(
            &response(final_items.clone(), "completed"),
            "connection",
            10,
        )
        .unwrap();
    assert_eq!(state.snapshot()["turns"][0]["items"], json!(final_items));
    assert!(state.active_turn().is_none());
    assert!(!state.apply(
        &event(
            "item/agentMessage/delta",
            json!({"turnId":"turn","itemId":"obsolete","delta":"残余旧 draft"})
        ),
        4
    ));
}

#[test]
fn hydrating_another_thread_cannot_reuse_old_items_or_requests() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            1,
        )
        .unwrap();
    assert!(state.apply(&delta("旧聊天的正文"), 2));
    let mut other = response(vec![], "inProgress");
    other["thread"]["id"] = json!("other-chat");
    other["initialTurnsPage"]["data"][0]["id"] = json!("other-turn");
    state.hydrate(&other, "connection", 3).unwrap();
    assert_eq!(state.snapshot()["turns"].as_array().unwrap().len(), 1);
    assert!(state.snapshot()["turns"][0]["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!state.apply(&delta("不能跨聊天追加"), 4));
}

#[test]
fn typed_requests_do_not_collide_and_other_device_resolution_clears_only_the_original() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection-one",
            2,
        )
        .unwrap();
    let request = |id| {
        json!({
            "id":id,"method":"mcpServer/elicitation/request","params":{
                "threadId":"chat","turnId":"turn","serverName":"fixture","mode":"form",
                "message":"允许吗？","requestedSchema":{"type":"object","properties":{}}
            }
        })
    };
    state.server_request("connection-one", request(json!(731)));
    state.server_request("connection-one", request(json!("731")));
    let pending = state.pending();
    assert_eq!(pending.len(), 2);
    assert_ne!(pending[0]["id"], pending[1]["id"]);
    let old_id = pending[0]["id"].clone();
    assert!(state.apply(
        &event("serverRequest/resolved", json!({"requestId":731})),
        3
    ));
    assert_eq!(state.pending().len(), 1);
    assert_eq!(state.requests.values().next().unwrap()["id"], "731");
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection-two",
            30,
        )
        .unwrap();
    assert!(state.pending().is_empty());
    state.server_request("connection-two", request(json!(731)));
    assert_ne!(state.pending()[0]["id"], old_id);
    state.server_request("connection-one", request(json!(732)));
    assert_eq!(state.pending().len(), 1);
}

#[test]
fn official_async_questions_resolve_when_the_matching_user_answer_is_persisted() {
    let question = json!({"id":"question","type":"agentMessage","delivery":"async","text":"需要两个选择",
        "questions":[{"title":"部署到哪里？","options":["测试"]},{"title":"何时部署？","options":null}]});
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message(), question.clone()], "completed"),
            "connection",
            10,
        )
        .unwrap();
    let pending = state.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["questions"].as_array().unwrap().len(), 2);
    let first = pending[0]["questions"][0]["id"].as_str().unwrap();
    let second = pending[0]["questions"][1]["id"].clone();
    let input = json!({"answers":{first:["测试"]},"skipped":[second]});
    let body = interactions::async_reply(&state.turns, pending[0]["id"].as_str().unwrap(), &input)
        .unwrap();
    let mut saved = response(vec![user_message(), question], "completed");
    saved["initialTurnsPage"]["data"]
        .as_array_mut()
        .unwrap()
        .insert(
            0,
            json!({
                "id":"answer-turn","status":"completed","itemsView":"full","items":[{
                    "id":"reply","type":"userMessage","content":[{"type":"text","text":body}]
                }]
            }),
        );
    state.hydrate(&saved, "connection", 20).unwrap();
    let pending = state.pending();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["questions"].as_array().unwrap().len(), 1);
    assert_eq!(pending[0]["questions"][0]["id"], second);
}

#[test]
fn effective_settings_and_native_status_are_present_in_the_live_snapshot() {
    let mut state = LiveState::default();
    let hydrated = response(vec![user_message()], "inProgress");
    state.hydrate(&hydrated, "connection", 2).unwrap();
    let snapshot = state.snapshot();
    assert_eq!(snapshot["thread"]["model"], "gpt-6.1-sol");
    assert_eq!(snapshot["thread"]["reasoningEffort"], "max");
    assert_eq!(snapshot["thread"]["status"], hydrated["thread"]["status"]);
    assert_eq!(
        state.settings_view(vec![])["permissions"]["mode"],
        "full-access"
    );
    let mut settings = json!({
        "model":"gpt-6.1-sol","effort":"max","cwd":"/workspace",
        "collaborationMode":{"mode":"plan","settings":{"model":"gpt-6.1-sol","reasoning_effort":"max"}},
        "approvalPolicy":"never","approvalsReviewer":"user","sandboxPolicy":{"type":"dangerFullAccess"},
        "activePermissionProfile":{"id":":danger-full-access"}
    });
    assert!(state.apply(
        &event(
            "thread/settings/updated",
            json!({"threadSettings":settings})
        ),
        3
    ));
    assert_eq!(state.snapshot()["thread"]["mode"], "plan");
    settings["model"] = json!("updated-model");
    assert!(state.apply(
        &event(
            "thread/settings/updated",
            json!({"threadSettings":settings})
        ),
        4
    ));
    assert_eq!(state.snapshot()["thread"]["model"], "updated-model");
    assert_eq!(state.snapshot()["thread"]["reasoningEffort"], "max");
}

#[test]
fn metadata_snapshot_suppresses_old_metadata_but_keeps_content_and_request_resolution() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            10,
        )
        .unwrap();
    let mut old_settings = state.settings.clone();
    old_settings["model"] = json!("old-model");
    state.server_request(
        "connection",
        json!({"id":731,"method":"item/commandExecution/requestApproval","params":{
            "threadId":"chat","turnId":"turn","command":"fixture-command"
        }}),
    );
    let mut current = response(vec![user_message()], "inProgress");
    current["model"] = json!("current-model");
    current["collaborationMode"]["settings"]["model"] = json!("current-model");
    current["thread"]["name"] = json!("当前标题");
    state.hydrate(&current, "connection", 30).unwrap();
    assert!(!state.apply(
        &event(
            "thread/settings/updated",
            json!({"threadSettings":old_settings})
        ),
        20
    ));
    assert!(!state.apply(
        &event(
            "thread/status/changed",
            json!({"status":{"type":"active","activeFlags":["waitingOnApproval"]}})
        ),
        20
    ));
    assert!(!state.apply(
        &event("thread/name/updated", json!({"threadName":"旧标题"})),
        20
    ));
    assert_eq!(state.snapshot()["thread"]["model"], "current-model");
    assert_eq!(state.snapshot()["thread"]["title"], "当前标题");
    assert_eq!(
        state.snapshot()["thread"]["status"],
        current["thread"]["status"]
    );
    assert!(state.apply(&delta("快照未包含的正文"), 20));
    assert_eq!(answer(&state)["text"], "快照未包含的正文");
    assert!(state.apply(
        &event("serverRequest/resolved", json!({"requestId":731})),
        21
    ));
    assert!(state.pending().is_empty());
}

#[test]
fn each_metadata_notification_has_its_own_replay_watermark() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            10,
        )
        .unwrap();
    let mut latest_settings = state.settings.clone();
    latest_settings["model"] = json!("latest-model");
    latest_settings["effort"] = json!("low");
    let settings = event(
        "thread/settings/updated",
        json!({"threadSettings":latest_settings}),
    );
    let status = event("thread/status/changed", json!({"status":{"type":"idle"}}));
    let name = event("thread/name/updated", json!({"threadName":"最新标题"}));
    assert!(state.apply(&settings, 30));
    assert!(
        state.apply(&status, 20),
        "new settings must not suppress a newer status notification"
    );
    assert!(
        state.apply(&name, 25),
        "another metadata class must not suppress the title notification"
    );
    assert_eq!(state.snapshot()["thread"]["model"], "latest-model");
    assert_eq!(state.snapshot()["thread"]["reasoningEffort"], "low");
    assert_eq!(state.snapshot()["thread"]["status"]["type"], "idle");
    assert_eq!(state.snapshot()["thread"]["title"], "最新标题");
    assert!(!state.apply(&settings, 29));
    assert!(!state.apply(&status, 19));
    assert!(!state.apply(&name, 24));
    assert!(state.apply(&delta("元数据之后仍能回放正文"), 21));
    assert!(!state.apply(&delta("不能重复追加"), 21));
    assert_eq!(answer(&state)["text"], "元数据之后仍能回放正文");
}

#[test]
fn delayed_snapshot_refreshes_other_metadata_without_reverting_a_newer_class() {
    for newer_class in ["settings", "status", "name"] {
        let mut state = LiveState::default();
        state
            .hydrate(
                &response(vec![user_message()], "inProgress"),
                "connection",
                10,
            )
            .unwrap();
        let mut latest_settings = state.settings.clone();
        latest_settings["model"] = json!("latest-model");
        latest_settings["effort"] = json!("high");
        latest_settings["collaborationMode"]["mode"] = json!("plan");
        let newest = match newer_class {
            "settings" => event(
                "thread/settings/updated",
                json!({"threadSettings":latest_settings}),
            ),
            "status" => event("thread/status/changed", json!({"status":{"type":"idle"}})),
            "name" => event(
                "thread/name/updated",
                json!({"threadName":"通知里的最新标题"}),
            ),
            _ => unreachable!(),
        };
        assert!(state.apply(&newest, 30));
        let mut snapshot = response(vec![user_message()], "inProgress");
        snapshot["thread"]["name"] = json!("快照里的更新标题");
        snapshot["thread"]["status"] = json!({"type":"active","activeFlags":["waitingOnApproval"]});
        snapshot["model"] = json!("snapshot-model");
        snapshot["reasoningEffort"] = json!("low");
        snapshot["collaborationMode"]["settings"]["model"] = json!("snapshot-model");
        snapshot["collaborationMode"]["settings"]["reasoning_effort"] = json!("low");
        state.hydrate(&snapshot, "connection", 25).unwrap();
        let current = state.snapshot();
        assert_eq!(
            current["thread"]["model"],
            if newer_class == "settings" {
                "latest-model"
            } else {
                "snapshot-model"
            },
            "class: {newer_class}"
        );
        assert_eq!(
            current["thread"]["reasoningEffort"],
            if newer_class == "settings" {
                "high"
            } else {
                "low"
            },
            "class: {newer_class}"
        );
        assert_eq!(
            current["thread"]["mode"],
            if newer_class == "settings" {
                "plan"
            } else {
                "default"
            },
            "class: {newer_class}"
        );
        assert_eq!(
            current["thread"]["title"],
            if newer_class == "name" {
                "通知里的最新标题"
            } else {
                "快照里的更新标题"
            },
            "class: {newer_class}"
        );
        assert_eq!(
            current["thread"]["status"],
            if newer_class == "status" {
                json!({"type":"idle"})
            } else {
                snapshot["thread"]["status"].clone()
            },
            "class: {newer_class}"
        );
        assert!(!state.apply(
            &event("thread/name/updated", json!({"threadName":"更旧标题"})),
            24
        ));
        assert!(!state.apply(
            &event("thread/status/changed", json!({"status":{"type":"idle"}})),
            24
        ));
        let mut old_settings = state.settings.clone();
        old_settings["model"] = json!("stale-model");
        assert!(!state.apply(
            &event(
                "thread/settings/updated",
                json!({"threadSettings":old_settings})
            ),
            24
        ));
        assert!(state.apply(&delta("快照外的正文"), 24));
        assert_eq!(answer(&state)["text"], "快照外的正文");
    }
}

#[test]
fn metadata_already_observed_after_a_snapshot_remains_current_when_that_snapshot_finishes() {
    let mut state = LiveState::default();
    state
        .hydrate(
            &response(vec![user_message()], "inProgress"),
            "connection",
            10,
        )
        .unwrap();
    let mut observed = state.settings.clone();
    observed["model"] = json!("latest-model");
    assert!(state.apply(
        &event(
            "thread/settings/updated",
            json!({"threadSettings":observed})
        ),
        30
    ));
    assert!(state.apply(&delta("已观察到的正文"), 20));
    let mut older = response(vec![user_message()], "inProgress");
    older["model"] = json!("older-model");
    state.hydrate(&older, "connection", 25).unwrap();
    assert_eq!(state.snapshot()["thread"]["model"], "latest-model");
    assert_eq!(answer(&state)["text"], "已观察到的正文");
    let mut reconnected = response(vec![user_message()], "inProgress");
    reconnected["model"] = json!("reconnected-model");
    state.hydrate(&reconnected, "new-connection", 2).unwrap();
    assert_eq!(state.snapshot()["thread"]["model"], "reconnected-model");
    assert!(state.apply(&delta(" + 新连接正文"), 1));
    assert_eq!(answer(&state)["text"], "已观察到的正文 + 新连接正文");
}
