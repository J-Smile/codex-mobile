use crate::{
    auth::{Auth, RelayPhone},
    config::ServiceConfig,
    http::{Bridge, Server},
    paths::Paths,
    relay::RemoteAccess,
    runtime,
    updates::{Candidate, Updates},
    BridgeError, Result,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{watch, Mutex, RwLock};

pub struct Host {
    pub paths: Paths,
    pub updates: Updates,
    config: RwLock<ServiceConfig>,
    relay: Arc<RemoteAccess>,
    server: Mutex<Option<Server>>,
    service_error: RwLock<Option<String>>,
    desktop_connection: Mutex<Option<Value>>,
    desktop_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    pub update_state: RwLock<Value>,
    pub candidate: Mutex<Option<Candidate>>,
    mobile_update: watch::Sender<bool>,
}

impl Host {
    pub fn open(paths: Paths, packaged: bool) -> Result<Arc<Self>> {
        let config = ServiceConfig::load(&paths.data_home)?;
        let relay = Arc::new(RemoteAccess::open(paths.data_home.clone())?);
        let updates = Updates::new(paths.data_home.clone(), packaged)?;
        let last_error = if packaged {
            crate::paths::read_json_optional(
                &paths.data_home.join("updates/native/last-error.json"),
            )?
        } else {
            None
        };
        let update_state = match last_error {
            Some(error) => {
                json!({"phase":"error","message":format!("上次更新未完成，当前继续运行 v{}。{}",env!("CARGO_PKG_VERSION"),error["message"].as_str().unwrap_or(""))})
            }
            None => json!({"phase":"idle","message":""}),
        };
        Ok(Arc::new(Self {
            paths,
            updates,
            config: RwLock::new(config),
            relay,
            server: Mutex::new(None),
            service_error: RwLock::new(None),
            desktop_connection: Mutex::new(None),
            desktop_task: Mutex::new(None),
            update_state: RwLock::new(update_state),
            candidate: Mutex::new(None),
            mobile_update: watch::Sender::new(false),
        }))
    }
    pub async fn auto_start(&self) -> bool {
        self.config.read().await.auto_start
    }
    pub async fn start(self: &Arc<Self>) -> Result<()> {
        let mut server = self.server.lock().await;
        if server.is_some() {
            return Ok(());
        }
        let result = async {
            let app = Bridge::open_with_host(
                self.paths.clone(),
                self.config.read().await.clone(),
                self.updates.packaged,
                Arc::downgrade(self),
                self.relay.clone(),
            )?;
            app.start().await
        }
        .await;
        match result {
            Ok(started) => {
                *server = Some(started);
                *self.service_error.write().await = None;
                Ok(())
            }
            Err(error) => {
                *self.service_error.write().await = Some(error.message.clone());
                Err(error)
            }
        }
    }
    pub async fn stop(&self) -> Result<()> {
        let mut connection_task = self.desktop_task.lock().await;
        if let Some(task) = connection_task.take() {
            task.abort();
            let _ = task.await;
        }
        let mut server = self.server.lock().await;
        if let Some(server) = server.take() {
            server.stop().await?;
        }
        *self.service_error.write().await = None;
        *self.desktop_connection.lock().await = None;
        Ok(())
    }
    pub async fn app(&self) -> Option<Arc<Bridge>> {
        self.server.lock().await.as_ref().map(|s| s.app.clone())
    }
    pub async fn status(&self) -> Result<Value> {
        let app = self.app().await;
        let observed = match &app {
            Some(app) => app.desktop.sync_status().await,
            None => json!({"state":"stopped","message":"启动连接服务后，将连接官方 Codex。"}),
        };
        let sync = {
            let mut connecting = self.desktop_connection.lock().await;
            // 后台探测恢复后，启动错误不再覆盖真实状态；后续断线也不复活旧错误。
            if observed["state"] == "connected"
                && connecting
                    .as_ref()
                    .is_some_and(|status| status["state"] == "error")
            {
                *connecting = None;
            }
            connecting.clone().unwrap_or(observed)
        };
        let runtime = json!({"ready":app.as_ref().is_some_and(|app| app.session.capabilities()["readLive"] == true),"message":sync["message"]});
        let error = self.service_error.read().await.clone();
        let relay = self.relay.status().await;
        Ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"service":{"running":app.is_some(),"error":error},"sync":sync,"runtime":runtime,"desktopLaunch":runtime::desktop_launch_status(&self.paths),"relay":relay,"config":self.config.read().await.clone(),"update":self.update_state.read().await.clone(),"updateSettings":self.updates.status(),"release":self.updates.release.read().await.clone()}),
        )
    }
    pub async fn save_config(self: &Arc<Self>, config: ServiceConfig) -> Result<()> {
        config.save(&self.paths.data_home)?;
        let previous = self.config.read().await.clone();
        if previous.port == config.port
            && previous.bind == config.bind
            && previous.public_url == config.public_url
        {
            *self.config.write().await = config;
            return Ok(());
        }
        let running = self.app().await.is_some();
        if running {
            self.stop().await?;
        }
        *self.config.write().await = config;
        if running {
            self.start().await?;
        }
        Ok(())
    }
    pub async fn pairing(&self) -> Result<Value> {
        if let Some(app) = self.app().await {
            return Ok(app.pairing().await);
        }
        let auth = Auth::open(self.paths.data_home.clone())?;
        let config = self.config.read().await;
        Ok(
            json!({"accessKey":auth.key(),"urls":crate::auth::network_urls(config.port),"publicUrl":config.public_url}),
        )
    }
    pub async fn rotate_key(&self) -> Result<Value> {
        if let Some(app) = self.app().await {
            app.rotate_local().await?;
        } else {
            Auth::open(self.paths.data_home.clone())?.rotate_local()?;
            crate::notifications::Notifications::open(self.paths.data_home.clone())?
                .clear_lan()
                .await?;
        }
        self.pairing().await
    }
    pub async fn configure_relay(&self, url: &str, name: &str, token: &str) -> Result<()> {
        self.relay.configure(url, name, token).await
    }
    pub async fn relay_pairing(&self) -> Result<Value> {
        self.relay.pairing().await
    }
    pub async fn relay_approval(&self, login_key: Option<&str>) -> Result<Value> {
        self.relay.start_approval(login_key).await
    }
    pub async fn link_relay_desktop(self: &Arc<Self>) -> Result<Value> {
        self.start().await?;
        self.relay.link_from_phone().await
    }
    pub async fn decide_relay_approval(&self, request: &str, approve: bool) -> Result<()> {
        self.relay.decide_approval(request, approve).await
    }
    pub async fn revoke_relay_phone(&self, phone: &str) -> Result<()> {
        let server = self.server.lock().await;
        self.relay.revoke(phone).await?;
        let phone = RelayPhone::new(phone);
        match &*server {
            Some(server) => {
                server
                    .app
                    .notifications
                    .remove_for_owner(phone.owner(), None)
                    .await?;
            }
            None => {
                crate::notifications::Notifications::open(self.paths.data_home.clone())?
                    .remove_for_owner(phone.owner(), None)
                    .await?;
            }
        }
        Ok(())
    }
    pub async fn configure_desktop_launch(&self, binaries: PathBuf, enabled: bool) -> Result<()> {
        let task = self.desktop_task.lock().await;
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return Err(BridgeError::unavailable(
                "正在连接 Codex，请连接结束后再修改启动配置。",
            ));
        }
        let paths = self.paths.clone();
        tokio::task::spawn_blocking(move || {
            runtime::configure_desktop_launch(&paths, &binaries, enabled)
        })
        .await
        .map_err(|error| BridgeError::unavailable(error.to_string()))?
    }
    pub async fn connect_desktop(self: &Arc<Self>, binaries: PathBuf) {
        let mut task = self.desktop_task.lock().await;
        if task.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        if let Err(error) = self.start().await {
            *self.desktop_connection.lock().await =
                Some(json!({"state":"error","message":error.message}));
            return;
        }
        let Some(app) = self.app().await else {
            return;
        };
        *self.desktop_connection.lock().await =
            Some(json!({"state":"connecting","message":"正在连接 Codex…"}));
        let host = self.clone();
        *task = Some(tokio::spawn(async move {
            let result = tokio::time::timeout(Duration::from_secs(45), async {
                if host.updates.packaged {
                    let paths = host.paths.clone();
                    let binaries = binaries.clone();
                    tokio::task::spawn_blocking(move || runtime::ensure_desktop_launch(&paths, &binaries))
                        .await.map_err(|error| BridgeError::unavailable(error.to_string()))??;
                }
                // 复用无聊天副作用的连接探测，不借来源聊天读取项目。
                if app.desktop.probe().await.is_ok() {
                    return Ok(());
                }
                let prepared = runtime::prepare_desktop(&host.paths, &binaries).await?;
                if prepared["status"] == "history-only" {
                    return Ok(());
                }
                if prepared["status"] == "restart-required" {
                    return Err(BridgeError::unavailable(
                        if runtime::desktop_launch_status(&host.paths)["enabled"] == true {
                            "自动接入已配置。请在当前任务结束后正常重开一次 Codex；以后两个应用可按任意顺序打开。"
                        } else {
                            "Codex 已在运行，但尚未接入共享连接。请退出 Codex 后点击“重新连接”。"
                        },
                    ));
                }
                let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
                loop {
                    match app.desktop.probe().await {
                        Ok(_) => return Ok(()),
                        Err(error)
                            if prepared["status"] != "starting"
                                || tokio::time::Instant::now() >= deadline =>
                        {
                            return Err(error)
                        }
                        Err(_) => tokio::time::sleep(Duration::from_millis(500)).await,
                    }
                }
            })
            .await
            .unwrap_or_else(|_| {
                Err(BridgeError::unavailable(
                    "连接 Codex 超时，请检查 Codex 窗口后重试。",
                ))
            });
            *host.desktop_connection.lock().await = result
                .err()
                .map(|error| json!({"state":"error","message":error.message}));
        }));
    }
    pub(crate) fn request_mobile_update(&self) -> Result<()> {
        if !self.updates.packaged {
            return Err(BridgeError::invalid(
                "源码运行不支持在线安装，请使用桌面安装版。",
            ));
        }
        // 同一轮下载和切换只接受一次调度，重复请求共享运行结果。
        self.mobile_update.send_if_modified(|pending| {
            let changed = !*pending;
            *pending = true;
            changed
        });
        Ok(())
    }
    pub(crate) async fn mobile_update_status(&self) -> Value {
        let state = self.update_state.read().await;
        if *self.mobile_update.borrow()
            && !matches!(state["phase"].as_str(), Some("checking" | "applying"))
        {
            return json!({"phase":"queued","message":"电脑正在准备升级。"});
        }
        state.clone()
    }
    pub fn mobile_update_requests(&self) -> watch::Receiver<bool> {
        self.mobile_update.subscribe()
    }
    pub fn finish_mobile_update(&self) {
        self.mobile_update.send_replace(false);
    }
    pub async fn prepare_update(&self) -> Result<bool> {
        {
            let mut state = self.update_state.write().await;
            if state["phase"] == "checking" || state["phase"] == "applying" {
                return Ok(false);
            }
            *state = json!({"phase":"checking","message":"正在检查版本并准备更新…"});
        }
        match self
            .updates
            .prepare(env!("CARGO_PKG_VERSION"), |progress| async move {
                *self.update_state.write().await = progress;
            })
            .await
        {
            Ok(Some(candidate)) => {
                let version = candidate.version.clone();
                *self.candidate.lock().await = Some(candidate);
                let mut state = self.update_state.write().await;
                state["phase"] = json!("ready");
                state["version"] = json!(version);
                state["message"] = json!("更新已下载并通过校验。");
                Ok(true)
            }
            Ok(None) => {
                *self.candidate.lock().await = None;
                let mut state = self.update_state.write().await;
                state["phase"] = json!("current");
                state["message"] = json!("当前已是最新正式版本。");
                Ok(false)
            }
            Err(error) => {
                *self.candidate.lock().await = None;
                let mut state = self.update_state.write().await;
                state["phase"] = json!("error");
                state["message"] = json!(error.message);
                Err(error)
            }
        }
    }
    pub async fn diagnostics(&self) -> Result<String> {
        let state = self.status().await?;
        Ok(format!("Codex Mobile {}\n连接服务：{}\n监听地址：{}:{}\n实时同步：{}\n同步详情：{}\n共享运行时：{}\n服务详情：{}",
            env!("CARGO_PKG_VERSION"),if state["service"]["running"]==true{"运行中"}else{"已停止"},state["config"]["bind"].as_str().unwrap_or(""),state["config"]["port"],state["sync"]["state"].as_str().unwrap_or(""),state["sync"]["message"].as_str().unwrap_or(""),state["runtime"]["message"].as_str().unwrap_or(""),state["service"]["error"].as_str().unwrap_or("无")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{native_test_support::NativePipe, test_support::Fixture};

    #[tokio::test]
    async fn recovered_connection_clears_startup_error_without_hiding_later_disconnect() {
        let fixture = Fixture::new();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        ServiceConfig {
            port,
            bind: "127.0.0.1".into(),
            ..Default::default()
        }
        .save(&fixture.paths.data_home)
        .unwrap();
        let host = Host::open(fixture.paths.clone(), false).unwrap();
        host.start().await.unwrap();
        *host.desktop_connection.lock().await =
            Some(json!({"state":"error","message":"旧连接启动失败"}));
        assert_eq!(host.status().await.unwrap()["sync"]["state"], "error");
        let (peer, connection) = NativePipe::start(&fixture.paths).await;
        host.app()
            .await
            .unwrap()
            .session
            .install_native(connection.clone());
        let calls = peer.calls.lock().await.len();
        let recovered = host.status().await.unwrap();
        assert_eq!(recovered["runtime"]["ready"], true);
        assert_eq!(recovered["sync"]["state"], "connected");
        assert!(host.desktop_connection.lock().await.is_none());
        assert_eq!(peer.calls.lock().await.len(), calls);
        connection.close();
        let disconnected = host.status().await.unwrap();
        assert_eq!(disconnected["runtime"]["ready"], false);
        assert_ne!(disconnected["sync"]["state"], "connected");
        assert_ne!(disconnected["sync"]["message"], "旧连接启动失败");
        host.stop().await.unwrap();
    }
}
