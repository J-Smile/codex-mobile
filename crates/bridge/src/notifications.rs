use crate::{
    auth::RelayPhone,
    paths::{read_json_optional, write_private},
    BridgeError, Result,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::elliptic_curve::{rand_core::OsRng, sec1::ToEncodedPoint};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};
use tokio::sync::Mutex;
use web_push_native::{jwt_simple::algorithms::ES256KeyPair, Auth as PushAuth, WebPushBuilder};

struct State {
    entries: HashMap<String, Value>,
    turns: HashMap<String, HashMap<String, String>>,
    completed: HashSet<String>,
}
pub struct Notifications {
    directory: PathBuf,
    keys: Value,
    state: Mutex<State>,
    client: reqwest::Client,
}
pub fn validate(value: &Value) -> Result<Value> {
    let url = value["endpoint"]
        .as_str()
        .and_then(|s| url::Url::parse(s).ok())
        .ok_or_else(|| BridgeError::invalid("通知订阅地址无效。"))?;
    let host = url.host_str().unwrap_or("");
    let allowed = [
        "web.push.apple.com",
        "fcm.googleapis.com",
        "updates.push.services.mozilla.com",
    ]
    .contains(&host)
        || host.ends_with(".notify.windows.com");
    let key = |value: &Value, size: usize| {
        value.as_str().is_some_and(|s| {
            s.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
                && URL_SAFE_NO_PAD.decode(s).is_ok_and(|b| b.len() == size)
        })
    };
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || !allowed
        || !key(&value["keys"]["p256dh"], 65)
        || !key(&value["keys"]["auth"], 16)
    {
        return Err(BridgeError::invalid("浏览器通知订阅无效，请重新开启。"));
    }
    Ok(
        json!({"endpoint":url.as_str(),"keys":{"p256dh":value["keys"]["p256dh"],"auth":value["keys"]["auth"]}}),
    )
}
impl Notifications {
    pub fn open(directory: PathBuf) -> Result<Self> {
        let keys = if let Some(keys) = read_json_optional(&directory.join("push-keys.json"))? {
            keys
        } else {
            let private = p256::SecretKey::random(&mut OsRng);
            let public = private.public_key().to_encoded_point(false);
            let keys = json!({"publicKey":URL_SAFE_NO_PAD.encode(public.as_bytes()),"privateKey":URL_SAFE_NO_PAD.encode(private.to_bytes())});
            write_private(
                &directory.join("push-keys.json"),
                &serde_json::to_vec(&keys)?,
            )?;
            keys
        };
        let entries = read_json_optional(&directory.join("push-subscriptions.json"))?
            .unwrap_or(json!([]))
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["expiresAt"].as_i64().unwrap_or(0) > crate::now_ms())
            .filter_map(|e| {
                e["subscription"]["endpoint"].as_str().map(|endpoint| {
                    let mut entry = e.clone();
                    entry.as_object_mut().unwrap().shift_remove("threadIds");
                    if entry["enabledAt"].is_null() {
                        entry["enabledAt"] = json!(crate::now_ms());
                    }
                    (endpoint.into(), entry)
                })
            })
            .collect();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .build()?;
        Ok(Self {
            directory,
            keys,
            state: Mutex::new(State {
                entries,
                turns: HashMap::new(),
                completed: HashSet::new(),
            }),
            client,
        })
    }
    fn save(&self, state: &State) -> Result<()> {
        write_private(
            &self.directory.join("push-subscriptions.json"),
            &serde_json::to_vec(&state.entries.values().collect::<Vec<_>>())?,
        )
    }
    pub fn public_key(&self) -> Value {
        self.keys["publicKey"].clone()
    }
    pub async fn has_subscriptions(&self) -> bool {
        self.state
            .lock()
            .await
            .entries
            .values()
            .any(|e| e["expiresAt"].as_i64().unwrap_or(0) > crate::now_ms())
    }
    pub async fn status(&self, subscription: &Value, owner: &str, expires: i64) -> Result<Value> {
        let subscription = validate(subscription)?;
        let endpoint = subscription["endpoint"].as_str().unwrap();
        let mut state = self.state.lock().await;
        let Some(entry) = state
            .entries
            .get_mut(endpoint)
            .filter(|e| e["expiresAt"].as_i64().unwrap_or(0) > crate::now_ms())
        else {
            return Ok(json!({"enabled":false,"error":""}));
        };
        if entry["subscription"]["keys"] != subscription["keys"] {
            return Err(BridgeError::invalid("通知订阅不匹配，请重新开启。"));
        }
        entry["owner"] = json!(owner);
        entry["expiresAt"] = json!(expires);
        let error = entry["lastError"].as_str().unwrap_or("").to_string();
        self.save(&state)?;
        Ok(json!({"enabled":true,"error":error}))
    }
    pub async fn add(
        &self,
        subscription: &Value,
        owner: &str,
        expires: i64,
        origin: &str,
    ) -> Result<Value> {
        let subscription = validate(subscription)?;
        if !origin.starts_with("https://") {
            return Err(BridgeError::invalid("请通过 HTTPS 网页 App 开启系统通知。"));
        }
        let mut state = self.state.lock().await;
        let endpoint = subscription["endpoint"].as_str().unwrap().to_string();
        let enabled = state
            .entries
            .get(&endpoint)
            .and_then(|e| e["enabledAt"].as_i64())
            .unwrap_or_else(crate::now_ms);
        state.entries.insert(endpoint,json!({"subscription":subscription,"owner":owner,"expiresAt":expires,"origin":origin,"enabledAt":enabled,"lastError":""}));
        self.save(&state)?;
        Ok(json!({"enabled":true}))
    }
    pub(crate) async fn clear_lan(&self) -> Result<()> {
        self.remove_matching(|_, entry| {
            !entry["owner"]
                .as_str()
                .is_some_and(RelayPhone::owns_subscription)
        })
        .await
        .map(|_| ())
    }

    pub async fn remove_for_owner(&self, owner: &str, endpoint: Option<&str>) -> Result<Value> {
        self.remove_matching(|key, entry| {
            entry["owner"] == owner && endpoint.is_none_or(|endpoint| endpoint == key)
        })
        .await
    }

    async fn remove_matching(&self, remove: impl Fn(&str, &Value) -> bool) -> Result<Value> {
        let mut state = self.state.lock().await;
        state.entries.retain(|key, value| !remove(key, value));
        if state.entries.is_empty() {
            state.turns.clear();
            state.completed.clear();
        }
        self.save(&state)?;
        Ok(json!({"enabled":false,"remaining":0}))
    }
    async fn send(&self, entry: &Value, notice: &Value) -> Result<()> {
        let endpoint = entry["subscription"]["endpoint"].as_str().unwrap();
        if self.state.lock().await.entries.get(endpoint) != Some(entry)
            || entry["expiresAt"].as_i64().unwrap_or(0) <= crate::now_ms()
        {
            return Ok(());
        }
        let private = URL_SAFE_NO_PAD
            .decode(self.keys["privateKey"].as_str().unwrap())
            .map_err(|_| BridgeError::unavailable("系统通知密钥无效。"))?;
        let key = ES256KeyPair::from_bytes(&private)
            .map_err(|_| BridgeError::unavailable("系统通知密钥无效。"))?;
        let public = URL_SAFE_NO_PAD
            .decode(entry["subscription"]["keys"]["p256dh"].as_str().unwrap())
            .map_err(|_| BridgeError::invalid("通知订阅密钥无效。"))?;
        let auth = URL_SAFE_NO_PAD
            .decode(entry["subscription"]["keys"]["auth"].as_str().unwrap())
            .map_err(|_| BridgeError::invalid("通知订阅密钥无效。"))?;
        let builder = WebPushBuilder::new(
            endpoint
                .parse()
                .map_err(|_| BridgeError::invalid("通知订阅地址无效。"))?,
            p256::PublicKey::from_sec1_bytes(&public)
                .map_err(|_| BridgeError::invalid("通知订阅公钥无效。"))?,
            PushAuth::clone_from_slice(&auth),
        )
        .with_valid_duration(std::time::Duration::from_secs(3600))
        .with_vapid(&key, entry["origin"].as_str().unwrap_or(""));
        let (parts, body) = builder
            .build(serde_json::to_vec(notice)?)
            .map_err(|e| BridgeError::unavailable(format!("无法准备系统通知：{e}")))?
            .into_parts();
        let result = self
            .client
            .post(endpoint)
            .headers(parts.headers)
            .header("Urgency", "high")
            .body(body)
            .send()
            .await;
        let status = result.as_ref().map(|r| r.status().as_u16()).ok();
        let succeeded = status.is_some_and(|s| (200..300).contains(&s));
        let mut state = self.state.lock().await;
        if state.entries.get(endpoint) == Some(entry) {
            if matches!(status, Some(404 | 410)) {
                state.entries.remove(endpoint);
            } else if let Some(entry) = state.entries.get_mut(endpoint) {
                entry["lastError"] = json!(if succeeded {
                    ""
                } else {
                    "系统推送未送达，请发送测试通知检查。"
                });
            }
            self.save(&state)?;
        }
        if succeeded {
            Ok(())
        } else {
            Err(BridgeError::unavailable(
                "系统通知发送失败，请检查通知设置。",
            ))
        }
    }
    pub async fn test(&self, endpoint: &str, owner: &str) -> Result<()> {
        let entry = self
            .state
            .lock()
            .await
            .entries
            .get(endpoint)
            .filter(|e| e["owner"] == owner)
            .cloned()
            .ok_or_else(|| BridgeError::invalid("请先开启所有对话通知。"))?;
        self.send(&entry,&json!({"title":"Codex 通知已连接","body":"所有对话的任务完成或失败后，会在这里通知你。","tag":"codex-mobile-test"})).await
    }
    pub async fn baseline(&self, turns: &[Value]) {
        let mut state = self.state.lock().await;
        for turn in turns {
            state.completed.insert(format!(
                "{}:{}",
                turn["threadId"].as_str().unwrap_or(""),
                turn["id"].as_str().unwrap_or("")
            ));
        }
    }
    async fn notify(&self, id: &str, title: &str, turn: &Value, completed_at: Option<i64>) {
        let key = format!("{id}:{}", turn["id"].as_str().unwrap_or(""));
        let entries = {
            let mut state = self.state.lock().await;
            if !state.completed.insert(key.clone()) {
                return;
            }
            state
                .entries
                .values()
                .filter(|e| {
                    e["expiresAt"].as_i64().unwrap_or(0) > crate::now_ms()
                        && completed_at
                            .is_none_or(|time| time >= e["enabledAt"].as_i64().unwrap_or(0) / 1000)
                })
                .cloned()
                .collect::<Vec<_>>()
        };
        let failed = turn["status"] == "failed" || !turn["error"].is_null();
        let notice = json!({"title":if failed{"Codex 任务失败"}else{"Codex 任务完成"},"body":if title.is_empty(){"点按查看对话"}else{title},"threadId":id,"tag":format!("codex-{key}")});
        for entry in entries {
            if let Err(error) = self.send(&entry, &notice).await {
                tracing::warn!("{}", error);
            }
        }
    }
    pub async fn observe_history(&self, turns: &[Value]) {
        for turn in turns {
            self.notify(
                turn["threadId"].as_str().unwrap_or(""),
                turn["title"].as_str().unwrap_or(""),
                turn,
                turn["completedAt"].as_i64(),
            )
            .await;
        }
    }
    pub async fn observe(&self, snapshot: &Value) {
        if snapshot["connection"] != "live" || !self.has_subscriptions().await {
            return;
        }
        let id = snapshot["thread"]["id"].as_str().unwrap_or("");
        let current = snapshot["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| {
                Some((
                    t["id"].as_str()?.to_string(),
                    t["status"].as_str()?.to_string(),
                ))
            })
            .collect();
        let previous = self.state.lock().await.turns.insert(id.into(), current);
        let Some(previous) = previous else { return };
        for turn in snapshot["turns"].as_array().into_iter().flatten() {
            if previous
                .get(turn["id"].as_str().unwrap_or(""))
                .is_some_and(|s| s == "inProgress")
                && (turn["status"] == "completed" || turn["status"] == "failed")
            {
                self.notify(
                    id,
                    snapshot["thread"]["title"].as_str().unwrap_or(""),
                    turn,
                    None,
                )
                .await;
            }
        }
    }
}
