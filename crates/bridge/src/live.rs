//! Convert official App Server snapshots and notifications into the mobile view.
use crate::{interactions, state, BridgeError, Result};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::collections::HashSet;

#[derive(Clone, Default, Debug)]
pub struct LiveState {
    pub generation: String,
    pub sequence: u64,
    pub thread: Value,
    pub settings: Value,
    pub turns: Vec<Value>,
    pub requests: IndexMap<String, Value>,
    snapshot_sequence: u64,
    status_sequence: u64,
    name_sequence: u64,
    settings_sequence: u64,
    observed_items: HashSet<(String, String)>,
    completed_items: HashSet<(String, String)>,
}

pub fn thread_metadata(thread: &Value) -> Value {
    let mut result = state::select(
        thread,
        &[
            "id",
            "cwd",
            "model",
            "reasoningEffort",
            "status",
            "canAcceptDirectInput",
        ],
    );
    result["title"] = thread
        .get("name")
        .filter(|v| !v.is_null())
        .or_else(|| thread.get("title"))
        .cloned()
        .unwrap_or_else(|| thread["preview"].clone());
    // Desktop project IDs are supplied by its official catalog. App Server's
    // canonical project IDs belong to a different namespace.
    result
}

pub fn turn_view(turn: &Value) -> Value {
    let mut result = turn.clone();
    for key in ["startedAt", "completedAt"] {
        if let Some(seconds) = turn[key].as_i64() {
            result[key] = json!(seconds.saturating_mul(1000));
        }
    }
    result
}

impl LiveState {
    pub fn hydrate(&mut self, response: &Value, generation: &str, sequence: u64) -> Result<()> {
        if !response["thread"]["id"].is_string() {
            return Err(BridgeError::unavailable("Codex 未返回聊天快照。"));
        }
        let same_thread = self.thread["id"] == response["thread"]["id"];
        let previous = if same_thread {
            std::mem::take(&mut self.turns)
        } else {
            self.turns.clear();
            self.observed_items.clear();
            self.completed_items.clear();
            self.requests.clear();
            Vec::new()
        };
        let reset = !same_thread || self.generation != generation;
        if reset {
            self.sequence = 0;
            self.snapshot_sequence = 0;
            self.status_sequence = 0;
            self.name_sequence = 0;
            self.settings_sequence = 0;
            self.requests.clear();
        }
        self.generation = generation.into();
        // A response's receive sequence orders transport messages. Persisted
        // history can still omit agent text received before that response.
        // Only applied events advance the content replay watermark.
        if reset || sequence >= self.snapshot_sequence {
            let mut thread = thread_metadata(&response["thread"]);
            if sequence < self.status_sequence {
                thread["status"] = self.thread["status"].clone();
            } else {
                self.status_sequence = sequence;
            }
            if sequence < self.name_sequence {
                thread["title"] = self.thread["title"].clone();
            } else {
                self.name_sequence = sequence;
            }
            self.thread = thread;
            self.snapshot_sequence = sequence;
        }
        // Notifications for settings, status and name are independent. A
        // newer event in one class must not block this snapshot's other data.
        if reset || sequence >= self.settings_sequence {
            self.settings = state::select(
                response,
                &[
                    "model",
                    "reasoningEffort",
                    "collaborationMode",
                    "approvalPolicy",
                    "approvalsReviewer",
                    "activePermissionProfile",
                    "cwd",
                ],
            );
            self.settings["sandboxPolicy"] = response["sandbox"].clone();
            self.settings["effort"] = response["reasoningEffort"].clone();
            self.settings_sequence = sequence;
        }
        self.sync_thread_settings();
        let incoming: Vec<Value> =
            if let Some(turns) = response["initialTurnsPage"]["data"].as_array() {
                turns.iter().rev().map(turn_view).collect()
            } else {
                response["thread"]["turns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(turn_view)
                    .collect()
            };
        for turn in incoming {
            if turn["status"] != "inProgress" {
                let turn_id = turn["id"].as_str().unwrap_or("");
                self.completed_items.extend(
                    self.observed_items
                        .iter()
                        .filter(|(id, _)| id == turn_id)
                        .cloned(),
                );
            }
            if turn["itemsView"] != "notLoaded" && turn["itemsView"] != "summary" {
                for item in turn["items"].as_array().into_iter().flatten() {
                    if item["type"] == "agentMessage" || item_finished(item) {
                        self.completed_items
                            .insert(item_key(&turn["id"], &item["id"]));
                    }
                }
            }
            let turn = previous
                .iter()
                .find(|old| old["id"] == turn["id"])
                .map(|old| merge_turn(turn.clone(), old, &self.observed_items, true))
                .unwrap_or(turn);
            self.turns.push(turn);
        }
        let active = self.thread["status"] == "active" || self.thread["status"]["type"] == "active";
        if active && !self.turns.iter().any(|turn| turn["status"] == "inProgress") {
            for old in previous
                .into_iter()
                .filter(|turn| turn["status"] == "inProgress")
            {
                if !self.turns.iter().any(|turn| turn["id"] == old["id"]) {
                    self.turns.push(old);
                }
            }
        }
        self.requests.retain(|_, request| {
            !self.turns.iter().any(|turn| {
                turn["id"] == request["params"]["turnId"] && turn["status"] != "inProgress"
            })
        });
        self.trim();
        Ok(())
    }
    fn sync_thread_settings(&mut self) {
        for key in ["model", "cwd"] {
            if let Some(value) = self.settings.get(key) {
                self.thread[key] = value.clone();
            }
        }
        if let Some(value) = self
            .settings
            .get("effort")
            .or_else(|| self.settings.get("reasoningEffort"))
        {
            self.thread["reasoningEffort"] = value.clone();
        }
    }
    fn trim(&mut self) {
        let excess = self.turns.len().saturating_sub(5);
        self.turns.drain(..excess);
        let retained: HashSet<String> = self
            .turns
            .iter()
            .filter_map(|turn| turn["id"].as_str().map(str::to_owned))
            .collect();
        self.observed_items
            .retain(|(turn, _)| retained.contains(turn));
        self.completed_items
            .retain(|(turn, _)| retained.contains(turn));
    }
    fn turn(&mut self, id: &str) -> &mut Value {
        let index = self
            .turns
            .iter()
            .position(|t| t["id"] == id)
            .unwrap_or_else(|| {
                self.turns
                    .push(json!({"id":id,"status":"inProgress","items":[]}));
                self.turns.len() - 1
            });
        &mut self.turns[index]
    }
    fn item(&mut self, turn: &str, id: &str, kind: &str) -> &mut Value {
        let turn = self.turn(turn);
        if !turn["items"].is_array() {
            turn["items"] = json!([]);
        }
        let items = turn["items"].as_array_mut().unwrap();
        let index = items.iter().position(|i| i["id"] == id).unwrap_or_else(|| {
            items.push(json!({"id":id,"type":kind}));
            items.len() - 1
        });
        &mut items[index]
    }
    pub fn server_request(&mut self, generation: &str, request: Value) {
        if generation != self.generation || request["params"]["threadId"] != self.thread["id"] {
            return;
        }
        let key = crate::digest(json!([generation, request["id"]]).to_string());
        self.requests.insert(key, request);
    }
    pub fn apply(&mut self, message: &Value, sequence: u64) -> bool {
        let p = &message["params"];
        if p["threadId"] != self.thread["id"] {
            return false;
        }
        let method = message["method"].as_str().unwrap_or("");
        let applied = match method {
            "thread/status/changed" => self.status_sequence,
            "thread/name/updated" => self.name_sequence,
            "thread/settings/updated" => self.settings_sequence,
            _ => self.sequence,
        };
        if sequence <= applied {
            return false;
        }
        match method {
            "serverRequest/resolved" => self
                .requests
                .retain(|_, request| request["id"] != p["requestId"]),
            "thread/status/changed" => self.thread["status"] = p["status"].clone(),
            "thread/name/updated" => self.thread["title"] = p["threadName"].clone(),
            "thread/settings/updated" => {
                self.settings = p["threadSettings"].clone();
                self.sync_thread_settings();
            }
            "turn/started" | "turn/completed" => {
                let id = p["turn"]["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return false;
                }
                let previous = self.turn(id).clone();
                let incoming = merge_turn(
                    turn_view(&p["turn"]),
                    &previous,
                    &self.observed_items,
                    false,
                );
                *self.turn(id) = incoming;
                if method == "turn/completed" {
                    self.requests
                        .retain(|_, request| request["params"]["turnId"] != id);
                    let completed: Vec<_> = self.turn(id)["items"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|item| item_key(&json!(id), &item["id"]))
                        .collect();
                    self.completed_items.extend(completed);
                }
                self.trim();
            }
            "item/started" | "item/completed" => {
                let Some(turn) = p["turnId"].as_str() else {
                    return false;
                };
                let Some(id) = p["item"]["id"].as_str() else {
                    return false;
                };
                let key = (turn.to_owned(), id.to_owned());
                if method == "item/started" && self.completed_items.contains(&key) {
                    return false;
                }
                let observed = self.observed_items.contains(&key);
                let item = self.item(turn, id, p["item"]["type"].as_str().unwrap_or(""));
                let partial = if method == "item/started" && observed {
                    state::select(
                        item,
                        &["text", "aggregatedOutput", "output", "summary", "content"],
                    )
                } else {
                    json!({})
                };
                *item = p["item"].clone();
                for (field, value) in partial.as_object().unwrap() {
                    item[field] = value.clone();
                }
                self.observed_items.insert(key.clone());
                if method == "item/completed" {
                    self.completed_items.insert(key);
                }
            }
            "item/agentMessage/delta"
            | "item/plan/delta"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta" => {
                let Some(turn) = p["turnId"].as_str() else {
                    return false;
                };
                let Some(id) = p["itemId"].as_str() else {
                    return false;
                };
                let key = (turn.to_owned(), id.to_owned());
                if self.completed_items.contains(&key) {
                    return false;
                }
                self.observed_items.insert(key);
                let (kind, field) = match method {
                    "item/agentMessage/delta" => ("agentMessage", "text"),
                    "item/plan/delta" => ("plan", "text"),
                    "item/commandExecution/outputDelta" => ("commandExecution", "aggregatedOutput"),
                    _ => ("fileChange", "output"),
                };
                let item = self.item(turn, id, kind);
                let mut text = item[field].as_str().unwrap_or("").to_string();
                text.push_str(p["delta"].as_str().unwrap_or(""));
                item[field] = json!(text);
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let Some(turn) = p["turnId"].as_str() else {
                    return false;
                };
                let Some(id) = p["itemId"].as_str() else {
                    return false;
                };
                let key = (turn.to_owned(), id.to_owned());
                if self.completed_items.contains(&key) {
                    return false;
                }
                let (field, index) = if method == "item/reasoning/summaryTextDelta" {
                    ("summary", p["summaryIndex"].as_u64().unwrap_or(0))
                } else {
                    ("content", p["contentIndex"].as_u64().unwrap_or(0))
                };
                if index > 1024 {
                    return false;
                }
                self.observed_items.insert(key);
                let item = self.item(turn, id, "reasoning");
                if !item[field].is_array() {
                    item[field] = json!([]);
                }
                let values = item[field].as_array_mut().unwrap();
                let index = index as usize;
                values.resize_with(values.len().max(index + 1), || json!(""));
                let mut text = values[index].as_str().unwrap_or("").to_string();
                text.push_str(p["delta"].as_str().unwrap_or(""));
                values[index] = json!(text);
            }
            _ => return false,
        }
        match method {
            "thread/status/changed" => self.status_sequence = sequence,
            "thread/name/updated" => self.name_sequence = sequence,
            "thread/settings/updated" => self.settings_sequence = sequence,
            _ => self.sequence = sequence,
        }
        true
    }
    pub fn active_turn(&self) -> Option<&str> {
        self.turns
            .last()
            .filter(|t| t["status"] == "inProgress")
            .and_then(|t| t["id"].as_str())
    }
    pub fn pending(&self) -> Vec<Value> {
        let mut entries: Vec<Value> = self
            .requests
            .iter()
            .filter_map(|(key, request)| {
                let mut entry = interactions::normalize(request)?;
                entry["id"] = json!(key);
                Some(entry)
            })
            .collect();
        entries.extend(interactions::async_pending(&self.turns));
        entries
    }
    pub fn permission_state(&self) -> Value {
        state::read_permissions(&self.settings)
    }
    pub fn settings_view(&self, recent_models: Vec<String>) -> Value {
        let mode = &self.settings["collaborationMode"];
        json!({"model":self.settings["model"],
            "effort":self.settings.get("effort").unwrap_or(&self.settings["reasoningEffort"]),
            "mode":mode["mode"].as_str().unwrap_or("default"),
            "recentModels":recent_models,"permissions":self.permission_state()})
    }
    pub fn snapshot(&self) -> Value {
        let mut thread = self.thread.clone();
        thread["mode"] = json!(self.settings["collaborationMode"]["mode"]
            .as_str()
            .unwrap_or("default"));
        let permission = self.permission_state();
        thread["permissionMode"] = permission["sandboxType"].clone();
        thread["permissionProfile"] = permission["profile"].clone();
        json!({"connection":"live","thread":thread,"turns":self.turns,"interactions":self.pending()})
    }
}

fn item_key(turn: &Value, item: &Value) -> (String, String) {
    (
        turn.as_str().unwrap_or("").to_owned(),
        item.as_str().unwrap_or("").to_owned(),
    )
}

fn item_finished(item: &Value) -> bool {
    matches!(
        item["status"].as_str(),
        Some("completed" | "failed" | "interrupted" | "declined")
    )
}

fn merge_turn(
    mut incoming: Value,
    previous: &Value,
    observed: &HashSet<(String, String)>,
    persisted: bool,
) -> Value {
    if persisted
        && incoming["status"] != "inProgress"
        && incoming["itemsView"] != "notLoaded"
        && incoming["itemsView"] != "summary"
    {
        return incoming;
    }
    let previous_items = previous["items"].as_array().cloned().unwrap_or_default();
    if incoming["itemsView"] == "notLoaded"
        || incoming["itemsView"] == "summary"
        || incoming["items"].as_array().is_none_or(Vec::is_empty)
    {
        incoming["items"] = json!(previous_items);
    } else {
        let turn_id = incoming["id"].as_str().unwrap_or("").to_owned();
        let items = incoming["items"].as_array_mut().unwrap();
        for old in previous_items {
            if let Some(item) = items.iter_mut().find(|item| item["id"] == old["id"]) {
                if item_finished(&old) && item["status"] == "inProgress" {
                    *item = old;
                }
            } else if observed
                .contains(&(turn_id.clone(), old["id"].as_str().unwrap_or("").to_owned()))
            {
                items.push(old);
            }
        }
    }
    if previous["status"] != "inProgress"
        && !previous["status"].is_null()
        && incoming["status"] == "inProgress"
    {
        for key in ["status", "completedAt", "error", "durationMs"] {
            if let Some(value) = previous.get(key) {
                incoming[key] = value.clone();
            }
        }
    }
    incoming
}
