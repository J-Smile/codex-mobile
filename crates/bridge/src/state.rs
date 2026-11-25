use crate::{BridgeError, Result};
use serde_json::{json, Value};

pub fn select(value: &Value, keys: &[&str]) -> Value {
    Value::Object(
        keys.iter()
            .filter_map(|key| value.get(*key).map(|v| ((*key).into(), v.clone())))
            .collect(),
    )
}

pub fn permissions(mode: &str) -> Result<Value> {
    let (profile, policy) = match mode {
        "read-only" => (":read-only", "on-request"),
        "workspace-write" => (":workspace", "on-request"),
        "full-access" => (":danger-full-access", "never"),
        _ => return Err(BridgeError::invalid("请选择有效的访问权限。")),
    };
    Ok(json!({"permissions":profile,"approvalPolicy":policy,"approvalsReviewer":"user"}))
}

pub fn read_permissions(settings: &Value) -> Value {
    let profile = settings
        .get("permissions")
        .unwrap_or(&settings["activePermissionProfile"]["id"]);
    let matching = match profile.as_str() {
        Some(":read-only") => Some(("read-only", "readOnly", "on-request")),
        Some(":workspace") => Some(("workspace-write", "workspaceWrite", "on-request")),
        Some(":danger-full-access") => Some(("full-access", "dangerFullAccess", "never")),
        _ => None,
    };
    let sandbox = matching
        .map(|(_, s, _)| Value::String(s.into()))
        .unwrap_or_else(|| settings["sandboxPolicy"]["type"].clone());
    let entry = matching.or_else(|| {
        if profile.is_null() {
            match sandbox.as_str() {
                Some("readOnly") => Some(("read-only", "readOnly", "on-request")),
                Some("workspaceWrite") => Some(("workspace-write", "workspaceWrite", "on-request")),
                Some("dangerFullAccess") => Some(("full-access", "dangerFullAccess", "never")),
                _ => None,
            }
        } else {
            None
        }
    });
    let reviewer = &settings["approvalsReviewer"];
    let policy = &settings["approvalPolicy"];
    let mode = entry
        .filter(|(_, _, p)| {
            (reviewer.is_null() || reviewer == "user") && (policy.is_null() || policy == *p)
        })
        .map(|(m, _, _)| m)
        .unwrap_or("custom");
    let mut result = json!({"mode":mode,"profile":profile});
    if !sandbox.is_null() {
        result["sandboxType"] = sandbox;
    }
    result
}

pub fn chat_creation_input(item: &Value) -> Option<&str> {
    if item["type"] != "functionCallOutput"
        || (item["name"] != "create_thread" && item["name"] != "send_message_to_thread")
        || item["namespace"] != "codex_app"
    {
        return None;
    }
    let delegation = item["output"]
        .as_str()?
        .trim()
        .strip_prefix("<codex_delegation>")?
        .strip_suffix("</codex_delegation>")?
        .trim();
    let (source, request) = delegation
        .strip_prefix("<source_thread_id>")?
        .split_once("</source_thread_id>")?;
    if source.trim().is_empty() || source.contains(['<', '>']) {
        return None;
    }
    let input = request
        .trim()
        .strip_prefix("<input>")?
        .strip_suffix("</input>")?;
    (!input.trim().is_empty()).then_some(input)
}

pub fn summarize_item(item: &Value) -> Value {
    // 官方原生创建和追加使用 delegation 条目承载输入，保留原文供消息渲染。
    if item["detailsAvailable"] == true || chat_creation_input(item).is_some() {
        return item.clone();
    }
    match item["type"].as_str().unwrap_or("") {
        "userMessage" | "agentMessage" | "plan" => select(
            item,
            &[
                "id",
                "type",
                "text",
                "phase",
                "content",
                "clientId",
                "pending",
                "questions",
                "contentSource",
            ],
        ),
        "reasoning" => {
            json!({"id":item["id"],"type":"reasoning","summary":item.get("summary").cloned().unwrap_or(json!([]))})
        }
        _ => {
            let mut result = select(
                item,
                &[
                    "id",
                    "type",
                    "status",
                    "tool",
                    "server",
                    "name",
                    "path",
                    "exitCode",
                    "durationMs",
                    "startedAtMs",
                    "completedAtMs",
                    "kind",
                    "agentThreadId",
                    "agentPath",
                    "model",
                    "reasoningEffort",
                    "receiverThreadIds",
                ],
            );
            if let Some(command) = item["command"].as_str() {
                result["command"] = json!(command.chars().take(1200).collect::<String>());
            }
            if let Some(actions) = item["commandActions"].as_array() {
                result["commandActions"] = Value::Array(
                    actions
                        .iter()
                        .map(|action| {
                            let mut a = action.clone();
                            if let Some(command) = action["command"].as_str() {
                                a["command"] = json!(command.chars().take(300).collect::<String>());
                            }
                            a
                        })
                        .collect(),
                );
            }
            if !item["arguments"]["title"].is_null() {
                result["arguments"] = json!({"title":item["arguments"]["title"]});
            }
            if let Some(changes) = item["changes"].as_array() {
                result["changes"] = Value::Array(
                    changes
                        .iter()
                        .map(|change| {
                            let mut c = select(change, &["path", "kind"]);
                            if let Some(diff) = change["diff"]
                                .as_str()
                                .or_else(|| change["diff"]["text"].as_str())
                            {
                                let lines: Vec<&str> = diff.lines().collect();
                                let kind = change["kind"]
                                    .as_str()
                                    .or_else(|| change["kind"]["type"].as_str())
                                    .unwrap_or("");
                                let unified = lines.iter().any(|line| line.starts_with("@@ "));
                                c["additions"] = json!(if !unified && kind == "add" {
                                    lines.len()
                                } else if !unified && kind == "delete" {
                                    0
                                } else {
                                    lines
                                        .iter()
                                        .filter(|line| {
                                            line.starts_with('+') && !line.starts_with("+++ ")
                                        })
                                        .count()
                                });
                                c["deletions"] = json!(if !unified && kind == "delete" {
                                    lines.len()
                                } else if !unified && kind == "add" {
                                    0
                                } else {
                                    lines
                                        .iter()
                                        .filter(|line| {
                                            line.starts_with('-') && !line.starts_with("--- ")
                                        })
                                        .count()
                                });
                            }
                            c
                        })
                        .collect(),
                );
            }
            result["detailsAvailable"] = json!(true);
            result["detailVersion"] = item
                .get("detailVersion")
                .cloned()
                .unwrap_or_else(|| json!(&crate::digest(item.to_string())[..16]));
            result
        }
    }
}

pub fn summarize_snapshot(mut snapshot: Value) -> Value {
    if let Some(turns) = snapshot["turns"].as_array_mut() {
        for turn in turns {
            turn["items"] = Value::Array(
                turn["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|i| {
                        i["type"] != "reasoning"
                            || i["summary"].as_array().is_some_and(|s| !s.is_empty())
                    })
                    .map(summarize_item)
                    .collect(),
            );
        }
    }
    snapshot
}

pub fn text_append(previous: &Value, item: &Value, min_offset: usize) -> Option<Value> {
    if item["type"] != "agentMessage" {
        return None;
    }
    let before = previous["text"].as_str()?;
    let after = item["text"].as_str()?;
    let offset = before.encode_utf16().count();
    if offset < min_offset || after.len() <= before.len() || !after.starts_with(before) {
        return None;
    }
    let mut old = previous.clone();
    let mut new = item.clone();
    old.as_object_mut()?.shift_remove("text");
    new.as_object_mut()?.shift_remove("text");
    (old == new).then(|| json!({"itemId":item["id"],"offset":offset,"text":&after[before.len()..]}))
}
