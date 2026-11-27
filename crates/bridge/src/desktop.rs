use crate::{
    app_server::{AppServerConnection, AppServerEvent},
    config::MessageModel,
    history::History,
    live::LiveState,
    native_events::RolloutWatch,
    paths::Paths,
    session::Session,
    BridgeError, Result,
};
use futures_util::{future::BoxFuture, future::Shared, FutureExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex, Notify, RwLock};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

#[derive(Clone, Default, Debug)]
pub struct LiveView {
    pub snapshot: Option<Value>,
    pub queue: Option<Value>,
    pub error: Option<String>,
    pub owner: Option<String>,
}
pub struct Follow {
    finished: AtomicBool,
    pub updates: watch::Sender<LiveView>,
    pub state: RwLock<LiveState>,
    connection: RwLock<Option<Arc<AppServerConnection>>>,
    refresh: Notify,
}

type NativeReadFuture = Shared<BoxFuture<'static, Result<Value>>>;
type NativeReads = std::sync::Mutex<HashMap<(String, String), Weak<NativeReadFuture>>>;

struct NativeRead<'a> {
    reads: &'a NativeReads,
    key: (String, String),
    future: Option<Arc<NativeReadFuture>>,
}

impl Drop for NativeRead<'_> {
    fn drop(&mut self) {
        let mut reads = self.reads.lock().unwrap();
        let future = self.future.take().unwrap();
        let completed = future.peek().is_some();
        let current = reads
            .get(&self.key)
            .is_some_and(|read| read.ptr_eq(&Arc::downgrade(&future)));
        // 在同一把锁内释放等待者，避免并发取消各自看到对方仍在等待。
        drop(future);
        // 结果只属于本次等待；取消最后一个等待者时也释放原 RPC。
        if current && (completed || reads.get(&self.key).and_then(Weak::upgrade).is_none()) {
            reads.remove(&self.key);
        }
    }
}

pub struct Desktop {
    tasks: TaskTracker,
    pub paths: Paths,
    pub history: Arc<History>,
    pub session: Arc<Session>,
    owns_session: bool,
    catalog_hydration: Mutex<()>,
    native_threads: RwLock<HashMap<String, Value>>,
    native_reads: NativeReads,
    catalog_instance: Mutex<Option<String>>,
    follows: std::sync::Mutex<HashMap<String, Arc<Follow>>>,
    queue_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    cancel: CancellationToken,
    pub last_native_error: RwLock<Option<String>>,
}

impl Desktop {
    pub fn new(paths: Paths, history: Arc<History>) -> Arc<Self> {
        Self::build(paths.clone(), history, Session::new(paths), true)
    }
    pub fn with_session(paths: Paths, history: Arc<History>, session: Arc<Session>) -> Arc<Self> {
        Self::build(paths, history, session, false)
    }
    fn build(
        paths: Paths,
        history: Arc<History>,
        session: Arc<Session>,
        owns_session: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            tasks: TaskTracker::new(),
            paths,
            history,
            session,
            owns_session,
            catalog_hydration: Mutex::new(()),
            native_threads: RwLock::new(HashMap::new()),
            native_reads: std::sync::Mutex::new(HashMap::new()),
            catalog_instance: Mutex::new(None),
            follows: std::sync::Mutex::new(HashMap::new()),
            queue_locks: Mutex::new(HashMap::new()),
            cancel: CancellationToken::new(),
            last_native_error: RwLock::new(None),
        })
    }
    pub async fn sync_status(&self) -> Value {
        if (!crate::runtime::SHARED_RUNTIME_SUPPORTED || !crate::native::eligible(&self.paths))
            && self.session.status()["native"] != true
        {
            return json!({"state":"history-only","message":"本版本仅提供本机历史阅读；任务操作请在官方 Codex 完成。"});
        }
        if self.session.status()["connected"] != true {
            return json!({"state":"waiting","message":self.last_native_error.read().await.as_deref().unwrap_or("桌面未接入，已保存的历史仍可阅读。")});
        }
        match self.last_native_error.read().await.as_ref() {
            None if self.session.status()["controls"] == true => {
                json!({"state":"connected","message":"已连接官方桌面，支持手机回复、审批、停止和官方队列。"})
            }
            None if crate::runtime::SHARED_RUNTIME_SUPPORTED => {
                json!({"state":"connected","message":"已连接官方消息通道。启用审批、停止和队列：待任务结束后退出 Codex，再从 Mobile 打开。"})
            }
            None => {
                json!({"state":"connected","message":"已连接官方桌面，支持文字发送和聊天同步。"})
            }
            Some(error) => json!({"state":"error","message":error}),
        }
    }
    pub async fn probe(&self) -> Result<()> {
        if !crate::native::eligible(&self.paths) {
            return Err(
                BridgeError::unavailable("此环境未开放桌面控制，保留历史阅读。").before_send(),
            );
        }
        let result = async {
            let connection = self.session.connect().await?;
            if !connection.is_native() {
                self.session.require("readLive")?;
                connection.rpc("thread/loaded/list", json!({})).await?;
            }
            Ok(())
        }
        .await;
        *self.last_native_error.write().await = result
            .as_ref()
            .err()
            .map(|error: &BridgeError| error.message.clone());
        result
    }
    pub async fn tools_call(
        &self,
        tool: &str,
        args: Value,
        thread: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<Value> {
        if ![
            "list_threads",
            "list_projects",
            "navigate_to_codex_page",
            "read_thread",
        ]
        .contains(&tool)
        {
            return Err(BridgeError::invalid("该桌面操作未开放。").before_send());
        }
        let thread = thread.filter(|id| !id.is_empty()).ok_or_else(|| {
            BridgeError::unavailable("桌面工具需要明确的目标聊天；不会创建或借用控制聊天。")
                .before_send()
        })?;
        self.session.require("readLive")?;
        let connection = self.session.connect().await?;
        self.check_instance(&connection).await;
        let payload = if let Some(native) = connection.native_tools() {
            self.native_read(&native, thread).await?;
            native.call_tool(thread, tool, args, request_id).await?
        } else {
            self.require_route(&connection, thread).await?;
            let result = connection
                .request(
                    "mcpServer/tool/call",
                    json!({"threadId":thread,"server":"codex_app","tool":tool,"arguments":args}),
                    Duration::from_secs(60),
                )
                .await?;
            let text = result["content"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| item["type"] == "text")
                .filter_map(|item| item["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if result["isError"] == true {
                return Err(BridgeError::unavailable(if text.is_empty() {
                    "Codex 桌面端未接受请求。".into()
                } else {
                    text
                }));
            }
            let payload = if let Some(value) = result
                .get("structuredContent")
                .filter(|value| value.is_object())
            {
                value.clone()
            } else {
                serde_json::from_str(&text).map_err(|_| {
                    BridgeError::unavailable(
                        "Codex 桌面端返回了无法识别的结果，请刷新确认操作结果。",
                    )
                })?
            };
            payload
        };
        if tool == "list_threads" {
            let mut threads = self.native_threads.write().await;
            let mut changed = Vec::new();
            for field in ["pinnedThreads", "threads"] {
                for thread in payload[field].as_array().into_iter().flatten() {
                    if thread["kind"] == "codex" && thread["hostId"] == "local" {
                        if let Some(id) = thread["id"].as_str() {
                            if threads.get(id).and_then(|old| old.get("projectId"))
                                != thread.get("projectId")
                            {
                                changed.push(id.to_string());
                            }
                            threads.insert(id.into(), thread.clone());
                        }
                    }
                }
            }
            drop(threads);
            // A catalog refresh can change membership without any new turn.
            // Wake the open view so it publishes the new native assignment.
            let follows = self.follows.lock().unwrap();
            for id in changed {
                if let Some(follow) = follows.get(&id) {
                    follow.updates.send_modify(|_| {});
                }
            }
        }
        Ok(payload)
    }
    async fn native_read(&self, connection: &Arc<AppServerConnection>, id: &str) -> Result<Value> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(BridgeError::invalid("聊天编号无效。").before_send());
        }
        let key = (connection.id.clone(), id.to_owned());
        let future = {
            let mut reads = self.native_reads.lock().unwrap();
            if let Some(read) = reads
                .get(&key)
                .and_then(Weak::upgrade)
                .filter(|read| read.peek().is_none())
            {
                read
            } else {
                let connection = connection.clone();
                let id = id.to_owned();
                let read = Arc::new(async move {
                    let payload = connection.call_tool(&id, "read_thread", json!({
                        "threadId":id,"hostId":"local","turnLimit":5,"includeOutputs":false,"maxOutputCharsPerItem":20000,
                    }), None).await.map_err(BridgeError::before_send)?;
                    if payload["schemaVersion"] != 1
                        || payload["thread"]["id"] != id
                        || payload["thread"]["kind"] != "codex"
                        || payload["thread"]["hostId"] != "local"
                        || !payload["turns"].is_array()
                    {
                        return Err(BridgeError::new(403, "官方桌面未确认此聊天的本机归属，保留历史阅读。").before_send());
                    }
                    Ok(payload)
                }.boxed().shared());
                reads.insert(key.clone(), Arc::downgrade(&read));
                read
            }
        };
        let read = NativeRead {
            reads: &self.native_reads,
            key,
            future: Some(future),
        };
        let payload = read.future.as_ref().unwrap().as_ref().clone().await?;
        drop(read);
        if !connection.is_alive() || !connection.owner_valid() {
            return Err(BridgeError::unavailable("官方桌面连接已失效，请重新连接。").before_send());
        }
        self.native_threads
            .write()
            .await
            .insert(id.into(), payload["thread"].clone());
        Ok(payload)
    }
    pub async fn scoped_projects(self: &Arc<Self>, source: &str) -> Result<Vec<Value>> {
        self.authorize(source).await?;
        crate::catalog::normalize_projects(
            self.tools_call("list_projects", json!({}), Some(source), None)
                .await?,
        )
    }
    pub async fn create_chat(
        self: &Arc<Self>,
        source: &str,
        project: &str,
        content: (&str, &[Value]),
        request_id: &str,
        title: Option<&str>,
        selection: &MessageModel,
    ) -> Result<Value> {
        let (prompt, images) = content;
        self.session.require("create")?;
        let projects = self
            .scoped_projects(source)
            .await
            .map_err(BridgeError::before_send)?;
        if !projects.iter().any(|row| row["projectId"] == project) {
            return Err(BridgeError::invalid("该官方本机项目已不可用，请重新选择。").before_send());
        }
        let control = self.session.connect().await?;
        if !images.is_empty() {
            self.session.require("images")?;
            control.rpc("codexMobile/createImage/prepare", json!({
                "parentTurnId":format!("mobile-operation-{request_id}"),
                "sourceThreadId":source,"prompt":if prompt.is_empty() {"请查看这些图片。"} else {prompt},
                "input":Self::input(prompt, images),"messageId":request_id,
            })).await.map_err(BridgeError::before_send)?;
        }
        let connection = control
            .native_tools()
            .ok_or_else(|| BridgeError::unavailable("新建需要官方原生桌面入口。").before_send())?;
        let mut args = json!({
            "prompt":if prompt.is_empty() {"请查看这些图片。"} else {prompt}, "target":{"type":"project","projectId":project,"environment":{"type":"local"}},
        });
        if let Some(title) = title {
            args["title"] = json!(title);
        }
        selection.apply(&mut args);
        let result = connection
            .call_tool(source, "create_thread", args, Some(request_id))
            .await?;
        let id = result["threadId"]
            .as_str()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok());
        if result["hostId"] != "local" || id.is_none() || result.get("clientThreadId").is_some() {
            return Err(BridgeError::unavailable(
                "创建尚未返回正式本机聊天 ID，请在桌面核对；不会再次创建。",
            ));
        }
        if !images.is_empty() {
            let receipt = control
                .rpc(
                    "codexMobile/createImage/finish",
                    json!({
                        "parentTurnId":format!("mobile-operation-{request_id}"),
                    }),
                )
                .await?;
            if receipt["threadId"].as_str() != id {
                return Err(BridgeError::unavailable(
                    "官方桌面尚未确认首条图片附件，请核对原聊天；不会重复创建。",
                ));
            }
        }
        Ok(json!({"status":"created","threadId":id,"firstTurnAccepted":false}))
    }
    pub async fn browser_inventory(self: &Arc<Self>, thread: &str) -> Result<Value> {
        self.session.require("contextBrowser")?;
        let (_, connection) = self.connected_follow(thread).await?;
        connection.request(
            "mcpServer/tool/call",
            json!({"threadId":thread,"server":"cua_repl","tool":"js","arguments":{"code":"await cua.getState();","title":"Reading browser tabs"}}),
            Duration::from_secs(60),
        ).await
    }
    pub async fn manage_thread(
        self: &Arc<Self>,
        id: &str,
        action: &str,
        input: Value,
    ) -> Result<Value> {
        let (capability, tool, field) = match action {
            "rename" => ("threadRename", "set_thread_title", "title"),
            "pin" => ("threadPin", "set_thread_pinned", "pinned"),
            "archive" => ("threadArchive", "set_thread_archived", "archived"),
            "delete" => ("threadDelete", "delete_thread", "confirm"),
            _ => return Err(BridgeError::invalid("聊天操作无效。").before_send()),
        };
        self.session.require(capability)?;
        let mut args = json!({"threadId":id,"source":"codex"});
        let value = &input[field];
        match action {
            "rename" => {
                let title = value
                    .as_str()
                    .map(str::trim)
                    .filter(|title| !title.is_empty() && title.chars().count() <= 200)
                    .ok_or_else(|| {
                        BridgeError::invalid("聊天标题须为 1 到 200 字。").before_send()
                    })?;
                args[field] = json!(title);
            }
            "pin" | "archive" if value.is_boolean() => args[field] = value.clone(),
            "delete" if value == "delete-thread" => {}
            _ => return Err(BridgeError::invalid("请明确确认本次聊天操作。").before_send()),
        }
        if action == "archive" || action == "delete" {
            args["hostId"] = json!("local");
        }
        let connection = self.session.connect().await?;
        if matches!(action, "archive" | "delete") && !connection.is_native() {
            // 归档和删除会关闭目标聊天的 MCP；两端统一走同实例 App Server，
            // 回执不依赖即将关闭的聊天，已归档聊天也可以恢复或明确确认后删除。
            let metadata = self.history.metadata(id, false)?;
            if metadata["archived"] != true {
                self.require_route(&connection, id).await?;
            }
            let thread = connection
                .rpc("thread/read", json!({"threadId":id}))
                .await?;
            if thread["thread"]["id"] != id {
                return Err(BridgeError::unavailable("官方桌面未确认当前聊天。").before_send());
            }
            let method = match action {
                "delete" => "thread/delete",
                _ if args["archived"] == true => "thread/archive",
                _ => "thread/unarchive",
            };
            let mut targets = vec![id.to_owned()];
            if action == "delete" {
                // 与官方删除入口一致，先收集活动及归档的附属聊天，再从子级删除。
                let mut index = 0;
                while index < targets.len() {
                    for archived in [false, true] {
                        let mut cursor = Value::Null;
                        loop {
                            let page = connection
                                .rpc(
                                    "thread/list",
                                    json!({
                                        "attachedToThreadId":targets[index],"archived":archived,
                                        "cursor":cursor,"limit":100,"useStateDbOnly":true,
                                    }),
                                )
                                .await
                                .map_err(BridgeError::before_send)?;
                            let rows = page["data"].as_array().ok_or_else(|| {
                                BridgeError::unavailable("官方附属聊天清单无法识别，尚未删除。")
                                    .before_send()
                            })?;
                            for row in rows {
                                let child = row["id"]
                                    .as_str()
                                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                                    .ok_or_else(|| {
                                        BridgeError::unavailable("官方附属聊天编号无效，尚未删除。")
                                            .before_send()
                                    })?;
                                if !targets.iter().any(|id| id == child) {
                                    targets.push(child.to_owned());
                                }
                            }
                            cursor = page["nextCursor"].clone();
                            if cursor.is_null() {
                                break;
                            }
                        }
                    }
                    index += 1;
                }
            }
            for target in targets.into_iter().rev() {
                connection.rpc(method, json!({"threadId":target})).await?;
            }
            self.native_threads.write().await.remove(id);
            if let Some(follow) = self.follows.lock().unwrap().get(id) {
                follow.refresh.notify_one();
            }
            return Ok(if action == "delete" {
                json!({"threadId":id,"deleted":true})
            } else {
                json!({"threadId":id,"archived":args["archived"]})
            });
        }
        let native = connection
            .native_tools()
            .ok_or_else(|| BridgeError::unavailable("官方桌面消息连接已断开。").before_send())?;
        self.require_route(&native, id).await?;
        let result = native.call_tool(id, tool, args.clone(), None).await?;
        if result["threadId"] != id
            || match action {
                "delete" => result["deleted"] != true,
                _ => result[field] != args[field],
            }
        {
            return Err(BridgeError::unavailable(
                "官方桌面尚未确认操作完成，请重新读取聊天状态。",
            ));
        }
        self.native_threads.write().await.remove(id);
        if let Some(follow) = self.follows.lock().unwrap().get(id) {
            follow.refresh.notify_one();
        }
        Ok(result)
    }
    pub async fn project_assignment(&self, id: &str) -> Option<Value> {
        self.native_threads
            .read()
            .await
            .get(id)
            .and_then(|thread| thread.get("projectId").cloned())
    }
    pub async fn metadata(self: &Arc<Self>, id: &str) -> Result<Value> {
        match self.history.metadata(id, false) {
            Ok(metadata) => Ok(metadata),
            Err(error)
                if error.status == 404 && self.session.capabilities()["readLive"] == true =>
            {
                self.authorize(id).await?;
                self.native_threads
                    .read()
                    .await
                    .get(id)
                    .cloned()
                    .ok_or(error)
            }
            Err(error) => Err(error),
        }
    }
    async fn hydrate_project_assignment(&self, id: &str) -> Result<()> {
        if self.project_assignment(id).await.is_some() {
            return Ok(());
        }
        // Several cold chats share this catalog read. Native IDs cannot be
        // derived from cwd or App Server's separate canonical project IDs.
        let _guard = self.catalog_hydration.lock().await;
        if self.project_assignment(id).await.is_some() {
            return Ok(());
        }
        let mut cursor: Option<String> = None;
        loop {
            let mut args = json!({"mode":"history","source":"codex","hostId":"local","archived":false,"limit":50});
            if let Some(cursor) = &cursor {
                args["cursor"] = json!(cursor);
            }
            let page = self
                .tools_call("list_threads", args, Some(id), None)
                .await?;
            if self.project_assignment(id).await.is_some() {
                return Ok(());
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                return Ok(());
            }
        }
    }
    pub async fn projects(&self) -> Result<Vec<Value>> {
        crate::catalog::projects(self).await
    }
    async fn check_instance(&self, connection: &AppServerConnection) {
        let mut instance = self.catalog_instance.lock().await;
        if *instance != connection.runtime_id() {
            self.native_threads.write().await.clear();
            *instance = connection.runtime_id();
        }
    }
    pub async fn authorize(self: &Arc<Self>, id: &str) -> Result<()> {
        self.session.require("readLive")?;
        let connection = self.session.connect().await?;
        if let Some(native) = connection.native_tools() {
            self.check_instance(&connection).await;
            self.native_read(&native, id).await?;
            return Ok(());
        }
        self.authorize_on(&connection, id).await
    }
    async fn authorize_on(&self, connection: &Arc<AppServerConnection>, id: &str) -> Result<()> {
        self.check_instance(connection).await;
        self.require_route(connection, id).await?;
        if let Some(native) = connection.native_tools() {
            self.native_read(&native, id).await?;
            return Ok(());
        }
        self.hydrate_project_assignment(id).await?;
        if !self.native_threads.read().await.contains_key(id) {
            return Err(BridgeError::new(
                403,
                "当前桌面目录没有确认此聊天属于 local，只能查看已保存历史。",
            )
            .before_send());
        }
        Ok(())
    }
    async fn require_route(&self, connection: &Arc<AppServerConnection>, id: &str) -> Result<()> {
        if connection.is_native() {
            self.native_read(connection, id).await?;
            return Ok(());
        }
        let mut route = connection
            .rpc("codexMobile/thread/route", json!({"threadId":id}))
            .await
            .map_err(BridgeError::before_send)?;
        if route["available"] == false && connection.has_mcp_tools() {
            self.native_read(connection, id).await?;
            route = connection
                .rpc("codexMobile/thread/route", json!({"threadId":id}))
                .await?;
        }
        if route["threadId"] != id
            || route["instanceId"].as_str() != connection.runtime_id().as_deref()
            || route["available"] != true
        {
            return Err(BridgeError::unavailable(
                "此聊天不由当前桌面 App Server 执行；已保存历史可阅读，操作请使用官方桌面入口。",
            )
            .before_send());
        }
        Ok(())
    }
    fn follow(self: &Arc<Self>, id: &str) -> Arc<Follow> {
        let mut follows = self.follows.lock().unwrap();
        if let Some(follow) = follows.get(id).filter(|follow| {
            !follow.finished.load(Ordering::Acquire) || follow.updates.receiver_count() > 0
        }) {
            return follow.clone();
        }
        let (updates, _) = watch::channel(LiveView::default());
        let follow = Arc::new(Follow {
            finished: AtomicBool::new(false),
            updates,
            state: RwLock::new(LiveState::default()),
            connection: RwLock::new(None),
            refresh: Notify::new(),
        });
        follows.insert(id.to_string(), follow.clone());
        let this = self.clone();
        let thread = id.to_string();
        let task = follow.clone();
        self.tasks.spawn(async move {
            this.follow_loop(thread, task).await;
        });
        follow
    }
    pub fn subscribe(self: &Arc<Self>, id: &str) -> watch::Receiver<LiveView> {
        self.follow(id).updates.subscribe()
    }
    async fn connected_follow(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<(Arc<Follow>, Arc<AppServerConnection>)> {
        self.authorize(id).await?;
        let follow = self.follow(id);
        let mut changes = follow.updates.subscribe();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(connection) = follow
                    .connection
                    .read()
                    .await
                    .as_ref()
                    .filter(|connection| connection.is_alive())
                    .cloned()
                {
                    return Ok((follow.clone(), connection));
                }
                let view = changes.borrow().clone();
                if let Some(error) = view.error {
                    return Err(BridgeError::unavailable(error).before_send());
                }
                changes
                    .changed()
                    .await
                    .map_err(|_| BridgeError::unavailable("聊天连接已关闭。").before_send())?;
            }
        })
        .await
        .map_err(|_| BridgeError::unavailable("等待聊天同步超时，请检查桌面连接。").before_send())?
    }
    async fn publish(follow: &Follow) {
        let snapshot = follow.state.read().await.snapshot();
        follow.updates.send_modify(|view| {
            view.snapshot = Some(snapshot);
            view.error = None;
        });
    }
    pub async fn snapshot(self: &Arc<Self>, id: &str) -> Result<Value> {
        let connection = self.session.connect().await?;
        if let Some(native) = connection.native_tools() {
            if connection.is_native() || self.require_route(&connection, id).await.is_err() {
                return self.native_snapshot(&native, id, None).await;
            }
        }
        let (follow, _) = self.connected_follow(id).await?;
        let snapshot = follow.state.read().await.snapshot();
        Ok(snapshot)
    }
    async fn follow_loop(self: Arc<Self>, id: String, follow: Arc<Follow>) {
        let mut delay = Duration::from_millis(250);
        let mut failures = 0;
        while !self.cancel.is_cancelled() {
            let attempt = async {
                let connection = self.session.connect().await?;
                self.session.require("readLive")?;
                if let Some(native) = connection.native_tools() {
                    self.check_instance(&connection).await;
                    if connection.is_native() || self.require_route(&connection, &id).await.is_err()
                    {
                        return self.follow_native(&native, &id, &follow).await;
                    }
                }
                self.authorize_on(&connection, &id).await?;
                let mut events = connection.subscribe();
                let (resume, sequence) = connection
                    .rpc_with_sequence(
                        "thread/resume",
                        json!({"threadId":id,"excludeTurns":true,
                    "initialTurnsPage":{"limit":5,"sortDirection":"desc","itemsView":"full"}}),
                    )
                    .await?;
                self.hydrate_project_assignment(&id).await?;
                {
                    let mut state = follow.state.write().await;
                    state.hydrate(&resume, &connection.id, sequence)?;
                    // 重入也以当前连接的待处理请求为准，不保留退出期间已结束的卡片。
                    state.requests.clear();
                    for request in connection.pending_requests(&id) {
                        state.server_request(&connection.id, request);
                    }
                }
                *follow.connection.write().await = Some(connection.clone());
                follow.updates.send_modify(|view| {
                    view.owner = Some(connection.id.clone());
                    view.error = None;
                });
                Self::publish(&follow).await;
                if let Ok(queue) = Self::read_queue(&connection, &id).await {
                    follow.updates.send_modify(|view| view.queue = Some(queue));
                }
                let mut flush = tokio::time::interval(Duration::from_millis(40));
                let mut idle = tokio::time::interval(Duration::from_secs(1));
                idle.tick().await;
                let mut dirty = false;
                let mut queue_dirty = false;
                loop {
                    tokio::select! {
                        _=idle.tick()=>{
                            // 先撤销可用连接，再等待退订；重入者等待下一次同步，不能复用退出中的连接。
                            let mut current = follow.connection.write().await;
                            let unused = follow.updates.receiver_count()==0 && Arc::strong_count(&follow)<=2;
                            if unused { *current = None; }
                            drop(current);
                            if unused {
                                connection.request("thread/unsubscribe",json!({"threadId":id}),Duration::from_secs(5)).await?;
                                return Ok(());
                            }
                        },
                        _=self.cancel.cancelled()=>return Ok::<(),BridgeError>(()),
                        _=flush.tick(),if dirty||queue_dirty=>{
                            if dirty { dirty=false;Self::publish(&follow).await; }
                            if queue_dirty { queue_dirty=false;let queue=Self::read_queue(&connection,&id).await?;follow.updates.send_modify(|view|view.queue=Some(queue)); }
                        },
                        event=events.recv()=>{
                            match event {
                                Ok(AppServerEvent::Notification{message,sequence,..})=>{
                                    if message["method"]=="codexMobile/resync" { return Err(BridgeError::unavailable("正在重新读取聊天状态。")); }
                                    if message["params"]["threadId"]!=id { continue; }
                                    if message["method"]=="thread/queue/changed" { queue_dirty=true; }
                                    dirty|=follow.state.write().await.apply(&message,sequence);
                                },
                                Ok(AppServerEvent::ServerRequest{connection_id,message,..})=>{
                                    if message["params"]["threadId"]==id {
                                        follow.state.write().await.server_request(&connection_id,message);dirty=true;
                                    }
                                },
                                Ok(AppServerEvent::Disconnected{error,..})=>return Err(error),
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>return Err(BridgeError::unavailable("正在补齐聊天状态。")),
                                Err(_)=>return Err(BridgeError::unavailable("聊天连接已断开。")),
                            }
                        }
                    }
                }
            };
            let attempt =
                tokio::select! { _=self.cancel.cancelled()=>Ok(()), result=attempt=>result };
            *follow.connection.write().await = None;
            if self.cancel.is_cancelled() {
                break;
            }
            if attempt.is_ok() {
                let mut follows = self.follows.lock().unwrap();
                if follows
                    .get(&id)
                    .is_some_and(|current| Arc::ptr_eq(current, &follow))
                {
                    // 退订完成前回来的消费者沿同一任务重新订阅；彻底空闲才从目录中移除。
                    if follow.updates.receiver_count() > 0 || Arc::strong_count(&follow) > 2 {
                        continue;
                    }
                    follows.remove(&id);
                }
                break;
            }
            let terminal = attempt
                .as_ref()
                .is_err_and(|error| error.rpc_error.is_some() || error.status == 403);
            failures += 1;
            let error = attempt
                .err()
                .map(|error| error.message)
                .unwrap_or_else(|| "正在重新连接聊天。".into());
            follow.updates.send_modify(|view| {
                view.owner = None;
                view.error = Some(error.clone());
                if let Some(snapshot) = view.snapshot.as_mut() {
                    snapshot["connection"] = json!("history");
                }
            });
            if terminal || failures >= 3 {
                break;
            }
            tokio::select! {_=self.cancel.cancelled()=>break,_=tokio::time::sleep(delay)=>{}}
            delay = (delay * 2).min(Duration::from_secs(3));
        }
        follow.finished.store(true, Ordering::Release);
    }
    async fn follow_native(
        &self,
        connection: &Arc<AppServerConnection>,
        id: &str,
        follow: &Arc<Follow>,
    ) -> Result<()> {
        let mut events = connection.subscribe();
        let initial = self.native_snapshot(connection, id, None).await?;
        *follow.connection.write().await = Some(connection.clone());
        Self::publish_native(follow, &connection.id, initial);

        let mut rollout: Option<RolloutWatch> = None;
        // 首次校验通过后安装监听，再读取一次以衔接监听开始前的写入。
        let mut refresh = tokio::time::interval(Duration::from_secs(2));
        let mut idle = tokio::time::interval(Duration::from_secs(1));
        idle.tick().await;
        loop {
            let history_ready = tokio::select! {
                _ = idle.tick() => {
                    let mut follows = self.follows.lock().unwrap();
                    if follow.updates.receiver_count() == 0 && Arc::strong_count(follow) <= 2 {
                        follows.remove(id);
                        return Ok(());
                    }
                    continue;
                }
                event = events.recv() => {
                    if let Ok(AppServerEvent::Disconnected { error, .. }) = event {
                        return Err(error);
                    }
                    if !connection.is_alive() {
                        return Err(BridgeError::unavailable("官方桌面连接已断开。"));
                    }
                    continue;
                }
                _ = refresh.tick() => None,
                _ = follow.refresh.notified() => None,
                valid = async {
                    match rollout.as_mut() {
                        Some(watch) => watch.changed().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if !valid {
                        rollout = None;
                    }
                    // 给官方历史索引留出提交时间，同时启动官方读取。
                    Some(tokio::time::Instant::now() + Duration::from_millis(40))
                }
            };
            if !rollout.as_ref().is_some_and(RolloutWatch::file_exists) {
                rollout = self
                    .history
                    .rollout_path(id)
                    .ok()
                    .flatten()
                    .and_then(RolloutWatch::open);
            }
            let snapshot = self.native_snapshot(connection, id, history_ready).await?;
            Self::publish_native(follow, &connection.id, snapshot);
            let current = self.session.connect().await?;
            if !current.is_native() && self.require_route(&current, id).await.is_ok() {
                return Ok(());
            }
            // 有订阅时仅低频核对未写入会话文件的状态。首轮还没落库时沿用短间隔读取。
            refresh.reset_after(if rollout.is_some() {
                Duration::from_secs(15)
            } else {
                Duration::from_secs(2)
            });
        }
    }
    fn publish_native(follow: &Follow, owner: &str, snapshot: Value) {
        follow.updates.send_if_modified(|view| {
            if view.snapshot.as_ref() == Some(&snapshot)
                && view.error.is_none()
                && view.owner.as_deref() == Some(owner)
            {
                return false;
            }
            view.snapshot = Some(snapshot);
            view.error = None;
            view.owner = Some(owner.to_owned());
            true
        });
    }
    pub async fn live_item(&self, id: &str, item_id: &str) -> Option<Value> {
        let follow = self.follows.lock().unwrap().get(id).cloned()?;
        let state = follow.state.read().await;
        state
            .turns
            .iter()
            .flat_map(|turn| turn["items"].as_array().into_iter().flatten())
            .find(|item| item["id"] == item_id)
            .cloned()
    }
    pub async fn settings(self: &Arc<Self>, id: &str) -> Result<Value> {
        let (follow, _) = self.connected_follow(id).await?;
        let settings = follow
            .state
            .read()
            .await
            .settings_view(self.history.recent_models()?);
        Ok(settings)
    }
    pub async fn update_settings(self: &Arc<Self>, id: &str, changes: Value) -> Result<Value> {
        self.session.require("settings")?;
        let (follow, connection) = self.connected_follow(id).await?;
        if connection.is_native() {
            return Err(BridgeError::unavailable("此聊天的控制通道尚未就绪。").before_send());
        }
        let mut params = changes.clone();
        params["threadId"] = json!(id);
        if let Some(permission) = changes["permissions"].as_str() {
            for (key, value) in crate::state::permissions(permission)?.as_object().unwrap() {
                params[key] = value.clone();
            }
        }
        if let Some(mode) = params.as_object_mut().unwrap().remove("mode") {
            let state = follow.state.read().await;
            // 由官方按目标模式生成指令，不能把执行指令带进规划模式。
            params["collaborationMode"] = json!({"mode":mode,"settings":{
                "model":changes.get("model").unwrap_or(&state.settings["model"]),
                "reasoning_effort":changes.get("effort").unwrap_or(&state.settings["effort"]),
                "developer_instructions":null
            }});
        }
        connection.rpc("thread/settings/update", params).await?;
        let (snapshot, sequence) = connection.rpc_with_sequence("thread/resume", json!({"threadId":id,
            "excludeTurns":true,"initialTurnsPage":{"limit":5,"sortDirection":"desc","itemsView":"full"}})).await?;
        follow
            .state
            .write()
            .await
            .hydrate(&snapshot, &connection.id, sequence)?;
        Self::publish(&follow).await;
        let settings = follow
            .state
            .read()
            .await
            .settings_view(self.history.recent_models()?);
        for field in ["mode", "model", "effort", "permissions"] {
            if let Some(expected) = changes.get(field) {
                let actual = if field == "permissions" {
                    &settings[field]["mode"]
                } else {
                    &settings[field]
                };
                if actual != expected {
                    return Err(BridgeError::unavailable(
                        "官方回读的设置尚未与本次修改一致，请重新读取后确认。",
                    ));
                }
            }
        }
        Ok(settings)
    }
    pub async fn interrupt(self: &Arc<Self>, id: &str, turn_id: &str) -> Result<Value> {
        self.session.require("interrupt")?;
        let lock = self.queue_lock(id).await;
        let _guard = lock.lock().await;
        let (follow, connection) = self.connected_follow(id).await?;
        if connection.is_native() {
            return Err(BridgeError::unavailable("此聊天的控制通道尚未就绪。").before_send());
        }
        if follow.state.read().await.active_turn() != Some(turn_id) {
            return Err(BridgeError::new(409, "任务已变化，请刷新后再停止。").before_send());
        }
        let goal = connection
            .rpc("thread/goal/get", json!({"threadId":id}))
            .await?;
        let paused = goal["goal"]["status"] == "active";
        if paused {
            connection
                .rpc(
                    "thread/goal/set",
                    json!({"threadId":id,"status":"paused","origin":"user"}),
                )
                .await?;
        }
        connection
            .rpc("turn/interrupt", json!({"threadId":id,"turnId":turn_id}))
            .await
            .map_err(|mut error| {
                if paused {
                    error.before_send = false;
                    error.message = format!("目标已暂停，但停止轮次尚未确认：{}", error.message);
                }
                error
            })?;
        Ok(json!({"ok":true,"goalPaused":paused,"turnId":turn_id}))
    }
    pub async fn resume_turn(self: &Arc<Self>, id: &str, turn_id: &str) -> Result<Value> {
        self.session.require("resume")?;
        let lock = self.queue_lock(id).await;
        let _guard = lock.lock().await;
        let connection = self
            .session
            .connect()
            .await
            .map_err(BridgeError::before_send)?;
        if let Some(native) = connection.native_tools() {
            let snapshot = self
                .native_snapshot(&native, id, None)
                .await
                .map_err(BridgeError::before_send)?;
            if snapshot["thread"]["status"] == "active"
                || snapshot["thread"]["status"]["type"] == "active"
                || snapshot["turns"]
                    .as_array()
                    .and_then(|turns| turns.last())
                    .is_none_or(|turn| {
                        turn["id"] != turn_id
                            || !matches!(turn["status"].as_str(), Some("failed" | "interrupted"))
                    })
            {
                return Err(BridgeError::new(
                    409,
                    "仅能继续最后一个失败或停止的任务，请刷新确认。",
                )
                .before_send());
            }
            let prompt = format!("请继续本聊天中轮次 {turn_id} 未完成的任务。先核对已有结果，避免重复已经完成的操作；保留原任务约束。");
            return self
                .send_message(
                    id,
                    &prompt,
                    &format!("continue-{}", crate::digest(turn_id)),
                    &[],
                    &MessageModel::default(),
                )
                .await;
        }
        let (follow, connection) = self.connected_follow(id).await?;
        {
            let state = follow.state.read().await;
            if state.turns.last().is_none_or(|turn| {
                turn["id"] != turn_id
                    || !matches!(turn["status"].as_str(), Some("failed" | "interrupted"))
            }) {
                return Err(BridgeError::new(
                    409,
                    "仅能继续最后一个失败或停止的任务，请刷新确认。",
                )
                .before_send());
            }
        }
        if !Self::queue_rows(&connection, id).await?.is_empty() {
            return Err(
                BridgeError::new(409, "请先处理待执行队列，再继续上一任务。").before_send(),
            );
        }
        // 协议没有 turn/resume。保留原轮次，以明确指令开启后续轮次。
        connection.rpc("turn/start", json!({"threadId":id,"clientUserMessageId":format!("continue-{}",crate::digest(turn_id)),
            "input":Self::input("请基于当前聊天继续上一轮未完成的任务，先核对已有结果，避免重复已经完成的操作。", &[])})).await
    }
    pub async fn reply(
        self: &Arc<Self>,
        id: &str,
        request_id: &str,
        input: &Value,
    ) -> Result<Value> {
        if self.session.capabilities()["native"] == true
            && serde_json::from_str::<Value>(request_id)
                .is_ok_and(|id| id[0] == "request_user_input_async")
        {
            self.session.require("asyncReplies")?;
            let connection = self
                .session
                .connect()
                .await
                .map_err(BridgeError::before_send)?
                .native_tools()
                .ok_or_else(|| BridgeError::unavailable("官方消息通道已断开。").before_send())?;
            let snapshot = self
                .native_snapshot(&connection, id, None)
                .await
                .map_err(BridgeError::before_send)?;
            let turns = snapshot["turns"].as_array().unwrap();
            if input
                .get("skipped")
                .is_some_and(|skipped| skipped != &json!([]))
            {
                return Err(BridgeError::invalid("请完成每个问题后再发送回复。").before_send());
            }
            let prompt = crate::interactions::async_reply(turns, request_id, input)
                .map_err(BridgeError::before_send)?;
            let mut receipt = self
                .send_message(
                    id,
                    &prompt,
                    &format!("reply-{}", crate::digest(request_id)),
                    &[],
                    &MessageModel::default(),
                )
                .await?;
            // 官方接受答案消息不等于原问题已结束；保留两个事实。
            receipt["resolved"] = json!(false);
            receipt["deliveryStatus"] = json!("accepted");
            return Ok(receipt);
        }
        self.session.require("interactions")?;
        let (follow, connection) = self.connected_follow(id).await?;
        if connection.is_native() {
            return Err(BridgeError::unavailable("此审批的原始控制连接已失效。").before_send());
        }
        let mut changes = follow.updates.subscribe();
        let request = follow.state.read().await.requests.get(request_id).cloned();
        if let Some(request) = request {
            let result = crate::interactions::app_server_reply(&request, input)
                .map_err(BridgeError::before_send)?;
            connection.respond(request["id"].clone(), result).await?;
        } else {
            let state = follow.state.read().await;
            let prompt = crate::interactions::async_reply(&state.turns, request_id, input)
                .map_err(BridgeError::before_send)?;
            let active = state.active_turn().map(str::to_owned);
            drop(state);
            let params = json!({"threadId":id,"input":Self::input(&prompt,&[]),"clientUserMessageId":format!("reply-{}",crate::digest(request_id))});
            if let Some(turn) = active {
                let mut params = params;
                params["expectedTurnId"] = json!(turn);
                connection.rpc("turn/steer", params).await?;
            } else {
                connection.rpc("turn/start", params).await?;
            }
        }
        let resolved = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if !connection.is_alive() || follow.state.read().await.generation != connection.id {
                    return Err(BridgeError::unavailable(
                        "回复后的连接已断开，请等待同步；不会重复提交。",
                    ));
                }
                if !follow
                    .state
                    .read()
                    .await
                    .pending()
                    .iter()
                    .any(|entry| entry["id"] == request_id)
                {
                    return Ok(json!({"ok":true,"resolved":true}));
                }
                changes
                    .changed()
                    .await
                    .map_err(|_| BridgeError::unavailable("回复结果尚未确认，请重新连接查看。"))?;
            }
        })
        .await;
        match resolved {
            Ok(Ok(result)) => Ok(result),
            _ => Ok(json!({"ok":true,"resolved":false})),
        }
    }
    async fn queue_rows(connection: &AppServerConnection, id: &str) -> Result<Vec<Value>> {
        let mut rows = vec![];
        let mut cursor = Value::Null;
        loop {
            let page = connection
                .rpc(
                    "thread/queue/list",
                    json!({"threadId":id,"cursor":cursor,"limit":100}),
                )
                .await?;
            rows.extend(
                page["data"]
                    .as_array()
                    .ok_or_else(|| BridgeError::unavailable("Codex 队列格式无效。"))?
                    .iter()
                    .cloned(),
            );
            cursor = page["nextCursor"].clone();
            if cursor.is_null() {
                break;
            }
        }
        Ok(rows)
    }
    pub fn queue_snapshot(rows: &[Value]) -> Value {
        let messages:Vec<Value>=rows.iter().map(|row|json!({"id":row["id"],"clientUserMessageId":row["clientUserMessageId"],
            "prompt":row["input"].as_array().into_iter().flatten().filter(|item|item["type"]=="text").filter_map(|item|item["text"].as_str()).collect::<Vec<_>>().join("\n"),
            "status":"queued","attachmentCount":row["input"].as_array().into_iter().flatten().filter(|item|item["type"]!="text").count()})).collect();
        json!({"revision":crate::digest(serde_json::to_vec(rows).unwrap()),"messages":messages})
    }
    async fn read_queue(connection: &AppServerConnection, id: &str) -> Result<Value> {
        Ok(Self::queue_snapshot(
            &Self::queue_rows(connection, id).await?,
        ))
    }
    pub async fn queue(self: &Arc<Self>, id: &str) -> Result<Value> {
        self.session.require("queue")?;
        let (_, connection) = self.connected_follow(id).await?;
        Self::read_queue(&connection, id).await
    }
    fn input(prompt: &str, images: &[Value]) -> Vec<Value> {
        let mut input = vec![];
        if !prompt.trim().is_empty() {
            input.push(json!({"type":"text","text":prompt,"text_elements":[]}));
        }
        input.extend(
            images
                .iter()
                .map(|image| json!({"type":"localImage","path":image["localPath"]})),
        );
        input
    }
    async fn enqueue_on(
        connection: &AppServerConnection,
        id: &str,
        prompt: &str,
        message_id: &str,
        images: &[Value],
    ) -> Result<Value> {
        let input = Self::input(prompt, images);
        connection
            .rpc(
                "thread/queue/add",
                json!({"threadId":id,"clientUserMessageId":message_id,"input":input}),
            )
            .await?;
        Ok(json!({"status":"queued","messageId":message_id}))
    }
    pub async fn enqueue(
        self: &Arc<Self>,
        id: &str,
        prompt: &str,
        message_id: &str,
        images: &[Value],
    ) -> Result<Value> {
        let lock = self.queue_lock(id).await;
        let _guard = lock.lock().await;
        let (_, connection) = self.connected_follow(id).await?;
        Self::enqueue_on(&connection, id, prompt, message_id, images).await
    }
    async fn queue_lock(&self, id: &str) -> Arc<Mutex<()>> {
        self.queue_locks
            .lock()
            .await
            .entry(id.into())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
    pub async fn update_queue(self: &Arc<Self>, id: &str, change: QueueChange) -> Result<Value> {
        let capability = match &change.operation {
            QueueOperation::Delete { .. } => "queue",
            QueueOperation::Edit { .. } => "queueEdit",
            QueueOperation::Reorder { .. } => "queueReorder",
            QueueOperation::Send { .. } => "queueStart",
        };
        self.session.require(capability)?;
        let lock = self.queue_lock(id).await;
        let _guard = lock.lock().await;
        let (_, connection) = self.connected_follow(id).await?;
        let rows = Self::queue_rows(&connection, id).await?;
        if Self::queue_snapshot(&rows)["revision"] != change.revision {
            return Err(BridgeError::new(409, "队列已变化，请刷新确认。").before_send());
        }
        let (method, mut params) = match change.operation {
            QueueOperation::Reorder { message_ids } => {
                let unique: std::collections::HashSet<_> = message_ids.iter().collect();
                if unique.len() != rows.len()
                    || unique.len() != message_ids.len()
                    || rows
                        .iter()
                        .any(|row| !message_ids.iter().any(|id| row["id"] == *id))
                {
                    return Err(
                        BridgeError::new(409, "排序必须包含当前队列的每一项。").before_send()
                    );
                }
                (
                    "thread/queue/reorder",
                    json!({"queuedSubmissionIds":message_ids}),
                )
            }
            operation => {
                let message_id = match &operation {
                    QueueOperation::Delete { message_id }
                    | QueueOperation::Send { message_id }
                    | QueueOperation::Edit { message_id, .. } => message_id,
                    _ => unreachable!(),
                };
                let row = rows
                    .iter()
                    .find(|row| row["id"] == *message_id)
                    .ok_or_else(|| {
                        BridgeError::new(409, "待执行项已变化，请刷新确认。").before_send()
                    })?;
                let mut params = json!({"queuedSubmissionId":message_id});
                let method = match operation {
                    QueueOperation::Delete { .. } => "thread/queue/delete",
                    QueueOperation::Send { .. } => "thread/queue/start",
                    QueueOperation::Edit { prompt, .. } => {
                        let mut input: Vec<Value> = row["input"]
                            .as_array()
                            .ok_or_else(|| {
                                BridgeError::unavailable("Codex 队列内容格式无效，尚未修改。")
                                    .before_send()
                            })?
                            .iter()
                            .filter(|item| item["type"] != "text")
                            .cloned()
                            .collect();
                        if prompt.trim().is_empty() && input.is_empty() {
                            return Err(BridgeError::invalid("请输入待执行内容。").before_send());
                        }
                        input.splice(0..0, Self::input(&prompt, &[]));
                        params["input"] = json!(input);
                        "thread/queue/update"
                    }
                    _ => unreachable!(),
                };
                (method, params)
            }
        };
        params["threadId"] = json!(id);
        connection.rpc(method, params).await?;
        Self::read_queue(&connection, id).await
    }
    pub async fn send_message(
        self: &Arc<Self>,
        id: &str,
        prompt: &str,
        message_id: &str,
        images: &[Value],
        selection: &MessageModel,
    ) -> Result<Value> {
        self.session.require("send")?;
        let connection = self.session.connect().await?;
        if let Some(native) = connection.native_tools().filter(|_| images.is_empty()) {
            self.native_read(&native, id).await?;
            let mut args = json!({"threadId":id,"hostId":"local","prompt":prompt});
            selection.apply(&mut args);
            let result = native
                .call_tool(id, "send_message_to_thread", args, Some(message_id))
                .await?;
            if result["threadId"] != id {
                return Err(BridgeError::unavailable(
                    "发送回执的聊天编号不一致，请在桌面核对。",
                ));
            }
            if let Some(follow) = self.follows.lock().unwrap().get(id) {
                // 已接受的输入沿现有订阅立即核对，不另建轮询或改变订阅寿命。
                follow.refresh.notify_one();
            }
            return Ok(json!({"status":"accepted","threadId":id,"messageId":message_id}));
        }
        if !images.is_empty() {
            self.session.require("images")?;
            self.require_route(&connection, id).await?;
        }
        let lock = self.queue_lock(id).await;
        let _guard = lock.lock().await;
        let (follow, connection) = self.connected_follow(id).await?;
        let active = {
            let state = follow.state.read().await;
            state.thread["status"] == "active"
                || state.thread["status"]["type"] == "active"
                || state.active_turn().is_some()
        };
        if active
            || !Self::queue_rows(&connection, id)
                .await
                .map_err(BridgeError::before_send)?
                .is_empty()
        {
            if !selection.is_empty() {
                return Err(BridgeError::new(
                    409,
                    "当前需加入队列，队列不能覆盖模型；请清除本次模型选择后发送。",
                )
                .before_send());
            }
            return Self::enqueue_on(&connection, id, prompt, message_id, images).await;
        }
        let mut params = json!({"threadId":id,"input":Self::input(prompt,images),"clientUserMessageId":message_id});
        if let Some(model) = &selection.model {
            params["model"] = json!(model);
        }
        if let Some(effort) = &selection.thinking {
            params["effort"] = json!(effort);
        }
        connection.rpc("turn/start", params).await
    }
    pub(crate) async fn creation_turn(&self, id: &str) -> Result<Value> {
        let connection = self.session.connect().await?;
        let native = connection
            .native_tools()
            .ok_or_else(|| BridgeError::unavailable("等待官方桌面确认首轮。"))?;
        let payload = self.native_read(&native, id).await?;
        Ok(first_observed_turn(&payload))
    }
    async fn native_snapshot(
        &self,
        connection: &Arc<AppServerConnection>,
        id: &str,
        history_ready: Option<tokio::time::Instant>,
    ) -> Result<Value> {
        let history = self.history.clone();
        let thread_id = id.to_owned();
        // 元数据可以提前读取。正文在官方回执及索引等待之后读取，以纳入这段时间新落库的条目。
        let metadata = tokio::task::spawn_blocking(move || history.metadata(&thread_id, true));
        let (payload, metadata) = tokio::join!(self.native_read(connection, id), metadata);
        let payload = payload?;
        if let Some(ready) = history_ready {
            tokio::time::sleep_until(ready).await;
        }
        // 新聊天的元数据可能刚落库，预读失败时由历史分页按当前状态读取。
        let metadata = metadata.ok().and_then(Result::ok);
        let history = match self.history.page_with_metadata(id, None, 5, metadata) {
            Ok(history) => history,
            Err(error) if error.status == 404 => {
                json!({"thread":{"id":id},"turns":[],"nextCursor":null})
            }
            Err(error) => return Err(error),
        };
        Ok(merge_native_snapshot(history, payload))
    }
    pub async fn close(&self) {
        self.cancel.cancel();
        self.tasks.close();
        self.tasks.wait().await;
        if self.owns_session {
            self.session.close().await;
        }
    }
}

fn merge_native_snapshot(mut history: Value, payload: Value) -> Value {
    if let Some(metadata) = payload["thread"].as_object() {
        for (key, value) in metadata {
            history["thread"][key] = value.clone();
        }
    }
    let mut turns = match history["turns"].take() {
        Value::Array(turns) => turns,
        _ => Vec::new(),
    };
    for native in payload["turns"].as_array().into_iter().flatten().rev() {
        let mut native = native.clone();
        if let Some(saved) = turns.iter_mut().find(|saved| saved["id"] == native["id"]) {
            for field in ["status", "error", "completedAt", "durationMs"] {
                saved[field] = native[field].clone();
            }
            // 工具读取是摘要；同 ID 的完整落库条目始终优先，不用摘要覆盖正文或用户输入。
            let items = saved["items"].as_array_mut().unwrap();
            for item in native["items"].as_array().into_iter().flatten() {
                if !items.iter().any(|saved| saved["id"] == item["id"]) {
                    let mut item = item.clone();
                    item["contentSource"] = json!("desktop-summary");
                    items.push(item);
                }
            }
        } else {
            for item in native["items"].as_array_mut().into_iter().flatten() {
                item["contentSource"] = json!("desktop-summary");
            }
            turns.push(native);
        }
    }
    turns.sort_by_key(|turn| turn["startedAt"].as_i64().unwrap_or(0));
    let mut interactions = crate::interactions::async_pending(&turns);
    for entry in &mut interactions {
        entry["replySupported"] = json!(true);
        entry["skipSupported"] = json!(false);
    }
    history["interactions"] = json!(interactions);
    history["turns"] = Value::Array(turns);
    history["connection"] = json!("live");
    // 沿用旧客户端识别原生桌面快照及首轮回执的协议值。
    history["syncMode"] = json!("desktop-poll");
    history["firstTurnObserved"] = first_observed_turn(&payload);
    history
}

fn first_observed_turn(payload: &Value) -> Value {
    payload["turns"]
        .as_array()
        .and_then(|turns| turns.last())
        .filter(|turn| {
            turn["id"].is_string()
                && turn["startedAt"].is_number()
                && matches!(
                    turn["status"].as_str(),
                    Some("inProgress" | "completed" | "failed" | "interrupted")
                )
        })
        .map(|turn| turn["id"].clone())
        .unwrap_or(Value::Null)
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::{
        native_test_support::{NativePipe, CHAT, OTHER},
        test_support::Fixture,
    };

    async fn read_fixture() -> (
        Fixture,
        Arc<Desktop>,
        Arc<NativePipe>,
        Arc<AppServerConnection>,
    ) {
        let fixture = Fixture::new();
        let (peer, connection) = NativePipe::start(&fixture.paths).await;
        let session = Session::new(fixture.paths.clone());
        session.install_native(connection.clone());
        let desktop = Desktop::with_session(
            fixture.paths.clone(),
            Arc::new(History::new(fixture.paths.codex_home.clone())),
            session,
        );
        (fixture, desktop, peer, connection)
    }

    #[tokio::test]
    async fn concurrent_native_reads_share_only_the_current_request() {
        let (_fixture, desktop, peer, connection) = read_fixture().await;
        peer.hold_reads.store(true, Ordering::Release);
        let mut first = Box::pin(desktop.native_read(&connection, CHAT));
        let mut second = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut second).is_pending());
        peer.wait_calls("read_thread", 1).await;
        peer.release_reads.add_permits(1);
        let first = first.await.unwrap();
        assert_eq!(peer.count("read_thread").await, 1);
        assert!(desktop.native_reads.lock().unwrap().is_empty());

        peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["title"] = json!("新读取");
        let mut next = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut next).is_pending());
        peer.wait_calls("read_thread", 2).await;
        assert_eq!(second.await.unwrap(), first);
        assert_eq!(desktop.native_reads.lock().unwrap().len(), 1);
        peer.release_reads.add_permits(1);
        assert_eq!(next.await.unwrap()["thread"]["title"], "新读取");
        assert_eq!(peer.count("read_thread").await, 2);
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        connection.close();
    }

    #[tokio::test]
    async fn native_reads_never_share_across_threads_or_generations() {
        let (fixture, desktop, peer, connection) = read_fixture().await;
        let (next_peer, next_connection) = NativePipe::start(&fixture.paths).await;
        assert_ne!(connection.id, next_connection.id);
        assert_eq!(connection.runtime_id(), next_connection.runtime_id());
        peer.hold_reads.store(true, Ordering::Release);
        next_peer.hold_reads.store(true, Ordering::Release);
        next_peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["title"] = json!("新连接");
        let mut first = Box::pin(desktop.native_read(&connection, CHAT));
        let mut other = Box::pin(desktop.native_read(&connection, OTHER));
        let mut next = Box::pin(desktop.native_read(&next_connection, CHAT));
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut other).is_pending());
        assert!(futures_util::poll!(&mut next).is_pending());
        peer.wait_calls("read_thread", 2).await;
        next_peer.wait_calls("read_thread", 1).await;
        peer.release_reads.add_permits(2);
        next_peer.release_reads.add_permits(1);
        let (first, other, next) = tokio::join!(first, other, next);
        assert_eq!(first.unwrap()["thread"]["id"], CHAT);
        assert_eq!(other.unwrap()["thread"]["id"], OTHER);
        assert_eq!(next.unwrap()["thread"]["title"], "新连接");
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        connection.close();
        next_connection.close();
    }

    #[tokio::test]
    async fn failed_native_read_reaches_all_waiters_and_is_not_cached() {
        let (_fixture, desktop, peer, connection) = read_fixture().await;
        peer.hold_reads.store(true, Ordering::Release);
        peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["hostId"] = json!("remote");
        let mut first = Box::pin(desktop.native_read(&connection, CHAT));
        let mut second = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut second).is_pending());
        peer.wait_calls("read_thread", 1).await;
        peer.release_reads.add_permits(1);
        let (first, second) = tokio::join!(first, second);
        for result in [first, second] {
            let error = result.unwrap_err();
            assert_eq!(error.status, 403);
            assert!(error.before_send);
        }
        assert_eq!(peer.count("read_thread").await, 1);
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        assert!(!desktop.native_threads.read().await.contains_key(CHAT));

        peer.hold_reads.store(false, Ordering::Release);
        peer.reads.lock().await.get_mut(CHAT).unwrap()["thread"]["hostId"] = json!("local");
        desktop.native_read(&connection, CHAT).await.unwrap();
        assert_eq!(peer.count("read_thread").await, 2);
        connection.close();
    }

    #[tokio::test]
    async fn cancelling_one_native_reader_preserves_other_waiters() {
        let (_fixture, desktop, peer, connection) = read_fixture().await;
        peer.hold_reads.store(true, Ordering::Release);
        let mut first = Box::pin(desktop.native_read(&connection, CHAT));
        let mut second = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut second).is_pending());
        peer.wait_calls("read_thread", 1).await;
        drop(first);
        peer.release_reads.add_permits(1);
        assert_eq!(second.await.unwrap()["thread"]["id"], CHAT);
        assert_eq!(peer.count("read_thread").await, 1);
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        connection.close();
    }

    #[tokio::test]
    async fn cancelling_all_native_readers_releases_the_request() {
        let (_fixture, desktop, peer, connection) = read_fixture().await;
        peer.hold_reads.store(true, Ordering::Release);
        let mut first = Box::pin(desktop.native_read(&connection, CHAT));
        let mut second = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut second).is_pending());
        peer.wait_calls("read_thread", 1).await;
        std::thread::scope(|scope| {
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let concurrent = barrier.clone();
            scope.spawn(move || {
                concurrent.wait();
                drop(first);
            });
            barrier.wait();
            drop(second);
        });
        assert!(desktop.native_reads.lock().unwrap().is_empty());

        let mut retry = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut retry).is_pending());
        peer.wait_calls("read_thread", 2).await;
        peer.release_reads.add_permits(2);
        assert_eq!(retry.await.unwrap()["thread"]["id"], CHAT);
        assert_eq!(peer.count("read_thread").await, 2);
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        connection.close();
    }

    #[tokio::test]
    async fn disconnected_native_read_fails_all_waiters_without_authorizing() {
        let (_fixture, desktop, peer, connection) = read_fixture().await;
        peer.hold_reads.store(true, Ordering::Release);
        let mut first = Box::pin(desktop.native_read(&connection, CHAT));
        let mut second = Box::pin(desktop.native_read(&connection, CHAT));
        assert!(futures_util::poll!(&mut first).is_pending());
        assert!(futures_util::poll!(&mut second).is_pending());
        peer.wait_calls("read_thread", 1).await;
        connection.close();
        let (first, second) = tokio::join!(first, second);
        assert!(first.is_err());
        assert!(second.is_err());
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        assert!(!desktop.native_threads.read().await.contains_key(CHAT));
    }

    #[tokio::test]
    async fn accepted_send_refreshes_again_after_an_overlapping_native_read() {
        let (_fixture, desktop, peer, connection) = read_fixture().await;
        let mut live = desktop.subscribe(CHAT);
        peer.wait_calls("read_thread", 2).await;
        // 首帧和安装监听后的核对均结束后，再制造发送与读取的实际重叠。
        tokio::time::timeout(Duration::from_secs(5), async {
            while !desktop.native_reads.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        peer.hold_sends.store(true, Ordering::Release);
        let send = {
            let desktop = desktop.clone();
            tokio::spawn(async move {
                desktop
                    .send_message(CHAT, "合成输入", "send-test", &[], &MessageModel::default())
                    .await
            })
        };
        peer.wait_calls("send_message_to_thread", 1).await;
        peer.hold_reads.store(true, Ordering::Release);
        desktop
            .follows
            .lock()
            .unwrap()
            .get(CHAT)
            .unwrap()
            .refresh
            .notify_one();
        peer.wait_calls("read_thread", 4).await;
        peer.release_sends.add_permits(1);
        assert_eq!(send.await.unwrap().unwrap()["status"], "accepted");
        peer.release_reads.add_permits(1);
        peer.wait_calls("read_thread", 5).await;
        peer.reads.lock().await.get_mut(CHAT).unwrap()["turns"] = json!([
            {"id":"confirmed-turn","startedAt":100,"status":"completed","items":[]}
        ]);
        peer.release_reads.add_permits(1);
        tokio::time::timeout(
            Duration::from_secs(5),
            live.wait_for(|view| {
                view.snapshot
                    .as_ref()
                    .is_some_and(|snapshot| snapshot["firstTurnObserved"] == "confirmed-turn")
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(peer.count("read_thread").await, 5);
        drop(live);
        tokio::time::timeout(Duration::from_secs(3), async {
            while desktop.follows.lock().unwrap().contains_key(CHAT) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(desktop.native_reads.lock().unwrap().is_empty());
        desktop.close().await;
        connection.close();
    }

    #[test]
    fn native_summary_updates_status_but_never_replaces_full_saved_content() {
        let history = json!({"thread":{"id":"chat","model":"saved-model"},"nextCursor":"older","turns":[{"id":"turn","startedAt":10,"status":"inProgress","items":[{"id":"user","type":"userMessage","text":"真实输入"},{"id":"reply","type":"agentMessage","text":"完整正文"}]}]});
        let native = json!({"thread":{"id":"chat","status":{"type":"idle"}},"turns":[{"id":"turn","startedAt":10,"status":"completed","items":[{"id":"reply","type":"agentMessage","text":"截断的摘要"}]}]});
        let merged = merge_native_snapshot(history, native);
        assert_eq!(merged["turns"][0]["items"][1]["text"], "完整正文");
        assert_eq!(merged["turns"][0]["items"][0]["text"], "真实输入");
        assert_eq!(merged["turns"][0]["status"], "completed");
        assert_eq!(merged["nextCursor"], "older");
        assert_eq!(merged["thread"]["model"], "saved-model");
        assert_eq!(merged["firstTurnObserved"], "turn");
    }

    #[test]
    fn native_questions_remain_readable_until_an_authoritative_answer() {
        let history = json!({"thread":{"id":"chat"},"turns":[{"id":"turn","startedAt":10,"items":[{"id":"question","type":"agentMessage","questions":[{"title":"选择颜色","options":["蓝色","绿色"]}]}]}]});
        let mut merged = merge_native_snapshot(history, json!({"turns":[]}));
        assert_eq!(
            merged["interactions"][0]["questions"][0]["question"],
            "选择颜色"
        );
        assert_eq!(merged["interactions"][0]["replySupported"], true);
        assert_eq!(merged["interactions"][0]["skipSupported"], false);
        let reply = format!(
            "<send_user_message_question_reply>{}</send_user_message_question_reply>",
            json!([{"questionItemId":merged["interactions"][0]["questions"][0]["id"],"question":"选择颜色","answer":"蓝色"}])
        );
        // 委派工具文字或模型确认不能冒充官方问题生命周期结束。
        merged["turns"][0]["items"]
            .as_array_mut()
            .unwrap()
            .push(json!({"id":"delegation","type":"functionCallOutput","output":reply}));
        merged = merge_native_snapshot(merged, json!({"turns":[]}));
        assert_eq!(merged["interactions"].as_array().unwrap().len(), 1);
        merged["turns"][0]["items"].as_array_mut().unwrap().push(
            json!({"id":"answer","type":"userMessage","content":[{"type":"text","text":reply}]}),
        );
        merged = merge_native_snapshot(merged, json!({"turns":[]}));
        assert!(merged["interactions"].as_array().unwrap().is_empty());
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct QueueChange {
    pub revision: String,
    #[serde(flatten)]
    pub operation: QueueOperation,
}
#[derive(Deserialize, Serialize, Debug)]
#[serde(
    tag = "action",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum QueueOperation {
    Delete { message_id: String },
    Reorder { message_ids: Vec<String> },
    Edit { message_id: String, prompt: String },
    Send { message_id: String },
}
impl QueueChange {
    pub fn parse(value: Value) -> Result<Self> {
        let change: Self = serde_json::from_value(value)
            .map_err(|_| BridgeError::invalid("队列操作无效，请刷新后重试。"))?;
        let valid = |id: &str| !id.is_empty() && id.len() <= 200;
        let ids = match &change.operation {
            QueueOperation::Reorder { message_ids } => message_ids.iter().all(|s| valid(s)),
            QueueOperation::Delete { message_id }
            | QueueOperation::Edit { message_id, .. }
            | QueueOperation::Send { message_id } => valid(message_id),
        };
        if change.revision.len() != 64
            || !change
                .revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !ids
        {
            return Err(BridgeError::invalid("队列操作无效，请刷新后重试。"));
        }
        Ok(change)
    }
}
