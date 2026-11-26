//! One official App Server JSON-RPC session shared across threads and callers.
use crate::{transport::Pipe, BridgeError, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

pub type AppServerSocket = WebSocketStream<Pipe>;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const QUEUED: u8 = 0;
const WRITING: u8 = 1;
const CANCELLED: u8 = 2;

/// Open a local WebSocket without sending any RPC initialization messages.
/// Unix sockets still use the normal HTTP WebSocket upgrade and framing.
pub async fn connect_websocket(endpoint: &str) -> Result<AppServerSocket> {
    connect_authenticated(endpoint, None).await
}
async fn connect_authenticated(endpoint: &str, token: Option<&str>) -> Result<AppServerSocket> {
    let url = url::Url::parse(endpoint)
        .map_err(|_| BridgeError::invalid("App Server 地址无效。").before_send())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(BridgeError::invalid("App Server 地址无效。").before_send());
    }
    tokio::time::timeout(CONNECT_TIMEOUT, async {
        let (stream, request): (Pipe, String) = match url.scheme() {
            "unix" => {
                if !endpoint.starts_with("unix://")
                    || url.host_str().is_some()
                    || url.query().is_some()
                {
                    return Err(BridgeError::invalid("App Server Unix 地址无效。").before_send());
                }
                #[cfg(unix)]
                {
                    let path = percent_encoding::percent_decode_str(url.path())
                        .decode_utf8()
                        .map_err(|_| {
                            BridgeError::invalid("App Server Unix 路径无效。").before_send()
                        })?;
                    let path = std::path::Path::new(path.as_ref());
                    if !path.is_absolute() {
                        return Err(BridgeError::invalid("App Server Unix 路径必须为绝对路径。")
                            .before_send());
                    }
                    let stream = tokio::net::UnixStream::connect(path)
                        .await
                        .map_err(|error| BridgeError::from(error).before_send())?;
                    (Box::new(stream), "ws://localhost/".into())
                }
                #[cfg(not(unix))]
                {
                    return Err(
                        BridgeError::unavailable("此平台不支持 App Server Unix 连接。")
                            .before_send(),
                    );
                }
            }
            "ws" => {
                let port = url.port_or_known_default().unwrap_or(80);
                let stream = match url.host() {
                    Some(url::Host::Ipv4(host)) if host.is_loopback() => {
                        tokio::net::TcpStream::connect(std::net::SocketAddr::new(host.into(), port))
                            .await
                    }
                    Some(url::Host::Ipv6(host)) if host.is_loopback() => {
                        tokio::net::TcpStream::connect(std::net::SocketAddr::new(host.into(), port))
                            .await
                    }
                    Some(url::Host::Domain("localhost")) => {
                        tokio::net::TcpStream::connect(("localhost", port)).await
                    }
                    _ => {
                        return Err(
                            BridgeError::invalid("App Server 必须使用本机连接。").before_send()
                        )
                    }
                }
                .map_err(|error| BridgeError::from(error).before_send())?;
                (Box::new(stream), url.to_string())
            }
            _ => return Err(BridgeError::invalid("App Server 连接协议无效。").before_send()),
        };
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;
        let mut request = request
            .into_client_request()
            .map_err(|_| BridgeError::invalid("共享连接地址无效。").before_send())?;
        if let Some(token) = token {
            request.headers_mut().insert(
                "authorization",
                format!("Bearer {token}")
                    .parse()
                    .map_err(|_| BridgeError::invalid("共享连接凭据无效。").before_send())?,
            );
        }
        tokio_tungstenite::client_async(request, stream)
            .await
            .map(|(socket, _)| socket)
            .map_err(|error| {
                BridgeError::unavailable(format!("App Server WebSocket 连接失败：{error}"))
                    .before_send()
            })
    })
    .await
    .map_err(|_| BridgeError::unavailable("连接 App Server 超时。").before_send())?
}

#[derive(Debug, Clone)]
pub enum AppServerEvent {
    Notification {
        connection_id: String,
        sequence: u64,
        message: Value,
    },
    ServerRequest {
        connection_id: String,
        sequence: u64,
        message: Value,
    },
    Disconnected {
        connection_id: String,
        sequence: u64,
        error: BridgeError,
    },
}

struct Delivery(AtomicU8);
impl Delivery {
    fn new() -> Arc<Self> {
        Arc::new(Self(AtomicU8::new(QUEUED)))
    }
    fn start(&self) -> bool {
        self.0
            .compare_exchange(QUEUED, WRITING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    fn cancel(&self) {
        let _ = self
            .0
            .compare_exchange(QUEUED, CANCELLED, Ordering::AcqRel, Ordering::Acquire);
    }
    fn error(&self, mut error: BridgeError) -> BridgeError {
        self.cancel();
        // A failed or interrupted write may already have delivered bytes.
        error.before_send = self.0.load(Ordering::Acquire) != WRITING;
        error
    }
}

struct Pending {
    reply: oneshot::Sender<Result<(Value, u64)>>,
    delivery: Arc<Delivery>,
}
struct ServerRequest {
    message: Value,
    responding: bool,
}
struct Shared {
    runtime: Mutex<Option<(std::path::PathBuf, crate::runtime::RuntimeInfo)>>,
    native: Option<crate::native::NativeOwner>,
    pending: Mutex<HashMap<String, Pending>>,
    requests: Mutex<HashMap<String, ServerRequest>>,
    alive: Arc<AtomicBool>,
    cancel: CancellationToken,
    id: String,
    sequence: AtomicU64,
    events: broadcast::Sender<AppServerEvent>,
}
struct Outgoing {
    message: Message,
    delivery: Arc<Delivery>,
    written: Option<oneshot::Sender<Result<()>>>,
}
struct Registration {
    shared: Arc<Shared>,
    key: String,
    delivery: Arc<Delivery>,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.delivery.cancel();
        self.shared.pending.lock().unwrap().remove(&self.key);
    }
}

/// Cache one Arc in the Bridge lifecycle; RPC calls never create or retry a connection.
pub struct AppServerConnection {
    pub id: String,
    pub url: String,
    pub initialized: Value,
    pub events: broadcast::Sender<AppServerEvent>,
    pub alive: Arc<AtomicBool>,
    sender: mpsc::Sender<Outgoing>,
    shared: Arc<Shared>,
}

impl AppServerConnection {
    pub async fn connect(endpoint: &str) -> Result<Arc<Self>> {
        Self::connect_inner(endpoint, None, false).await
    }
    pub async fn connect_broker(endpoint: &str, token: &str) -> Result<Arc<Self>> {
        Self::connect_inner(endpoint, Some(token), true).await
    }
    async fn connect_inner(endpoint: &str, token: Option<&str>, broker: bool) -> Result<Arc<Self>> {
        let socket = connect_authenticated(endpoint, token).await?;
        let id = uuid::Uuid::new_v4().to_string();
        let (events, _) = broadcast::channel(256);
        let alive = Arc::new(AtomicBool::new(true));
        let shared = Arc::new(Shared {
            runtime: Mutex::new(None),
            native: None,
            pending: Mutex::new(HashMap::new()),
            requests: Mutex::new(HashMap::new()),
            alive: alive.clone(),
            cancel: CancellationToken::new(),
            id: id.clone(),
            sequence: AtomicU64::new(0),
            events: events.clone(),
        });
        let (sender, outgoing) = mpsc::channel(64);
        tokio::spawn(run_socket(socket, outgoing, shared.clone()));
        let mut connection = Self {
            id,
            url: endpoint.into(),
            initialized: Value::Null,
            events,
            alive,
            sender,
            shared,
        };
        connection.initialized = connection
            .rpc(if broker { "codexMobile/attach" } else { "initialize" }, json!({
                "clientInfo": {"name": "codex-mobile", "title": "Codex Mobile", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true, "mcpServerOpenaiFormElicitation": true}
            }))
            .await
            .map_err(BridgeError::before_send)?;
        if !broker {
            connection
                .notify("initialized", json!({}))
                .await
                .map_err(BridgeError::before_send)?;
        }
        Ok(Arc::new(connection))
    }

    pub(crate) async fn connect_native(paths: &crate::paths::Paths) -> Result<Arc<Self>> {
        let (pipe, owner) = crate::native::discover(paths).await?;
        Self::connect_native_pipe(pipe, owner).await
    }
    pub(crate) async fn connect_native_pipe(
        pipe: Pipe,
        owner: crate::native::NativeOwner,
    ) -> Result<Arc<Self>> {
        owner.validate().map_err(BridgeError::before_send)?;
        let id = uuid::Uuid::new_v4().to_string();
        let (events, _) = broadcast::channel(256);
        let alive = Arc::new(AtomicBool::new(true));
        let url = owner.pipe.to_string_lossy().into_owned();
        let shared = Arc::new(Shared {
            runtime: Mutex::new(None),
            native: Some(owner),
            pending: Mutex::new(HashMap::new()),
            requests: Mutex::new(HashMap::new()),
            alive: alive.clone(),
            cancel: CancellationToken::new(),
            id: id.clone(),
            sequence: AtomicU64::new(0),
            events: events.clone(),
        });
        let (sender, outgoing) = mpsc::channel(64);
        tokio::spawn(run_native(pipe, outgoing, shared.clone()));
        let mut connection = Self {
            id,
            url,
            initialized: Value::Null,
            events,
            alive,
            sender,
            shared,
        };
        connection.initialized = connection
            .rpc("tools/list", json!({"threadStartKind":"all"}))
            .await
            .map_err(BridgeError::before_send)?;
        if !crate::native::supported_catalog(&connection.initialized) {
            return Err(
                BridgeError::unavailable("当前桌面工具契约不受支持，保留历史阅读。").before_send(),
            );
        }
        Ok(Arc::new(connection))
    }
    pub fn is_native(&self) -> bool {
        self.shared.native.is_some()
    }
    pub fn owner_valid(&self) -> bool {
        self.shared
            .native
            .as_ref()
            .is_some_and(|owner| owner.validate().is_ok())
    }

    pub fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Acquire)
    }
    pub fn bind_runtime(
        &self,
        directory: std::path::PathBuf,
        runtime: crate::runtime::RuntimeInfo,
    ) {
        *self.shared.runtime.lock().unwrap() = Some((directory, runtime));
    }
    pub fn runtime_id(&self) -> Option<String> {
        if let Some(owner) = &self.shared.native {
            return Some(owner.instance_id());
        }
        self.shared
            .runtime
            .lock()
            .unwrap()
            .as_ref()
            .map(|(_, runtime)| runtime.instance_id.clone())
    }
    pub fn subscribe(&self) -> broadcast::Receiver<AppServerEvent> {
        self.events.subscribe()
    }
    pub fn pending_requests(&self, thread: &str) -> Vec<Value> {
        self.shared
            .requests
            .lock()
            .unwrap()
            .values()
            .filter(|request| request.message["params"]["threadId"] == thread)
            .map(|request| request.message.clone())
            .collect()
    }
    pub async fn rpc(&self, method: &str, params: Value) -> Result<Value> {
        self.request(method, params, REQUEST_TIMEOUT).await
    }
    /// The sequence orders received messages on this connection only. It does
    /// not assert that a history snapshot contains every earlier text delta.
    pub async fn rpc_with_sequence(&self, method: &str, params: Value) -> Result<(Value, u64)> {
        self.request_with_sequence(method, params, REQUEST_TIMEOUT)
            .await
    }
    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.request_with_sequence(method, params, timeout)
            .await
            .map(|(value, _)| value)
    }
    pub async fn request_with_sequence(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<(Value, u64)> {
        if method.is_empty() {
            return Err(BridgeError::invalid("App Server 方法不能为空。").before_send());
        }
        if self.is_native() && !["tools/list", "tools/call"].contains(&method) {
            return Err(BridgeError::invalid("原生桌面通道不支持 App Server 操作。").before_send());
        }
        let id = json!(uuid::Uuid::new_v4().to_string());
        let key = id.to_string();
        let delivery = Delivery::new();
        let (reply, response) = oneshot::channel();
        {
            let mut pending = self.shared.pending.lock().unwrap();
            if !self.is_alive() {
                return Err(BridgeError::unavailable("App Server 连接已断开。").before_send());
            }
            pending.insert(
                key.clone(),
                Pending {
                    reply,
                    delivery: delivery.clone(),
                },
            );
        }
        let _registration = Registration {
            shared: self.shared.clone(),
            key,
            delivery: delivery.clone(),
        };
        let message = Message::Text(
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
                .to_string()
                .into(),
        );
        match tokio::time::timeout(timeout, async {
            self.sender
                .send(Outgoing {
                    message,
                    delivery: delivery.clone(),
                    written: None,
                })
                .await
                .map_err(|_| delivery.error(BridgeError::unavailable("App Server 连接已断开。")))?;
            response.await.map_err(|_| {
                delivery.error(BridgeError::unavailable(
                    "App Server 连接已断开，请确认操作结果。",
                ))
            })?
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(delivery.error(BridgeError::unavailable(
                "App Server 响应超时，请先确认操作结果，避免重复发送。",
            ))),
        }
    }
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        if method.is_empty() {
            return Err(BridgeError::invalid("App Server 方法不能为空。").before_send());
        }
        self.send(json!({"jsonrpc": "2.0", "method": method, "params": params}))
            .await
    }
    /// Reply using the original request id on its original connection.
    pub async fn respond(&self, id: Value, result: Value) -> Result<()> {
        let key = rpc_id(&id)
            .ok_or_else(|| BridgeError::invalid("App Server 请求编号无效。").before_send())?;
        {
            let mut requests = self.shared.requests.lock().unwrap();
            let request = requests
                .get_mut(&key)
                .filter(|request| !request.responding)
                .ok_or_else(|| {
                    BridgeError::new(409, "该请求已回复或已结束，请等待状态同步。").before_send()
                })?;
            request.responding = true;
        }
        let sent = self
            .send(json!({"jsonrpc": "2.0", "id": id, "result": result}))
            .await;
        if sent.as_ref().is_err_and(|error| error.before_send) {
            if let Some(request) = self.shared.requests.lock().unwrap().get_mut(&key) {
                request.responding = false;
            }
        }
        sent
    }
    async fn send(&self, value: Value) -> Result<()> {
        if !self.is_alive() {
            return Err(BridgeError::unavailable("App Server 连接已断开。").before_send());
        }
        let delivery = Delivery::new();
        let (written, response) = oneshot::channel();
        let result = tokio::time::timeout(REQUEST_TIMEOUT, async {
            self.sender
                .send(Outgoing {
                    message: Message::Text(value.to_string().into()),
                    delivery: delivery.clone(),
                    written: Some(written),
                })
                .await
                .map_err(|_| delivery.error(BridgeError::unavailable("App Server 连接已断开。")))?;
            response.await.map_err(|_| {
                delivery.error(BridgeError::unavailable(
                    "App Server 连接已断开，请确认操作结果。",
                ))
            })?
        })
        .await;
        match result {
            Ok(result) => result,
            Err(_) => Err(delivery.error(BridgeError::unavailable(
                "发送 App Server 消息超时，请确认操作结果。",
            ))),
        }
    }
    pub fn close(&self) {
        self.alive.store(false, Ordering::Release);
        self.shared.cancel.cancel();
    }
}
impl Drop for AppServerConnection {
    fn drop(&mut self) {
        self.close();
    }
}

fn rpc_id(value: &Value) -> Option<String> {
    (value.is_string() || value.as_i64().is_some() || value.as_u64().is_some())
        .then(|| value.to_string())
}

fn receive(shared: &Shared, value: Value) -> Result<()> {
    let sequence = shared.sequence.fetch_add(1, Ordering::AcqRel) + 1;
    if value.get("method").is_some() {
        if let Some(key) = value.get("id").and_then(rpc_id) {
            shared
                .requests
                .lock()
                .unwrap()
                .entry(key)
                .or_insert_with(|| ServerRequest {
                    message: value.clone(),
                    responding: false,
                });
        } else if value["method"] == "serverRequest/resolved" {
            shared
                .requests
                .lock()
                .unwrap()
                .remove(&value["params"]["requestId"].to_string());
        } else if value["method"] == "turn/completed" {
            shared.requests.lock().unwrap().retain(|_, request| {
                request.message["params"]["threadId"] != value["params"]["threadId"]
                    || request.message["params"]["turnId"] != value["params"]["turn"]["id"]
            });
        }
        let event = if value.get("id").is_some() {
            AppServerEvent::ServerRequest {
                connection_id: shared.id.clone(),
                sequence,
                message: value,
            }
        } else {
            AppServerEvent::Notification {
                connection_id: shared.id.clone(),
                sequence,
                message: value,
            }
        };
        let _ = shared.events.send(event);
    } else if let Some(key) = value.get("id").and_then(rpc_id) {
        if let Some(pending) = shared.pending.lock().unwrap().remove(&key) {
            let result = if let Some(error) = value.get("error").filter(|error| !error.is_null()) {
                let mut rejected = BridgeError::unavailable(
                    error["message"]
                        .as_str()
                        .unwrap_or("App Server 请求未被接受。"),
                );
                rejected.rpc_error = Some(Box::new(error.clone()));
                Err(rejected)
            } else {
                Ok((value["result"].clone(), sequence))
            };
            let _ = pending.reply.send(result);
        }
    } else {
        return Err(BridgeError::unavailable(
            "App Server 返回了无效的 RPC 消息。",
        ));
    }
    Ok(())
}

async fn run_native(pipe: Pipe, mut outgoing: mpsc::Receiver<Outgoing>, shared: Arc<Shared>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut reader, mut writer) = tokio::io::split(pipe);
    let incoming = async {
        loop {
            let size = reader.read_u32_le().await? as usize;
            if size == 0 || size > crate::native::MAX_FRAME {
                return Err::<(), BridgeError>(BridgeError::unavailable(
                    "官方桌面管道帧大小无效。",
                ));
            }
            let mut bytes = vec![0; size];
            reader.read_exact(&mut bytes).await?;
            shared.native.as_ref().unwrap().validate()?;
            receive(&shared, serde_json::from_slice(&bytes)?)?;
        }
    };
    tokio::pin!(incoming);
    let mut heartbeat = tokio::time::interval(Duration::from_millis(500));
    let error = loop {
        tokio::select! {
            result = &mut incoming => break result.err().unwrap_or_else(|| BridgeError::unavailable("官方桌面管道已关闭。")),
            _ = shared.cancel.cancelled() => break BridgeError::unavailable("手机原生连接已关闭。"),
            _ = heartbeat.tick() => {
                if let Err(error) = shared.native.as_ref().unwrap().validate() { break error; }
            }
            job = outgoing.recv() => {
                let Some(job) = job else { break BridgeError::unavailable("手机原生连接已关闭。"); };
                if let Err(error) = shared.native.as_ref().unwrap().validate() { break error; }
                let Message::Text(text) = job.message else { break BridgeError::invalid("原生管道只接受 JSON RPC。"); };
                if text.len() > crate::native::MAX_FRAME { break BridgeError::invalid("原生请求超过帧大小限制。"); }
                if !job.delivery.start() { continue; }
                let result = tokio::select! {
                    _ = shared.cancel.cancelled() => Err(BridgeError::unavailable("原生写入中断，请核对操作结果。")),
                    result = async {
                        writer.write_u32_le(text.len() as u32).await?;
                        writer.write_all(text.as_bytes()).await?;
                        writer.flush().await
                    } => result.map_err(BridgeError::from),
                };
                if let Some(written) = job.written { let _ = written.send(result.clone()); }
                if let Err(error) = result { break error; }
            }
        }
    };
    disconnected(&shared, error);
    drop(outgoing);
}

fn disconnected(shared: &Shared, error: BridgeError) {
    shared.alive.store(false, Ordering::Release);
    for (_, pending) in shared.pending.lock().unwrap().drain() {
        let _ = pending
            .reply
            .send(Err(pending.delivery.error(error.clone())));
    }
    let _ = shared.events.send(AppServerEvent::Disconnected {
        connection_id: shared.id.clone(),
        sequence: shared.sequence.fetch_add(1, Ordering::AcqRel) + 1,
        error,
    });
}

async fn run_socket(
    socket: AppServerSocket,
    mut outgoing: mpsc::Receiver<Outgoing>,
    shared: Arc<Shared>,
) {
    let (mut writer, mut reader) = socket.split();
    let mut heartbeat = tokio::time::interval(Duration::from_millis(500));
    let error = loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                let error = shared.runtime.lock().unwrap().as_ref().and_then(|(directory, runtime)| runtime.validate(directory).err());
                if let Some(error) = error { break error; }
            }
            _ = shared.cancel.cancelled() => break BridgeError::unavailable("App Server 连接已关闭。"),
            next = outgoing.recv() => {
                let Some(job) = next else { break BridgeError::unavailable("App Server 连接已关闭。"); };
                if !shared.alive.load(Ordering::Acquire) {
                    break BridgeError::unavailable("App Server 连接已关闭。");
                }
                let error = shared.runtime.lock().unwrap().as_ref().and_then(|(directory, runtime)| runtime.validate(directory).err());
                if let Some(error) = error { break error; }
                if !job.delivery.start() { continue; }
                let result = tokio::select! {
                    _ = shared.cancel.cancelled() => Err(BridgeError::unavailable("App Server 写入已中断，请确认操作结果。")),
                    result = writer.send(job.message) => result.map_err(|error| BridgeError::unavailable(format!("App Server 写入失败：{error}"))),
                };
                if let Some(written) = job.written { let _ = written.send(result.clone()); }
                if let Err(error) = result { break error; }
            }
            incoming = reader.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let value = match serde_json::from_str::<Value>(&text) {
                            Ok(value) => value,
                            Err(error) => break BridgeError::from(error),
                        };
                        if let Err(error) = receive(&shared, value) { break error; }
                    }
                    Some(Ok(Message::Ping(_))) => {
                        // Tungstenite queues the pong while reading; flush it
                        // rather than adding a second manual pong.
                        let result = tokio::select! {
                            _ = shared.cancel.cancelled() => Err(BridgeError::unavailable("App Server 连接已关闭。")),
                            result = writer.flush() => result.map_err(|error| BridgeError::unavailable(format!("App Server 心跳响应失败：{error}"))),
                        };
                        if let Err(error) = result { break error; }
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => break BridgeError::unavailable("App Server 连接已断开，请确认操作结果。"),
                    Some(Ok(_)) => break BridgeError::unavailable("App Server WebSocket 消息格式无效。"),
                    Some(Err(error)) => break BridgeError::unavailable(format!("App Server 连接已断开：{error}")),
                }
            }
        }
    };
    shared.alive.store(false, Ordering::Release);
    for (_, pending) in shared.pending.lock().unwrap().drain() {
        let _ = pending
            .reply
            .send(Err(pending.delivery.error(error.clone())));
    }
    drop(outgoing);
    let _ = shared.events.send(AppServerEvent::Disconnected {
        connection_id: shared.id.clone(),
        sequence: shared.sequence.fetch_add(1, Ordering::AcqRel) + 1,
        error,
    });
    let _ = tokio::time::timeout(Duration::from_secs(1), writer.close()).await;
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn framed_native_rpc_preserves_ids_and_unknown_delivery_without_replay() {
        let (client, mut server) = tokio::io::duplex(4096);
        let process = crate::process_identity::ProcessIdentity::current().unwrap();
        let owner = crate::native::NativeOwner {
            pipe: "native-test".into(),
            desktop: process.clone(),
            server: process,
        };
        let peer = tokio::spawn(async move {
            let size = server.read_u32_le().await.unwrap();
            let mut bytes = vec![0; size as usize];
            server.read_exact(&mut bytes).await.unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["method"], "tools/list");
            let tools=[("read_thread",vec!["threadId"]),("send_message_to_thread",vec!["threadId","prompt"]),("create_thread",vec!["prompt","target"]),("list_projects",vec![]),("list_threads",vec![])].map(|(name,required)| {
                let properties=required.iter().map(|key| ((*key).to_owned(),json!({"type":"string"}))).collect::<serde_json::Map<_,_>>();
                json!({"name":name,"namespace":"codex_app","inputSchema":{"type":"object","additionalProperties":false,"required":required,"properties":properties}})
            });
            let reply =
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"tools":tools}}).to_string();
            server.write_u32_le(reply.len() as u32).await.unwrap();
            server.write_all(reply.as_bytes()).await.unwrap();
            let size = server.read_u32_le().await.unwrap();
            let mut bytes = vec![0; size as usize];
            server.read_exact(&mut bytes).await.unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["method"], "tools/call");
            assert_eq!(request["params"]["callId"], "user-intent");
            // 投递后断开；不发送回执，客户端必须保留未知而不是自动重试。
        });
        let connection = AppServerConnection::connect_native_pipe(Box::new(client), owner)
            .await
            .unwrap();
        assert!(
            connection
                .rpc("turn/start", json!({}))
                .await
                .unwrap_err()
                .before_send
        );
        let error = connection
            .rpc("tools/call", json!({"callId":"user-intent"}))
            .await
            .unwrap_err();
        assert!(!error.before_send);
        peer.await.unwrap();
        assert!(!connection.is_alive());
    }
}
