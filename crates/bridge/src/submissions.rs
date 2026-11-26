use crate::{
    config::MessageModel,
    desktop::Desktop,
    images::{image_ids, Images},
    operations::{Claim, Operations},
    session::Session,
    BridgeError, Result,
};
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{watch, Mutex};
use tokio_util::{sync::CancellationToken, task::TaskTracker};
struct PendingSend {
    hash: String,
    result: watch::Sender<Option<Result<Value>>>,
}
struct OperationIdentity {
    owner: String,
    key: String,
    hash: String,
    target: String,
    kind: &'static str,
}
/// 一次提交的校验、投递与回执持久化；复用服务的关停边界。
pub(crate) struct Submissions {
    pub operations: Operations,
    session: Arc<Session>,
    desktop: Arc<Desktop>,
    images: Images,
    sends: Mutex<HashMap<String, Arc<PendingSend>>>,
    work: TaskTracker,
    abort_work: CancellationToken,
}
impl Submissions {
    pub fn new(
        operations: Operations,
        session: Arc<Session>,
        desktop: Arc<Desktop>,
        images: Images,
        work: TaskTracker,
        abort_work: CancellationToken,
    ) -> Arc<Self> {
        Arc::new(Self {
            operations,
            session,
            desktop,
            images,
            sends: Mutex::new(HashMap::new()),
            work,
            abort_work,
        })
    }
    pub async fn create(self: &Arc<Self>, operation_owner: String, input: Value) -> Result<Value> {
        if input.get("execution").is_some() {
            return Err(
                BridgeError::invalid("新建聊天统一由官方桌面执行，请刷新手机页面后重试。")
                    .before_send(),
            );
        }
        let (images, prompt, request_id, selection, title, source, project) = async {
            let images = image_ids(input.get("images"))?;
            if !images.is_empty() {
                self.session.require("images")?;
            }
            if input.as_object().unwrap().keys().any(|key| {
                ![
                    "prompt",
                    "requestId",
                    "sourceThreadId",
                    "projectId",
                    "images",
                    "title",
                    "model",
                    "thinking",
                ]
                .contains(&key.as_str())
            }) {
                return Err(BridgeError::invalid("新建请求包含不支持的参数。").before_send());
            }
            let (prompt, request_id) = message_fields(&input, &images)?;
            let selection =
                MessageModel::parse(&input, &self.session.model_efforts().unwrap_or_default())?;
            if !selection.is_empty() {
                self.session.require("modelSelection")?;
            }
            let title = input
                .get("title")
                .map(|value| {
                    value
                        .as_str()
                        .map(str::trim)
                        .filter(|title| !title.is_empty() && title.chars().count() <= 200)
                        .map(str::to_owned)
                        .ok_or_else(|| {
                            BridgeError::invalid("聊天标题须为 1 到 200 字。").before_send()
                        })
                })
                .transpose()?;
            let source = input["sourceThreadId"]
                .as_str()
                .filter(|source| !source.is_empty())
                .ok_or_else(|| BridgeError::invalid("请先明确选择来源聊天。").before_send())?
                .to_owned();
            let project = input["projectId"]
                .as_str()
                .filter(|project| !project.is_empty())
                .ok_or_else(|| BridgeError::invalid("请先选择官方项目。").before_send())?
                .to_owned();
            self.desktop.authorize(&source).await?;

            Ok((
                images, prompt, request_id, selection, title, source, project,
            ))
        }
        .await
        .map_err(|error: BridgeError| error.before_send())?;
        let target = format!("new:{project}");
        let mut identity = json!([source, project, prompt, title]);
        if !images.is_empty() || !selection.is_empty() {
            identity
                .as_array_mut()
                .unwrap()
                .extend([json!(images), json!(selection)]);
        }
        let hash = crate::digest(identity.to_string());
        let this = self.clone();
        let result = self
            .dedup(
                OperationIdentity {
                    owner: operation_owner,
                    key: request_id.clone(),
                    hash,
                    target,
                    kind: "create",
                },
                async move {
                    let attachments = this
                        .images
                        .attachments(&images)
                        .map_err(BridgeError::before_send)?;
                    let prompt = crate::images::context_prompt(&prompt, &attachments);
                    this.desktop
                        .create_chat(
                            &source,
                            &project,
                            &prompt,
                            &request_id,
                            title.as_deref(),
                            &selection,
                        )
                        .await
                },
            )
            .await?;

        Ok(
            json!({"created":true,"threadId":result["threadId"],"firstTurnAccepted":result["firstTurnAccepted"]==true}),
        )
    }
    pub async fn send(
        self: &Arc<Self>,
        operation_owner: String,
        id: &str,
        input: Value,
    ) -> Result<Value> {
        let (images, prompt, request_id, selection, mode) = async {
            self.desktop.authorize(id).await?;
            if self.operations.creation_pending(id, None)? {
                return Err(BridgeError::unavailable(
                    "新聊天首轮尚未确认，请等待桌面同步；不会重复创建。",
                ));
            }
            let images = image_ids(input.get("images"))?;
            let (prompt, request_id) = message_fields(&input, &images)?;
            let selection =
                MessageModel::parse(&input, &self.session.model_efforts().unwrap_or_default())?;
            if !selection.is_empty() {
                self.session.require("modelSelection")?;
            }
            let mode = input.get("mode").cloned().unwrap_or(json!("now"));
            if mode != "now" && mode != "queue" {
                return Err(BridgeError::invalid("请输入消息内容。"));
            }
            if mode == "queue" {
                self.session.require("queue")?;
                if !selection.is_empty() {
                    return Err(BridgeError::invalid("队列暂不支持覆盖模型设置。").before_send());
                }
            }
            if !images.is_empty() {
                self.session.require("images")?;
            }

            Ok((images, prompt, request_id, selection, mode))
        }
        .await
        .map_err(|error: BridgeError| error.before_send())?;
        let mut identity = json!([id, prompt, mode, images]);
        if !selection.is_empty() {
            identity.as_array_mut().unwrap().push(json!(selection));
        }
        let hash = crate::digest(identity.to_string());
        let key = request_id.clone();
        let this = self.clone();
        let thread = id.to_string();
        let queued = mode == "queue";
        let result = self
            .dedup(
                OperationIdentity {
                    owner: operation_owner,
                    key,
                    hash,
                    target: id.to_string(),
                    kind: "message",
                },
                async move {
                    let attachments = this
                        .images
                        .attachments(&images)
                        .map_err(BridgeError::before_send)?;
                    if queued {
                        this.desktop
                            .enqueue(&thread, &prompt, &request_id, &attachments)
                            .await
                    } else {
                        this.desktop
                            .send_message(&thread, &prompt, &request_id, &attachments, &selection)
                            .await
                    }
                },
            )
            .await?;

        let queued = queued || result["status"] == "queued";
        Ok(json!({"accepted":true,"queued":queued,"result":result}))
    }
    async fn dedup<F>(self: &Arc<Self>, identity: OperationIdentity, operation: F) -> Result<Value>
    where
        F: std::future::Future<Output = Result<Value>> + Send + 'static,
    {
        let OperationIdentity {
            owner,
            key,
            hash,
            target,
            kind,
        } = identity;
        let instance = self
            .session
            .connect()
            .await
            .map_err(BridgeError::before_send)?
            .runtime_id()
            .ok_or_else(|| BridgeError::unavailable("没有有效的本机实例。").before_send())?;
        let cache_key = format!("{owner}:{key}");
        let pending = {
            let mut sends = self.sends.lock().await;
            if let Some(pending) = sends.get(&cache_key) {
                if pending.hash != hash {
                    return Err(BridgeError::new(409, "发送编号已用于其他消息。"));
                }
                pending.clone()
            } else {
                match self
                    .operations
                    .claim(&owner, &key, &hash, &instance, &target, kind)?
                {
                    Claim::Saved(result) => return result,
                    Claim::Dispatch => {}
                }
                let (result, _) = watch::channel(None);
                let pending = Arc::new(PendingSend { hash, result });
                sends.insert(cache_key.clone(), pending.clone());
                let this = self.clone();
                let entry = pending.clone();
                let cancel = self.abort_work.clone();
                self.work.spawn(async move {
                    tokio::select! { _ = cancel.cancelled() => {}, _ = async move {
                    let mut result = operation.await;
                    if let Err(error) = &mut result {
                        error.outcome = Some(
                            if error.before_send {
                                "not-dispatched"
                            } else {
                                "unknown"
                            }
                            .into(),
                        );
                    }
                    if this.operations.finish(&owner, &key, &result).is_err() {
                        let mut error = BridgeError::unavailable(
                            "操作回执未能保存，请在桌面确认结果。不会重复执行。",
                        );
                        error.outcome = Some("unknown".into());
                        result = Err(error);
                    }
                    entry.result.send_replace(Some(result));
                    this.sends.lock().await.remove(&cache_key);
                    } => {} }
                });
                pending
            }
        };
        let mut receiver = pending.result.subscribe();
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver.changed().await.map_err(|_| {
                BridgeError::unavailable("等待操作结果的连接已关闭，请查看聊天确认。")
            })?;
        }
    }
    pub async fn reply(
        self: &Arc<Self>,
        owner: String,
        id: &str,
        request: &str,
        input: Value,
    ) -> Result<Value> {
        let desktop = self.desktop.clone();
        let target = format!("reply:{id}:{request}");
        let key = crate::digest(&target);
        let hash = crate::digest(input.to_string());
        let id = id.to_owned();
        let request = request.to_owned();
        self.dedup(
            OperationIdentity {
                owner,
                key,
                hash,
                target,
                kind: "reply",
            },
            async move { desktop.reply(&id, &request, &input).await },
        )
        .await
    }
    pub async fn resume(self: &Arc<Self>, owner: String, id: &str, turn: &str) -> Result<Value> {
        let desktop = self.desktop.clone();
        let target = format!("continue:{id}:{turn}");
        let key = crate::digest(&target);
        let id = id.to_owned();
        let turn = turn.to_owned();
        self.dedup(
            OperationIdentity {
                owner,
                key: key.clone(),
                hash: key,
                target,
                kind: "continue",
            },
            async move { desktop.resume_turn(&id, &turn).await },
        )
        .await
    }
}
fn message_fields(input: &Value, images: &[String]) -> Result<(String, String)> {
    let prompt = input["prompt"]
        .as_str()
        .filter(|s| !s.trim().is_empty() || !images.is_empty())
        .ok_or_else(|| BridgeError::invalid("请输入消息内容。"))?;
    let id = input["requestId"]
        .as_str()
        .filter(|s| valid_request_id(s))
        .ok_or_else(|| BridgeError::invalid("发送编号无效，请刷新后重试。"))?;
    Ok((prompt.into(), id.into()))
}
pub(crate) fn valid_request_id(id: &str) -> bool {
    (16..=100).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
