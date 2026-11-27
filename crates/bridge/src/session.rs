//! Lifecycle for the single official App Server connection used by this Bridge.
use crate::{
    app_server::{AppServerConnection, AppServerEvent},
    paths::Paths,
    BridgeError, Result,
};
use serde_json::{json, Value};
use std::{
    path::Path,
    sync::{Arc, RwLock},
};
use tokio::sync::{broadcast, Mutex};
use tokio_util::sync::CancellationToken;

pub struct Session {
    paths: Paths,
    #[cfg(test)]
    shared_fixture: std::sync::atomic::AtomicBool,
    current: RwLock<Option<Arc<AppServerConnection>>>,
    connecting: Mutex<()>,
    pub events: broadcast::Sender<AppServerEvent>,
    pub connections: broadcast::Sender<Arc<AppServerConnection>>,
    cancel: CancellationToken,
}

impl Session {
    // 仅供合成协议回归：仍走真实的实例、目录、路由和连接校验；发行版无此入口。
    #[cfg(test)]
    pub(crate) fn enable_shared_fixture(&self) {
        self.shared_fixture
            .store(true, std::sync::atomic::Ordering::Release);
    }
    #[cfg(test)]
    pub(crate) fn install_native(&self, connection: Arc<AppServerConnection>) {
        assert!(
            connection.is_alive()
                && connection
                    .native_tools()
                    .is_some_and(|native| native.owner_valid())
        );
        *self.current.write().unwrap() = Some(connection);
    }

    pub fn new(paths: Paths) -> Arc<Self> {
        let (events, _) = broadcast::channel(1024);
        let (connections, _) = broadcast::channel(16);
        Arc::new(Self {
            paths,
            #[cfg(test)]
            shared_fixture: std::sync::atomic::AtomicBool::new(false),
            current: RwLock::new(None),
            connecting: Mutex::new(()),
            events,
            connections,
            cancel: CancellationToken::new(),
        })
    }
    pub async fn connect(&self) -> Result<Arc<AppServerConnection>> {
        let _connecting = self.connecting.lock().await;
        if self.cancel.is_cancelled() {
            return Err(BridgeError::unavailable("Codex Mobile 已停止连接。").before_send());
        }
        let current = self.current.read().unwrap().clone();
        #[cfg(test)]
        if self
            .shared_fixture
            .load(std::sync::atomic::Ordering::Acquire)
        {
            if let Some(connection) = current.as_ref().filter(|connection| connection.is_alive()) {
                return Ok(connection.clone());
            }
        }
        let shared = if crate::runtime::SHARED_RUNTIME_VERIFIED {
            crate::runtime::read_runtime(&self.paths.runtime_home)
                .ok()
                .flatten()
        } else {
            None
        };
        if let Some(connection) = current.as_ref().filter(|c| {
            c.is_alive()
                && c.native_tools().is_some_and(|native| native.owner_valid())
                && shared.as_ref().map_or(c.is_native(), |runtime| {
                    c.runtime_id().as_deref() == Some(&runtime.instance_id)
                })
        }) {
            return Ok(connection.clone());
        }
        let (official, runtime) = match AppServerConnection::connect_native(&self.paths).await {
            Ok(connection) => (Some(connection), shared),
            Err(native_error) => {
                let runtime = crate::runtime::read_runtime(&self.paths.runtime_home)
                    .map_err(BridgeError::before_send)?
                    .ok_or_else(|| {
                        if crate::native::eligible(&self.paths) {
                            return native_error.before_send();
                        }
                        BridgeError::unavailable("请通过 Codex Mobile 桌面程序启用共享连接。")
                            .before_send()
                    })?;
                (None, Some(runtime))
            }
        };
        let connection = match runtime {
            None => official.unwrap(),
            Some(runtime) => {
                if official.is_none() {
                    if let Some(connection) = current.as_ref().filter(|c| {
                        c.is_alive() && c.runtime_id().as_deref() == Some(&runtime.instance_id)
                    }) {
                        return Ok(connection.clone());
                    }
                }
                let connection =
                    AppServerConnection::connect_broker(&runtime.url, &runtime.auth_token).await?;
                let verified = crate::runtime::supported_initialize(&connection.initialized);
                #[cfg(test)]
                let verified = verified
                    || self
                        .shared_fixture
                        .load(std::sync::atomic::Ordering::Acquire);
                if !verified {
                    connection.close();
                    return Err(BridgeError::unavailable(
                        "当前官方协议尚未验收，手机仅提供历史只读。",
                    )
                    .before_send());
                }
                let home = connection.initialized["codexHome"].as_str().map(Path::new);
                let expected = std::fs::canonicalize(&self.paths.codex_home).ok();
                if home
                    .and_then(|p| std::fs::canonicalize(p).ok())
                    .zip(expected)
                    .is_none_or(|(actual, expected)| actual != expected)
                {
                    connection.close();
                    return Err(BridgeError::unavailable(
                        "共享连接使用了其他 Codex 目录，请重新打开桌面连接。",
                    )
                    .before_send());
                }
                if let Err(error) = runtime.validate(&self.paths.runtime_home) {
                    connection.close();
                    return Err(error.before_send());
                }
                connection.bind_runtime(self.paths.runtime_home.clone(), runtime);
                if let Some(official) = official {
                    if let Err(error) = connection.bind_official(official) {
                        connection.close();
                        return Err(error);
                    }
                }
                connection
            }
        };
        let mut incoming = connection.subscribe();
        let outgoing = self.events.clone();
        let cancel = self.cancel.clone();
        let generation = connection.id.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _=cancel.cancelled()=>break,
                    event=incoming.recv()=>match event {
                        Ok(event)=>{
                            let disconnected=matches!(event, AppServerEvent::Disconnected { .. });
                            let _=outgoing.send(event);
                            if disconnected { break; }
                        }
                        Err(broadcast::error::RecvError::Lagged(_))=>{
                            // Each view reloads its official snapshot after a gap.
                            let _=outgoing.send(AppServerEvent::Notification {
                                connection_id:generation.clone(),
                                sequence:0,
                                message:serde_json::json!({"method":"codexMobile/resync","params":{}})
                            });
                        }
                        Err(broadcast::error::RecvError::Closed)=>break,
                    }
                }
            }
        });
        let old = self.current.write().unwrap().replace(connection.clone());
        if let Some(old) = old {
            old.close();
        }
        let _ = self.connections.send(connection.clone());
        Ok(connection)
    }
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        // Connection establishment is retriable; a sent RPC is never replayed.
        self.connect().await?.rpc(method, params).await
    }
    pub fn status(&self) -> Value {
        // 连接探测可能耗时，状态只读取当前已建立的连接。
        let current = self.current.read().unwrap().clone();
        let connection = current.as_ref().filter(|connection| {
            connection.is_alive()
                && (connection.is_native() && connection.owner_valid()
                    || crate::runtime::read_runtime(&self.paths.runtime_home)
                        .ok()
                        .flatten()
                        .is_some_and(|runtime| {
                            connection.runtime_id().as_deref() == Some(&runtime.instance_id)
                        }))
        });
        let version = connection
            .and_then(|connection| connection.initialized["userAgent"].as_str())
            .and_then(|agent| agent.split_once('/'))
            .and_then(|(_, rest)| rest.split_whitespace().next());
        json!({"connected":connection.is_some(),"native":connection.is_some_and(|c| c.native_tools().is_some()),
            "controls":connection.is_some_and(|c| !c.is_native() && c.native_tools().is_some()),"version":version})
    }
    pub fn capabilities(&self) -> Value {
        let status = self.status();
        #[cfg(test)]
        if self
            .shared_fixture
            .load(std::sync::atomic::Ordering::Acquire)
        {
            let available = status["connected"] == true && !self.cancel.is_cancelled();
            return json!({"native":status["native"]==true,"create":status["native"]==true,"asyncReplies":status["native"]==true,
                "readLive":available,"send":available,"images":available,
                "queue":available,"queueStart":available,"queueReorder":available,"queueEdit":available,
                "interrupt":available,"interactions":available,"resume":available,"settings":available,"goals":available});
        }
        let native = status["native"] == true;
        let connected = native && status["connected"] == true;
        let controls =
            connected && crate::runtime::SHARED_RUNTIME_VERIFIED && status["controls"] == true;
        let desktop_navigation = connected
            && self
                .current
                .read()
                .unwrap()
                .as_ref()
                .and_then(|connection| connection.native_tools())
                .is_some_and(|connection| {
                    connection.is_alive()
                        && connection.owner_valid()
                        && crate::native::navigation_supported(&connection.initialized)
                });
        let management = |name| {
            connected
                && self
                    .current
                    .read()
                    .unwrap()
                    .as_ref()
                    .and_then(|connection| connection.native_tools())
                    .is_some_and(|connection| {
                        crate::native::management_supported(&connection.initialized, name)
                    })
        };
        // 全局桌面目录没有无上下文入口，不借任意聊天或临时控制线程代替。
        json!({"readLive":connected,"send":connected,"queue":controls,"catalog":false,
            "native":native,"create":native,"images":controls && self.current.read().unwrap().as_ref()
                .is_some_and(|connection| connection.initialized["codexMobileNativeImages"] == true),
            "imageReferences":false,
            "desktopNavigation":desktop_navigation,
            "contextReferences":connected,
            "contextPlugins":connected,"contextBrowser":controls,"delegation":connected,
            "threadRename":management("set_thread_title"),"threadPin":management("set_thread_pinned"),
            "threadArchive":management("set_thread_archived"),"threadDelete":management("delete_thread"),
            "modelSelection":native && self.model_efforts().is_some(),
            "resume":connected && native,"resumeInterrupted":connected && native,"asyncReplies":connected && native,
            "queueStart":controls,"queueReorder":controls,"queueEdit":controls,
            "interrupt":controls,"interactions":controls,"settings":controls,"goals":controls})
    }
    pub fn model_efforts(&self) -> Option<Vec<String>> {
        self.current
            .read()
            .unwrap()
            .as_ref()
            .and_then(|connection| connection.native_tools())
            .filter(|connection| {
                connection.is_native() && connection.is_alive() && connection.owner_valid()
            })
            .and_then(|connection| crate::native::model_efforts(&connection.initialized))
    }
    pub fn require(&self, capability: &str) -> Result<()> {
        let capabilities = self.capabilities();
        if capabilities[capability] != true {
            if capability == "images" {
                return Err(BridgeError::unavailable("原生图片控制连接尚未就绪；请在当前任务结束后从新版 Codex Mobile 打开 Codex，再重试。图片草稿已保留。").before_send());
            }
            if capability == "interactions" && capabilities["native"] == true {
                return Err(
                    BridgeError::new(409, "此审批的原始控制连接已失效，请等待重新同步。")
                        .before_send(),
                );
            }
            return Err(BridgeError::unavailable(
                "此操作尚未开放，请在官方 Codex 桌面处理；草稿会保留。",
            )
            .before_send());
        }
        Ok(())
    }
    pub async fn close(&self) {
        self.cancel.cancel();
        let _connecting = self.connecting.lock().await;
        if let Some(connection) = self.current.write().unwrap().take() {
            connection.close();
        }
    }
}
