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
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{watch, Mutex, RwLock};
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
}

pub struct Desktop {
    tasks: TaskTracker,
    pub paths: Paths,
    pub history: Arc<History>,
    pub session: Arc<Session>,
    owns_session: bool,
    catalog_hydration: Mutex<()>,
    native_threads: RwLock<HashMap<String, Value>>,
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
            catalog_instance: Mutex::new(None),
            follows: std::sync::Mutex::new(HashMap::new()),
            queue_locks: Mutex::new(HashMap::new()),
            cancel: CancellationToken::new(),
            last_native_error: RwLock::new(None),
        })
    }
    pub async fn sync_status(&self) -> Value {
        if !crate::runtime::SHARED_RUNTIME_VERIFIED && self.session.status()["native"] != true {
            return json!({"state":"history-only","message":"本版本仅提供本机历史阅读；任务操作请在官方 Codex 完成。"});
        }
        if self.session.status()["connected"] != true {
            return json!({"state":"waiting","message":self.last_native_error.read().await.as_deref().unwrap_or("桌面未接入，已保存的历史仍可阅读。")});
        }
        match self.last_native_error.read().await.as_ref() {
            None => {
                json!({"state":"connected","message":"已连接官方桌面，支持文字发送和聊天同步。"})
            }
            Some(error) => json!({"state":"error","message":error}),
        }
    }
    pub async fn probe(&self) -> Result<()> {
        if !crate::native::eligible(&self.paths) && !crate::runtime::SHARED_RUNTIME_VERIFIED {
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
        self.require_route(&connection, thread).await?;
        let payload = if connection.is_native() {
            Self::native_call(&connection, thread, tool, args, request_id).await?
        } else {
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
    async fn native_call(
        connection: &AppServerConnection,
        source: &str,
        tool: &str,
        args: Value,
        request_id: Option<&str>,
    ) -> Result<Value> {
        let call_id = request_id
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let result = connection
            .request(
                "tools/call",
                json!({
                    "callerSource":"codex", "hostId":"local", "threadId":source,
                    "turnId":format!("mobile-operation-{call_id}"), "callId":call_id,
                    "namespace":"codex_app", "tool":tool, "arguments":args,
                }),
                Duration::from_secs(60),
            )
            .await?;
        let text = result["contentItems"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| item["type"] == "inputText")
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if result["success"] != true {
            return Err(BridgeError::unavailable(if text.is_empty() {
                "官方桌面未接受请求。".into()
            } else {
                text
            }));
        }
        serde_json::from_str(&text)
            .map_err(|_| BridgeError::unavailable("官方桌面回执无法识别，请核对操作结果。"))
    }
    async fn native_read(&self, connection: &AppServerConnection, id: &str) -> Result<Value> {
        if uuid::Uuid::parse_str(id).is_err() {
            return Err(BridgeError::invalid("聊天编号无效。").before_send());
        }
        let payload = Self::native_call(connection, id, "read_thread", json!({
            "threadId":id,"hostId":"local","turnLimit":5,"includeOutputs":false,"maxOutputCharsPerItem":20000,
        }), None).await.map_err(BridgeError::before_send)?;
        if payload["schemaVersion"] != 1
            || payload["thread"]["id"] != id
            || payload["thread"]["kind"] != "codex"
            || payload["thread"]["hostId"] != "local"
            || !payload["turns"].is_array()
        {
            return Err(
                BridgeError::new(403, "官方桌面未确认此聊天的本机归属，保留历史阅读。")
                    .before_send(),
            );
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
        prompt: &str,
        request_id: &str,
        title: Option<&str>,
        selection: &MessageModel,
    ) -> Result<Value> {
        self.session.require("create")?;
        let projects = self
            .scoped_projects(source)
            .await
            .map_err(BridgeError::before_send)?;
        if !projects.iter().any(|row| row["projectId"] == project) {
            return Err(BridgeError::invalid("该官方本机项目已不可用，请重新选择。").before_send());
        }
        let connection = self.session.connect().await?;
        if !connection.is_native() {
            return Err(BridgeError::unavailable("新建需要官方原生桌面入口。").before_send());
        }
        let mut args = json!({
            "prompt":prompt, "target":{"type":"project","projectId":project,"environment":{"type":"local"}},
        });
        if let Some(title) = title {
            args["title"] = json!(title);
        }
        selection.apply(&mut args);
        let result =
            Self::native_call(&connection, source, "create_thread", args, Some(request_id)).await?;
        let id = result["threadId"]
            .as_str()
            .filter(|id| uuid::Uuid::parse_str(id).is_ok());
        if result["hostId"] != "local" || id.is_none() || result.get("clientThreadId").is_some() {
            return Err(BridgeError::unavailable(
                "创建尚未返回正式本机聊天 ID，请在桌面核对；不会再次创建。",
            ));
        }
        Ok(json!({"status":"created","threadId":id,"firstTurnAccepted":false}))
    }
    pub async fn browser_inventory(self: &Arc<Self>, thread: &str) -> Result<Value> {
        let (_, connection) = self.connected_follow(thread).await?;
        connection.request(
            "mcpServer/tool/call",
            json!({"threadId":thread,"server":"cua_repl","tool":"js","arguments":{"code":"await cua.getState();","title":"Reading browser tabs"}}),
            Duration::from_secs(60),
        ).await
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
        self.authorize_on(&connection, id).await
    }
    async fn authorize_on(&self, connection: &AppServerConnection, id: &str) -> Result<()> {
        self.check_instance(connection).await;
        self.require_route(connection, id).await?;
        if connection.is_native() {
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
    async fn require_route(&self, connection: &AppServerConnection, id: &str) -> Result<()> {
        if connection.is_native() {
            self.native_read(connection, id).await?;
            return Ok(());
        }
        let route = connection
            .rpc("codexMobile/thread/route", json!({"threadId":id}))
            .await
            .map_err(BridgeError::before_send)?;
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
        if connection.is_native() {
            return self.native_snapshot(&connection, id, None).await;
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
                self.session.require("readLive")?;
                let connection = self.session.connect().await?;
                if connection.is_native() {
                    self.check_instance(&connection).await;
                    return self.follow_native(&connection, &id, &follow).await;
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
                follow
                    .state
                    .write()
                    .await
                    .hydrate(&resume, &connection.id, sequence)?;
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
                let mut dirty = false;
                let mut queue_dirty = false;
                loop {
                    tokio::select! {
                        _=idle.tick()=>{
                            if follow.updates.receiver_count()==0 && Arc::strong_count(&follow)<=2 {
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
            if self.cancel.is_cancelled() || attempt.is_ok() {
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
    pub async fn update_settings(self: &Arc<Self>, _id: &str, _changes: Value) -> Result<Value> {
        Err(BridgeError::unavailable("手机设置当前仅供查看，请在桌面修改。").before_send())
    }
    pub async fn interrupt(self: &Arc<Self>, id: &str, turn_id: &str) -> Result<Value> {
        // 暂停目标与中断轮次缺少已验收的组合语义，不能宣称全部停止。
        let _ = (id, turn_id);
        Err(BridgeError::unavailable("请在桌面停止任务及目标。").before_send())
    }
    pub async fn resume_turn(&self, _id: &str, _turn_id: &str) -> Result<Value> {
        Err(BridgeError::unavailable("请在桌面继续异常任务。").before_send())
    }
    pub async fn reply(
        self: &Arc<Self>,
        _id: &str,
        _request_id: &str,
        _input: &Value,
    ) -> Result<Value> {
        Err(BridgeError::unavailable("请在桌面处理问题和审批；手机保留答案草稿。").before_send())
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
    pub async fn queue(&self, id: &str) -> Result<Value> {
        self.session.require("queue")?;
        let connection = self.session.connect().await?;
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
        let QueueOperation::Delete { message_id } = change.operation else {
            return Err(BridgeError::unavailable(
                "手机仅支持删除待执行项；编辑、排序和立即发送请在桌面处理。",
            )
            .before_send());
        };
        let lock = self.queue_lock(id).await;
        let _guard = lock.lock().await;
        let (_, connection) = self.connected_follow(id).await?;
        let rows = Self::queue_rows(&connection, id).await?;
        if Self::queue_snapshot(&rows)["revision"] != change.revision
            || !rows.iter().any(|row| row["id"] == message_id)
        {
            return Err(BridgeError::new(409, "队列已变化，请刷新确认。").before_send());
        }
        connection
            .rpc(
                "thread/queue/delete",
                json!({"threadId":id,"queuedSubmissionId":message_id}),
            )
            .await?;
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
        if connection.is_native() {
            self.require_route(&connection, id).await?;
            let mut args = json!({"threadId":id,"hostId":"local","prompt":crate::images::context_prompt(prompt, images)});
            selection.apply(&mut args);
            let result = Self::native_call(
                &connection,
                id,
                "send_message_to_thread",
                args,
                Some(message_id),
            )
            .await?;
            if result["threadId"] != id {
                return Err(BridgeError::unavailable(
                    "发送回执的聊天编号不一致，请在桌面核对。",
                ));
            }
            return Ok(json!({"status":"accepted","threadId":id,"messageId":message_id}));
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
            return Self::enqueue_on(&connection, id, prompt, message_id, images).await;
        }
        connection.rpc("turn/start",json!({"threadId":id,"input":Self::input(prompt,images),"clientUserMessageId":message_id})).await
    }
    async fn native_snapshot(
        &self,
        connection: &AppServerConnection,
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
    history["turns"] = Value::Array(turns);
    history["connection"] = json!("live");
    // 沿用旧客户端识别原生桌面快照及首轮回执的协议值。
    history["syncMode"] = json!("desktop-poll");
    history["firstTurnObserved"] = payload["turns"]
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
        .unwrap_or(Value::Null);
    history["interactions"] = json!([]);
    history
}

#[cfg(test)]
mod native_tests {
    use super::*;
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
