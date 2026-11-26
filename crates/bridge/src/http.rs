use crate::{
    auth::{check_origin, header, Auth, RelayPhone},
    config::{valid_model, ServiceConfig, EFFORTS},
    desktop::{Desktop, LiveView, QueueChange},
    goals::Goals,
    history::History,
    host::Host,
    images::{Images, MAX_IMAGE_BYTES},
    notifications::Notifications,
    operations::Operations,
    paths::Paths,
    relay::RemoteAccess,
    session::Session,
    state::{summarize_snapshot, text_append},
    submissions::Submissions,
    updates::Updates,
    usage::{Usage, UsageQuery},
    BridgeError, Result,
};
use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{HeaderMap, Method, Request, Response, StatusCode},
    response::{
        sse::{Event, Sse},
        IntoResponse,
    },
    Json, Router,
};
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    future::Future,
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tower_http::compression::CompressionLayer;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TextAppends {
    Disabled,
    WithTurnState,
    Compact,
}

pub struct Bridge {
    pub paths: Paths,
    pub config: ServiceConfig,
    pub desktop: Arc<Desktop>,
    pub history: Arc<History>,
    pub goals: Arc<Goals>,
    pub session: Arc<Session>,
    pub notifications: Notifications,
    pub updates: Updates,
    pub auth: Mutex<Auth>,
    relay: Arc<RemoteAccess>,
    host: Weak<Host>,
    images: Images,
    usage: Mutex<Option<Arc<Usage>>>,
    submissions: Arc<Submissions>,
    closing: AtomicBool,
    streams: CancellationToken,
    rotations: broadcast::Sender<()>,
    work: TaskTracker,
    abort_work: CancellationToken,
}
pub struct Server {
    pub address: SocketAddr,
    pub app: Arc<Bridge>,
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<Result<()>>,
}

// Axum 的优雅退出会等待响应体；超时后也必须唤醒并关闭正在等待读写的连接。
struct ServiceListener {
    listener: tokio::net::TcpListener,
    cancel: CancellationToken,
}
struct ServiceSocket {
    socket: tokio::net::TcpStream,
    cancelled: Pin<Box<tokio_util::sync::WaitForCancellationFutureOwned>>,
}
impl axum::serve::Listener for ServiceListener {
    type Io = ServiceSocket;
    type Addr = SocketAddr;
    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (socket, address) = axum::serve::Listener::accept(&mut self.listener).await;
        (
            ServiceSocket {
                socket,
                cancelled: Box::pin(self.cancel.clone().cancelled_owned()),
            },
            address,
        )
    }
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }
}
impl ServiceSocket {
    fn check_open(&mut self, context: &mut Context<'_>) -> std::io::Result<()> {
        if self.cancelled.as_mut().poll(context).is_ready() {
            Err(std::io::ErrorKind::ConnectionAborted.into())
        } else {
            Ok(())
        }
    }
}
impl AsyncRead for ServiceSocket {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.check_open(context)?;
        Pin::new(&mut self.socket).poll_read(context, buffer)
    }
}
impl AsyncWrite for ServiceSocket {
    fn poll_write(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        self.check_open(context)?;
        Pin::new(&mut self.socket).poll_write(context, bytes)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        self.check_open(context)?;
        Pin::new(&mut self.socket).poll_flush(context)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.socket).poll_shutdown(context)
    }
}

impl Bridge {
    pub fn open(paths: Paths, config: ServiceConfig, packaged: bool) -> Result<Arc<Self>> {
        let relay = Arc::new(RemoteAccess::open(paths.data_home.clone())?);
        Self::open_with_host(paths, config, packaged, Weak::new(), relay)
    }
    pub(crate) fn open_with_host(
        paths: Paths,
        config: ServiceConfig,
        packaged: bool,
        host: Weak<Host>,
        relay: Arc<RemoteAccess>,
    ) -> Result<Arc<Self>> {
        let history = Arc::new(History::new(paths.codex_home.clone()));
        let session = Session::new(paths.clone());
        let desktop = Desktop::with_session(paths.clone(), history.clone(), session.clone());
        let goals = Goals::with_session(session.clone());
        let auth = Auth::open(paths.data_home.clone())?;
        let notifications = Notifications::open(paths.data_home.clone())?;
        let updates = Updates::new(paths.data_home.clone(), packaged)?;
        let images = Images {
            directory: paths.data_home.join("images"),
        };
        let (rotations, _) = broadcast::channel(8);
        let operations = Operations::open(&paths.data_home)?;
        let work = TaskTracker::new();
        let abort_work = CancellationToken::new();
        let submissions = Submissions::new(
            operations,
            session.clone(),
            desktop.clone(),
            images.clone(),
            work.clone(),
            abort_work.clone(),
        );
        Ok(Arc::new(Self {
            paths,
            config,
            desktop,
            history,
            goals,
            session,
            notifications,
            updates,
            auth: Mutex::new(auth),
            relay,
            host,
            images,
            usage: Mutex::new(None),
            submissions,
            closing: AtomicBool::new(false),
            streams: CancellationToken::new(),
            rotations,
            work,
            abort_work,
        }))
    }
    pub async fn start(self: &Arc<Self>) -> Result<Server> {
        let address = SocketAddr::new(
            self.config
                .bind
                .parse()
                .map_err(|_| BridgeError::invalid("服务监听地址无效。"))?,
            self.config.port,
        );
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::AddrInUse {
                    BridgeError::unavailable(format!(
                        "{} 端口已被占用，请先关闭旧版连接服务，再点击启动。",
                        self.config.port
                    ))
                } else {
                    error.into()
                }
            })?;
        let address = listener.local_addr()?;
        let listener = ServiceListener {
            listener,
            cancel: self.abort_work.clone(),
        };
        let router = Router::new()
            .fallback(handle)
            .with_state(self.clone())
            .layer(CompressionLayer::new().gzip(true));
        let shutdown = CancellationToken::new();
        let token = shutdown.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(token.cancelled_owned())
                .await?;
            Ok(())
        });
        let this = self.clone();
        self.spawn_work(async move {
            this.poll_notifications().await;
        });
        self.spawn_work(self.relay.clone().run(self.clone(), self.streams.clone()));
        let this = self.clone();
        self.spawn_work(async move {
            loop {
                if this.streams.is_cancelled() { break; }
                let _ = this.desktop.probe().await;
                tokio::select! { _ = this.streams.cancelled() => break, _ = tokio::time::sleep(Duration::from_secs(5)) => {} }
            }
        });
        Ok(Server {
            address,
            app: self.clone(),
            shutdown,
            task,
        })
    }
    fn spawn_work<F>(self: &Arc<Self>, future: F) -> tokio::task::JoinHandle<Option<F::Output>>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let cancel = self.abort_work.clone();
        self.work.spawn(async move {
            tokio::select! { _ = cancel.cancelled() => None, result = future => Some(result) }
        })
    }
    pub async fn pairing(&self) -> Value {
        json!({"accessKey":self.auth.lock().await.key(),"urls":crate::auth::network_urls(self.config.port),"publicUrl":self.config.public_url})
    }
    pub async fn rotate_local(&self) -> Result<String> {
        let key = self.auth.lock().await.rotate_local()?;
        self.notifications.clear_lan().await?;
        let _ = self.rotations.send(());
        Ok(key)
    }
    async fn usage(&self) -> Result<Arc<Usage>> {
        let mut service = self.usage.lock().await;
        if service.is_none() {
            *service = Some(Usage::open(
                self.paths.codex_home.clone(),
                self.paths.data_home.clone(),
            )?);
        }
        Ok(service.as_ref().unwrap().clone())
    }
    async fn poll_notifications(self: Arc<Self>) {
        let mut since = None;
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            tokio::select! {_=self.streams.cancelled()=>break,_=interval.tick()=>{
                if !self.notifications.has_subscriptions().await{since=None;continue;}
                let baseline=since.is_none();
                let time=*since.get_or_insert_with(||chrono::Utc::now().timestamp());
                match self.history.notification_turns(time){Ok(turns)=>{if baseline {self.notifications.baseline(&turns).await;} else {self.notifications.observe_history(&turns).await;}},Err(error)=>tracing::warn!("无法读取通知状态：{}",error)}
            }}
        }
    }
    async fn route(
        self: &Arc<Self>,
        request: Request<Body>,
        phone: Option<RelayPhone>,
    ) -> Result<Response<Body>> {
        let method = request.method().clone();
        let headers = request.headers().clone();
        let uri = request.uri().clone();
        let raw_path = uri.path();
        let query: HashMap<String, String> =
            url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
                .into_owned()
                .collect();
        if !raw_path.starts_with("/api/") {
            return self.static_file(&method, &headers, raw_path).await;
        }
        check_origin(&headers).map_err(BridgeError::before_send)?;
        let parts = raw_path
            .trim_start_matches('/')
            .split('/')
            .map(|part| {
                percent_encoding::percent_decode_str(part)
                    .decode_utf8()
                    .map(|s| s.into_owned())
                    .map_err(|_| BridgeError::invalid("请求地址无效。"))
            })
            .collect::<Result<Vec<_>>>()?;
        let route = parts.get(1).map(String::as_str).unwrap_or("");
        if route == "health" && method == Method::GET {
            return Ok(json_response(
                200,
                json!({"service":"codex-mobile","backend":"rust","version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()}),
            ));
        }
        if route == "pairing" {
            return Err(BridgeError::new(
                410,
                "连接信息与密钥管理已移到 Codex Mobile 桌面程序。",
            ));
        }
        if route == "login" && method == Method::POST {
            if phone.is_some() {
                return Err(not_allowed());
            }
            let input = body(request).await?;
            let token = self
                .auth
                .lock()
                .await
                .login(input["accessKey"].as_str().unwrap_or(""))?;
            let cookie = format!(
                "cm_session={token}; HttpOnly; SameSite=Strict; Path=/; Max-Age=604800{}",
                if header(&headers, "origin").starts_with("https://") {
                    "; Secure"
                } else {
                    ""
                }
            );
            let mut response = json_response(200, json!({"ok":true}));
            response
                .headers_mut()
                .insert("set-cookie", cookie.parse().unwrap());
            return Ok(response);
        }
        let (owner, expires) = match &phone {
            Some(phone) => (phone.owner().to_owned(), i64::MAX),
            None => self
                .auth
                .lock()
                .await
                .session(&headers)
                .map_err(BridgeError::before_send)?,
        };
        let operation_owner = match &phone {
            Some(phone) => phone.owner().to_owned(),
            None => format!("local:{}", crate::digest(self.auth.lock().await.key())),
        };
        if route == "operations" {
            if method == Method::GET {
                let mut result =
                    json!({"operations":self.submissions.operations.pending(&operation_owner)?});
                if let Some(ids) = query.get("ids") {
                    let ids: Vec<String> = ids.split(',').map(str::to_owned).collect();
                    if ids.len() > 100
                        || ids
                            .iter()
                            .any(|id| !crate::submissions::valid_request_id(id))
                    {
                        return Err(BridgeError::invalid("操作编号无效，单次最多查询 100 个。"));
                    }
                    result["results"] =
                        json!(self.submissions.operations.lookup(&operation_owner, &ids)?);
                }
                return Ok(json_response(200, result));
            }
            if method == Method::POST && parts.len() == 3 {
                if body(request).await?["confirm"] != "close-unknown" {
                    return Err(BridgeError::invalid(
                        "请明确确认结束等待；此操作不会重新发送。",
                    ));
                }
                self.submissions
                    .operations
                    .close_unknown(&operation_owner, &parts[2])?;
                return Ok(json_response(200, json!({"closed":true})));
            }
            return Err(not_allowed());
        }
        if route == "session" && method == Method::GET {
            if query.get("ping").is_some_and(|value| value == "1") {
                return Ok(
                    (StatusCode::NO_CONTENT, [("cache-control", "no-store")]).into_response()
                );
            }
            return Ok(json_response(
                200,
                json!({"defaultThreadId":self.history.default_thread_id()?,"capabilities":self.session.capabilities()}),
            ));
        }
        if method != Method::GET
            && method != Method::HEAD
            && route == "threads"
            && header(&headers, "x-codex-mobile-contract") != "2"
        {
            return Err(
                BridgeError::new(426, "连接协议已更新，请保留草稿后刷新页面。").before_send(),
            );
        }
        if route == "logout" && method == Method::POST {
            if phone.is_some() {
                return Err(not_allowed());
            }
            self.notifications.remove_for_owner(&owner, None).await?;
            self.auth.lock().await.logout(&owner)?;
            let mut response = json_response(200, json!({"ok":true}));
            response.headers_mut().insert(
                "set-cookie",
                "cm_session=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0"
                    .parse()
                    .unwrap(),
            );
            return Ok(response);
        }
        if route == "goal-service" {
            if method != Method::GET {
                return Err(not_allowed());
            }
            self.goals.connect().await?;
            return Ok(json_response(200, json!({"ready":true})));
        }
        if route == "update-settings" {
            if method != Method::GET {
                return Err(not_allowed());
            }
            let mut settings = self.updates.status();
            settings["appServer"] = self.session.status();
            if let Some(host) = self.host.upgrade() {
                settings["update"] = host.mobile_update_status().await;
            }
            return Ok(json_response(200, settings));
        }
        if route == "update" && parts.len() == 2 {
            if method != Method::POST {
                return Err(not_allowed());
            }
            self.host
                .upgrade()
                .ok_or_else(|| {
                    BridgeError::invalid("当前运行方式不支持在线安装，请使用桌面安装版。")
                })?
                .request_mobile_update()?;
            return Ok(json_response(202, json!({"ok":true})));
        }
        if route == "usage" || (route == "threads" && parts.get(3).is_some_and(|s| s == "usage")) {
            if method != Method::GET {
                return Err(not_allowed());
            }
            let options = UsageQuery::parse(
                query.get("period").map(String::as_str).unwrap_or("all"),
                query.get("timeZone").map(String::as_str).unwrap_or("UTC"),
                chrono::Utc::now(),
            )?;
            let service = self.usage().await?;
            let result = if route == "usage" && parts.get(2).is_some_and(|s| s == "threads") {
                let limit = query
                    .get("limit")
                    .map(|s| s.parse::<usize>())
                    .transpose()
                    .map_err(|_| BridgeError::invalid("每页条数无效。"))?
                    .unwrap_or(30);
                service.threads(
                    &options,
                    limit,
                    query.get("search").map(String::as_str).unwrap_or(""),
                    query.get("cursor").map(String::as_str),
                )?
            } else {
                service.overview(
                    &options,
                    if route == "threads" {
                        parts.get(2).map(String::as_str)
                    } else {
                        None
                    },
                )?
            };
            return Ok(json_response(200, result));
        }
        if route == "images" && parts.len() == 2 && method == Method::POST {
            let bytes = to_bytes(request.into_body(), MAX_IMAGE_BYTES)
                .await
                .map_err(|_| BridgeError::new(413, "每张图片不能超过 20 MB。"))?;
            return Ok(json_response(
                201,
                self.images.save(&bytes, header(&headers, "x-file-name"))?,
            ));
        }
        if route == "images" && parts.len() == 3 && method == Method::GET {
            let image = self.images.read(&parts[2])?;
            return file_response(
                Path::new(image["path"].as_str().unwrap()),
                image["type"].as_str().unwrap_or("application/octet-stream"),
                false,
                "private, no-store",
                None,
            )
            .await;
        }
        if route == "notifications" {
            let operation = parts.get(2).map(String::as_str).unwrap_or("");
            if operation == "key" && method == Method::GET {
                return Ok(json_response(
                    200,
                    json!({"publicKey":self.notifications.public_key()}),
                ));
            }
            if method != Method::POST {
                return Err(not_allowed());
            }
            let input = body(request).await?;
            let result = match operation {
                "status" => {
                    self.notifications
                        .status(&input["subscription"], &owner, expires)
                        .await?
                }
                "subscribe" => {
                    self.notifications
                        .add(
                            &input["subscription"],
                            &owner,
                            expires,
                            if headers.contains_key("origin") {
                                header(&headers, "origin")
                            } else {
                                &self.config.public_url
                            },
                        )
                        .await?
                }
                "unsubscribe" | "test" => {
                    let endpoint = input["endpoint"]
                        .as_str()
                        .ok_or_else(|| BridgeError::invalid("没有找到浏览器订阅。"))?;
                    if operation == "unsubscribe" {
                        self.notifications
                            .remove_for_owner(&owner, Some(endpoint))
                            .await?
                    } else {
                        self.notifications.test(endpoint, &owner).await?;
                        json!({"ok":true})
                    }
                }
                _ => return Err(BridgeError::new(404, "找不到这个接口。")),
            };
            return Ok(json_response(200, result));
        }
        if route == "projects" && method == Method::GET {
            return Ok(json_response(
                200,
                if let Some(source) = query.get("threadId").filter(|id| !id.is_empty()) {
                    json!({"projects":self.desktop.scoped_projects(source).await?})
                } else if self.session.capabilities()["catalog"] == true {
                    json!({"projects":self.desktop.projects().await?})
                } else {
                    json!({"projects":self.history.projects()?,"historyOnly":true,"capabilities":self.session.capabilities()})
                },
            ));
        }
        if route == "model-options" && method == Method::GET {
            self.session.require("modelSelection")?;
            return Ok(json_response(
                200,
                json!({
                    "recentModels":self.history.recent_models()?,
                    "efforts":self.session.model_efforts().unwrap_or_default(),
                }),
            ));
        }
        if ["new-thread-settings", "context"].contains(&route) && method == Method::GET {
            let catalog = self.session.capabilities()["catalog"] == true;
            self.session
                .require(if route == "new-thread-settings" || catalog {
                    "catalog"
                } else {
                    "contextReferences"
                })?;
            let thread = query.get("threadId").filter(|s| !s.is_empty());
            let project = query.get("projectId").filter(|s| !s.is_empty());
            let cwd = if let Some(thread) = thread.filter(|_| route == "context") {
                self.desktop.authorize(thread).await?;
                self.desktop.metadata(thread).await?["cwd"]
                    .as_str()
                    .map(PathBuf::from)
            } else if let Some(project) = project {
                let projects = if catalog {
                    self.desktop.projects().await?
                } else {
                    let source = query
                        .get("sourceThreadId")
                        .filter(|id| !id.is_empty())
                        .ok_or_else(|| BridgeError::invalid("请先选择新建聊天的来源。"))?;
                    self.desktop.scoped_projects(source).await?
                };
                Some(PathBuf::from(
                    projects
                        .iter()
                        .find(|p| p["projectId"] == *project)
                        .and_then(|p| p["path"].as_str())
                        .filter(|s| !s.is_empty())
                        .ok_or_else(|| BridgeError::invalid("该项目不可用，请重新选择。"))?,
                ))
            } else {
                None
            };
            return Ok(json_response(
                200,
                if route == "new-thread-settings" {
                    crate::config::new_thread_defaults(&self.paths.codex_home, cwd.as_deref())?
                } else if !catalog {
                    crate::context::references(
                        cwd.as_deref(),
                        query.get("query").map(String::as_str).unwrap_or(""),
                    )
                    .await
                } else {
                    crate::context::read(
                        &self.desktop,
                        cwd.as_deref(),
                        thread.map(String::as_str),
                        query.get("query").map(String::as_str).unwrap_or(""),
                    )
                    .await
                },
            ));
        }
        if route == "thread-catalog" && method == Method::GET {
            let search = query.get("query").map(|s| s.trim()).unwrap_or("");
            if search.encode_utf16().count() > 200 {
                return Err(BridgeError::invalid("搜索内容请控制在 200 字以内。"));
            }
            if self.session.capabilities()["catalog"] != true {
                let mut page = self.history.catalog(
                    query
                        .get("projectId")
                        .map(String::as_str)
                        .filter(|id| !id.is_empty()),
                    search,
                    query.get("cursor").map(String::as_str),
                    30,
                )?;
                page["historyOnly"] = json!(true);
                return Ok(json_response(200, page));
            }
            let project = if let Some(id) = query.get("projectId").filter(|s| !s.is_empty()) {
                Some(
                    self.desktop
                        .projects()
                        .await?
                        .into_iter()
                        .find(|p| p["projectId"] == *id)
                        .ok_or_else(|| BridgeError::new(404, "该项目已不可用，请返回项目列表。"))?,
                )
            } else {
                None
            };
            return Ok(json_response(
                200,
                crate::catalog::catalog(
                    &self.desktop,
                    project.as_ref(),
                    search,
                    query.get("cursor").map(String::as_str),
                    30,
                )
                .await?,
            ));
        }
        if route == "threads" && parts.len() == 2 {
            if method == Method::GET {
                return Ok(json_response(
                    200,
                    self.thread_list(query.get("cursor").map(String::as_str))
                        .await?,
                ));
            }
            if method != Method::POST {
                return Err(not_allowed());
            }
            self.session.require("create")?;
            let input = body(request).await.map_err(BridgeError::before_send)?;
            return Ok(json_response(
                200,
                self.submissions.create(operation_owner, input).await?,
            ));
        }
        if route != "threads" || parts.len() < 4 {
            return Err(BridgeError::new(404, "找不到这个接口。"));
        }
        let id = &parts[2];
        let operation = parts[3].as_str();
        if operation == "messages" && method == Method::POST {
            self.session.require("send")?;
            let input = body(request).await.map_err(BridgeError::before_send)?;
            return Ok(json_response(
                200,
                self.submissions.send(operation_owner, id, input).await?,
            ));
        }
        let metadata = self.desktop.metadata(id).await?;
        if method != Method::GET && method != Method::HEAD {
            let capability = match operation {
                "messages" => "send",
                "queue" => "queue",
                "settings" => "settings",
                "resume" => "resume",
                "interrupt" => "interrupt",
                "interactions"
                    if self.session.capabilities()["native"] == true
                        && parts
                            .get(4)
                            .and_then(|id| serde_json::from_str::<Value>(id).ok())
                            .is_some_and(|id| id[0] == "request_user_input_async") =>
                {
                    "asyncReplies"
                }
                "interactions" => "interactions",
                "goal" => "goals",
                "open" => "desktopNavigation",
                _ => "unsupported",
            };
            self.session.require(capability)?;
            self.desktop.authorize(id).await?;
        }
        if operation == "interactions" && parts.len() == 5 {
            if method != Method::POST {
                return Err(not_allowed());
            }
            return Ok(json_response(
                200,
                self.submissions
                    .reply(operation_owner, id, &parts[4], body(request).await?)
                    .await?,
            ));
        }
        if operation == "goal" {
            let (method_name, changes) = match method {
                Method::GET => ("thread/goal/get", json!({})),
                Method::DELETE => ("thread/goal/clear", json!({})),
                Method::PATCH => {
                    let mut changes = body(request).await?;
                    let keys = changes.as_object().unwrap();
                    if keys.is_empty()
                        || keys
                            .keys()
                            .any(|s| !["objective", "status", "tokenBudget"].contains(&s.as_str()))
                        || changes
                            .get("objective")
                            .is_some_and(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
                        || changes
                            .get("status")
                            .is_some_and(|v| v != "active" && v != "paused")
                        || changes.get("tokenBudget").is_some_and(|v| {
                            !v.is_null()
                                && !v
                                    .as_i64()
                                    .is_some_and(|n| n > 0 && n <= 9_007_199_254_740_991)
                        })
                    {
                        return Err(BridgeError::invalid("请填写目标；预算须为正整数，或留空。"));
                    }
                    if let Some(objective) = changes["objective"].as_str() {
                        changes["objective"] = json!(objective.trim());
                    }
                    ("thread/goal/set", changes)
                }
                _ => return Err(not_allowed()),
            };
            return Ok(json_response(
                200,
                self.goals.request(id, method_name, changes).await?,
            ));
        }
        match (operation, method) {
            ("history", Method::GET) => Ok(json_response(
                200,
                self.enrich(
                    id,
                    self.thread_page(id, query.get("cursor").map(String::as_str))
                        .await?,
                    &LiveView::default(),
                )
                .await,
            )),
            ("events", Method::GET) => {
                let appends = match (
                    query.get("textAppends").map(String::as_str),
                    query.get("compactTextAppends").map(String::as_str),
                ) {
                    (Some("1"), Some("1")) => TextAppends::Compact,
                    (Some("1"), _) => TextAppends::WithTurnState,
                    _ => TextAppends::Disabled,
                };
                self.events(id, appends).await
            }
            ("queue", Method::GET) => Ok(json_response(200, self.desktop.queue(id).await?)),
            ("queue", Method::POST) => Ok(json_response(
                200,
                self.desktop
                    .update_queue(id, QueueChange::parse(body(request).await?)?)
                    .await?,
            )),
            ("settings", Method::GET) => Ok(json_response(200, self.desktop.settings(id).await?)),
            ("settings", Method::POST) => {
                let settings = body(request).await?;
                let changes = if let Some(permission) = settings.get("permissions") {
                    if !permission.as_str().is_some_and(|s| {
                        ["read-only", "workspace-write", "full-access"].contains(&s)
                    }) {
                        return Err(BridgeError::invalid("请选择有效的访问权限。"));
                    }
                    json!({"permissions":permission})
                } else {
                    let changes = crate::state::select(&settings, &["model", "effort", "mode"]);
                    if changes.as_object().unwrap().is_empty()
                        || changes
                            .get("model")
                            .is_some_and(|v| !v.as_str().is_some_and(valid_model))
                        || changes.get("effort").is_some_and(|v| {
                            !v.is_null() && !v.as_str().is_some_and(|s| EFFORTS.contains(&s))
                        })
                        || changes
                            .get("mode")
                            .is_some_and(|v| v != "default" && v != "plan")
                    {
                        return Err(BridgeError::invalid("模型、思考强度或模式无效。"));
                    }
                    changes
                };
                let mut applied = self.desktop.update_settings(id, changes).await?;
                applied["applied"] = json!(true);
                Ok(json_response(200, applied))
            }
            ("interrupt", Method::POST) => {
                let input = body(request).await?;
                let turn = input["turnId"]
                    .as_str()
                    .filter(|s| !s.is_empty() && s.len() <= 100)
                    .ok_or_else(|| BridgeError::invalid("没有正在执行的任务。"))?;
                Ok(json_response(200, self.desktop.interrupt(id, turn).await?))
            }
            ("resume", Method::POST) => {
                let input = body(request).await?;
                let turn = input["turnId"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 100)
                    .ok_or_else(|| BridgeError::invalid("需要明确要继续的轮次。"))?;
                Ok(json_response(
                    200,
                    self.submissions.resume(operation_owner, id, turn).await?,
                ))
            }
            ("open", Method::POST) => {
                let result = self
                    .desktop
                    .tools_call(
                        "navigate_to_codex_page",
                        json!({"threadId":id}),
                        Some(id),
                        None,
                    )
                    .await?;
                if result["navigated"] != true {
                    return Err(BridgeError::unavailable(
                        "官方桌面尚未确认打开此对话，请在电脑核对。",
                    ));
                }
                Ok(json_response(200, json!({"ok":true})))
            }
            ("hooks", Method::GET) => Ok(json_response(
                200,
                crate::context::hooks(
                    &self.paths.codex_home,
                    metadata["cwd"].as_str().map(Path::new),
                ),
            )),
            ("item", Method::GET) => {
                let item_id = query.get("itemId").map(String::as_str).unwrap_or("");
                let item = match self.desktop.live_item(id, item_id).await {
                    Some(item) => Some(item),
                    None => self.history.item(id, item_id)?,
                }
                .ok_or_else(|| BridgeError::new(404, "该输出还未保存，请稍后重试。"))?;
                Ok(json_response(200, json!({"item":item})))
            }
            ("attachment", Method::GET | Method::HEAD) => {
                let item_id = query.get("itemId").map(String::as_str).unwrap_or("");
                let item = match self.desktop.live_item(id, item_id).await {
                    Some(item) => Some(item),
                    None => self.history.item(id, item_id)?,
                }
                .ok_or_else(|| BridgeError::new(403, "聊天中没有引用这个附件。"))?;
                let path = query.get("path").map(String::as_str).unwrap_or("");
                if query.get("preview").is_some_and(|value| value == "file") {
                    return crate::preview::response(
                        &self.paths.codex_home,
                        &metadata,
                        id,
                        &item,
                        path,
                        request,
                    )
                    .await;
                }
                if !crate::images::attachment_paths(&item, &self.images.directory)
                    .iter()
                    .any(|p| p == path)
                {
                    return Err(BridgeError::new(403, "聊天中没有引用这个附件。"));
                }
                crate::preview::check_attachment_type(Path::new(path))?;
                let mime = mime_guess::from_path(path)
                    .first_or_octet_stream()
                    .to_string();
                let image = mime.starts_with("image/") && !mime.contains("svg");
                file_response(
                    Path::new(path),
                    if image {
                        &mime
                    } else {
                        "application/octet-stream"
                    },
                    request.method() == Method::HEAD,
                    "private, no-store",
                    (!image).then_some(
                        Path::new(path)
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .as_ref(),
                    ),
                )
                .await
            }

            _ => Err(not_allowed()),
        }
    }
    async fn thread_list(self: &Arc<Self>, cursor: Option<&str>) -> Result<Value> {
        if self.session.capabilities()["catalog"] != true {
            let mut page = self.history.thread_list(cursor)?;
            page["historyOnly"] = json!(true);
            return Ok(page);
        }
        let result = crate::catalog::thread_list(&self.desktop, cursor).await?;
        Ok(result)
    }
    async fn enrich(&self, id: &str, mut snapshot: Value, view: &LiveView) -> Value {
        snapshot["capabilities"] = self.session.capabilities();
        if snapshot["syncMode"] == "desktop-poll" {
            for capability in [
                "queue",
                "queueStart",
                "queueEdit",
                "queueReorder",
                "interrupt",
                "interactions",
            ] {
                snapshot["capabilities"][capability] = json!(false);
            }
        }
        if snapshot["capabilities"]["native"] == true {
            if let Some(entries) = snapshot["interactions"].as_array_mut() {
                if let Err(error) = self
                    .submissions
                    .operations
                    .interaction_delivery(id, entries)
                {
                    snapshot["capabilities"]["asyncReplies"] = json!(false);
                    snapshot["syncError"] = json!(error.message);
                }
            }
        }
        let observed = (snapshot["syncMode"] == "desktop-poll")
            .then(|| snapshot["firstTurnObserved"].as_str())
            .flatten();
        match self.submissions.operations.creation_pending(id, observed) {
            Ok(pending) => {
                snapshot["firstTurnPending"] = json!(pending);
                if pending {
                    snapshot["capabilities"]["send"] = json!(false);
                }
            }
            Err(error) => {
                snapshot["capabilities"]["send"] = json!(false);
                snapshot["syncError"] = json!(error.message);
            }
        }
        if snapshot["connection"] != "live" {
            snapshot["capabilities"]["send"] = json!(false);
            snapshot["capabilities"]["queue"] = json!(false);
        }
        if snapshot["connection"] == "live" && snapshot["syncMode"] != "desktop-poll" {
            snapshot["thread"]
                .as_object_mut()
                .map(|thread| thread.shift_remove("projectId"));
        }
        if let Some(project) = self.desktop.project_assignment(id).await {
            snapshot["thread"]["projectId"] = project;
        }
        if let Some(queue) = &view.queue {
            snapshot["queue"] = queue.clone();
        }
        if let Some(state) = self.goals.states.read().await.get(id) {
            snapshot["thread"]["goal"] = state["goal"].clone();
        }
        let completed = self
            .goals
            .remember_completed(id, &snapshot["thread"]["completedGoal"])
            .await;
        if !completed.is_null() {
            snapshot["thread"]["completedGoal"] = completed;
        }
        summarize_snapshot(snapshot)
    }
    async fn thread_page(self: &Arc<Self>, id: &str, cursor: Option<&str>) -> Result<Value> {
        match self.history.page(id, cursor, 5) {
            Ok(page) => Ok(page),
            Err(error) if error.status == 404 && cursor.is_none() => {
                let mut snapshot = self.desktop.snapshot(id).await?;
                snapshot["nextCursor"] = Value::Null;
                Ok(snapshot)
            }
            Err(error) => Err(error),
        }
    }
    async fn events(self: &Arc<Self>, id: &str, appends: TextAppends) -> Result<Response<Body>> {
        let capabilities = self.session.capabilities();
        let native = capabilities["native"] == true;
        let mut history = if native {
            Value::Null
        } else {
            let mut page = self.thread_page(id, None).await?;
            if page.get("connection").is_none() {
                page["connection"] = json!("history");
            }
            page
        };
        if capabilities["readLive"] != true {
            let initial = self.enrich(id, history, &LiveView::default()).await;
            let (sender, receiver) = mpsc::channel::<std::result::Result<Event, Infallible>>(4);
            sender.send(Ok(event("snapshot", &initial))).await.unwrap();
            let this = self.clone();
            let thread = id.to_string();
            let mut rotations = self.rotations.subscribe();
            self.spawn_work(async move {
                let mut previous = initial;
                let mut refresh = tokio::time::interval(Duration::from_secs(5));
                refresh.tick().await;
                loop {
                    tokio::select! {
                        _ = this.streams.cancelled() => break,
                        _ = sender.closed() => break,
                        _ = rotations.recv() => break,
                        _ = refresh.tick() => {
                            match this.history.page(&thread, None, 5) {
                                Ok(page) => {
                                    let mut snapshot = this.enrich(&thread, page, &LiveView::default()).await;
                                    snapshot["connection"] = json!("history");
                                    if let Ok(files) = this.history.changed_files(&thread) { snapshot["changedFileCount"] = json!(files.len()); }
                                    if snapshot != previous {
                                        if sender.send(Ok(event("snapshot", &snapshot))).await.is_err() { break; }
                                        previous = snapshot;
                                    }
                                }
                                Err(error) => {
                                    let _ = sender.send(Ok(event("desktop-error", &json!({"message":error.message})))).await;
                                    break;
                                }
                            }
                            if sender.send(Ok(event("heartbeat", &json!({})))).await.is_err() { break; }
                        }
                    }
                }
            });
            return Ok(Sse::new(ReceiverStream::new(receiver)).into_response());
        }
        let mut live = self.desktop.subscribe(id);
        let view = if native {
            // 同一聊天的订阅共用已校验归属的首帧。
            tokio::select! {
                _ = self.streams.cancelled() => return Err(BridgeError::unavailable("手机服务正在停止。")),
                ready = live.wait_for(|view| view.snapshot.is_some() || view.error.is_some()) => {
                    ready.map_err(|_| BridgeError::unavailable("聊天连接已关闭。"))?.clone()
                }
            }
        } else {
            live.borrow_and_update().clone()
        };
        if native {
            history = match &view.snapshot {
                Some(snapshot) => snapshot.clone(),
                None => {
                    let mut page = self.thread_page(id, None).await?;
                    page["connection"] = json!("history");
                    page["syncError"] = json!(view.error);
                    page
                }
            };
        }
        let mut initial = history.clone();
        if let Some(snapshot) = view.snapshot.as_ref().filter(|_| !native) {
            let mut turns = IndexMap::<String, Value>::new();
            for turn in history["turns"]
                .as_array()
                .into_iter()
                .flatten()
                .chain(snapshot["turns"].as_array().into_iter().flatten())
            {
                if let Some(id) = turn["id"].as_str() {
                    turns.insert(id.into(), turn.clone());
                }
            }
            initial = snapshot.clone();
            initial["turns"] = json!(turns.into_values().collect::<Vec<_>>());
            initial["nextCursor"] = history["nextCursor"].clone();
        }
        initial = self.enrich(id, initial, &view).await;
        // 统计已落库的全部改动，再纳入实时窗口中尚未落库的成功记录。
        let mut changed_files = self.history.changed_files(id).ok();
        if let Some(files) = &mut changed_files {
            for turn in initial["turns"].as_array().into_iter().flatten() {
                for item in turn["items"].as_array().into_iter().flatten() {
                    record_file_change(files, item);
                }
            }
            initial["changedFileCount"] = json!(files.len());
        }
        let (sender, receiver) = mpsc::channel::<std::result::Result<Event, Infallible>>(4);
        sender.send(Ok(event("snapshot", &initial))).await.unwrap();
        if let Some(error) = &view.error {
            sender
                .send(Ok(event("desktop-error", &json!({"message":error}))))
                .await
                .unwrap();
        }
        let this = self.clone();
        let thread = id.to_string();
        let goals = self.goals.clone();
        let goal_thread = thread.clone();
        let mut reconnects = goals.reconnects.subscribe();
        self.spawn_work(async move {
            let _ = goals
                .request(&goal_thread, "thread/goal/get", json!({}))
                .await;
        });
        let mut changes = self.goals.changes.subscribe();
        let mut rotations = self.rotations.subscribe();
        self.spawn_work(async move{
            let mut previous=initial;let mut previous_error=view.error.clone();let mut previous_queue=view.queue;let mut heartbeat=tokio::time::interval(Duration::from_secs(15));heartbeat.tick().await;
            loop{
                let changed=tokio::select!{
                    _=this.streams.cancelled()=>break,_=sender.closed()=>break,_=rotations.recv()=>break,
                    connected=reconnects.recv()=>{if connected.is_ok(){let goals=this.goals.clone();let id=thread.clone();this.spawn_work(async move{let _=goals.request(&id,"thread/goal/get",json!({})).await;});}false},
                    _=heartbeat.tick()=>{if sender.send(Ok(event("heartbeat",&json!({})))).await.is_err(){break;}false},
                    changed=live.changed()=>{if changed.is_err(){break;}true},
                    changed=changes.recv()=>changed.is_ok_and(|id|id==thread),
                };
                if !changed{continue;}
                let view=live.borrow_and_update().clone();
                if view.queue!=previous_queue{if let Some(queue)=&view.queue{if sender.send(Ok(event("queue",queue))).await.is_err(){break;}}previous_queue=view.queue.clone();}
                if view.error!=previous_error{if let Some(error)=&view.error{if sender.send(Ok(event("desktop-error",&json!({"message":error})))).await.is_err(){break;}}previous_error=view.error.clone();}
                if view.error.is_some(){
                    let mut snapshot=this.enrich(&thread,view.snapshot.clone().unwrap_or_else(||history.clone()),&view).await;
                    snapshot["connection"]=json!("history");snapshot["capabilities"]["send"]=json!(false);
                    if sender.send(Ok(event("snapshot",&snapshot))).await.is_err(){break;}
                    previous=snapshot;continue;
                }
                let snapshot=this.enrich(&thread,view.snapshot.clone().unwrap_or_else(||history.clone()),&view).await;
                if snapshot["capabilities"]!=previous["capabilities"] || snapshot["firstTurnPending"]!=previous["firstTurnPending"] {
                    if sender.send(Ok(event("snapshot",&snapshot))).await.is_err(){break;}
                    previous=snapshot;continue;
                }
                if let Some(update)=stream_update(&previous,&snapshot,appends,changed_files.as_mut()){if sender.send(Ok(event("update",&update))).await.is_err(){break;}}
                this.notifications.observe(&snapshot).await;previous=snapshot;
            }
        });
        let mut response = Sse::new(ReceiverStream::new(receiver)).into_response();
        response
            .headers_mut()
            .insert("cache-control", "no-cache, no-transform".parse().unwrap());
        response
            .headers_mut()
            .insert("x-accel-buffering", "no".parse().unwrap());
        Ok(response)
    }
    async fn static_file(
        &self,
        method: &Method,
        headers: &HeaderMap,
        raw_path: &str,
    ) -> Result<Response<Body>> {
        if method != Method::GET && method != Method::HEAD {
            return Err(not_allowed());
        }
        let path = percent_encoding::percent_decode_str(raw_path)
            .decode_utf8()
            .map_err(|_| BridgeError::invalid("请求地址无效。"))?;
        if path == "/pair" || path == "/pair/" {
            return Ok(Response::builder()
                .status(302)
                .header("location", "/")
                .header("cache-control", "no-store")
                .body(Body::empty())
                .unwrap());
        }
        let relative = Path::new(path.trim_start_matches('/'));
        if relative.components().any(|c| {
            !matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        }) {
            return Err(BridgeError::new(403, "无效文件路径。"));
        }
        let target = self.paths.web_root.join(relative);
        let file = if target.is_file() {
            target.clone()
        } else {
            self.paths.web_root.join("index.html")
        };
        if !file.is_file() {
            return Err(BridgeError::unavailable(
                "运行包缺少手机网页资源，请重新构建或安装。",
            ));
        }
        let stat = tokio::fs::metadata(&file).await?;
        let stamp = stat
            .modified()?
            .duration_since(std::time::SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let etag = format!("W/\"{}-{stamp}\"", stat.len());
        if header(headers, "if-none-match")
            .split(',')
            .any(|v| v.trim() == etag)
        {
            return Ok(Response::builder()
                .status(304)
                .header("etag", etag)
                .body(Body::empty())
                .unwrap());
        }
        let mime = mime_guess::from_path(&file)
            .first_or_octet_stream()
            .to_string();
        let content_type = if mime.starts_with("text/") || mime == "application/javascript" {
            format!("{mime}; charset=utf-8")
        } else {
            mime
        };
        let cache = if file == target && path.starts_with("/assets/") {
            "public, max-age=31536000, immutable"
        } else {
            "no-cache"
        };
        let mut response =
            file_response(&file, &content_type, method == Method::HEAD, cache, None).await?;
        response.headers_mut().insert("etag", etag.parse().unwrap());
        response
            .headers_mut()
            .insert("referrer-policy", "same-origin".parse().unwrap());
        Ok(response)
    }

    pub(crate) async fn relay_http(
        self: &Arc<Self>,
        request: Request<Body>,
        phone: RelayPhone,
    ) -> Response<Body> {
        self.serve_request(request, Some(phone)).await
    }

    async fn serve_request(
        self: &Arc<Self>,
        request: Request<Body>,
        phone: Option<RelayPhone>,
    ) -> Response<Body> {
        if self.closing.load(Ordering::Acquire) {
            return BridgeError::unavailable("连接服务正在重启，请稍后重试。")
                .before_send()
                .into_response();
        }
        let app = self.clone();
        match self
            .spawn_work(async move { app.route(request, phone).await })
            .await
        {
            Ok(Some(Ok(response))) => response,
            Ok(Some(Err(error))) => error.into_response(),
            Ok(None) => {
                BridgeError::unavailable("服务已停止，请查看已保存的操作结果。").into_response()
            }
            Err(error) => BridgeError::unavailable(error.to_string()).into_response(),
        }
    }
}

impl Server {
    pub async fn stop(mut self) -> Result<()> {
        self.app.closing.store(true, Ordering::Release);
        self.shutdown.cancel();
        self.app.streams.cancel();
        self.app.work.close();
        let served = match tokio::time::timeout(Duration::from_secs(35), async {
            self.app.work.wait().await;
            (&mut self.task).await
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                self.app.abort_work.cancel();
                self.app.session.close().await;
                self.app.work.wait().await;
                self.task.await
            }
        };
        self.app.submissions.operations.interrupted()?;
        self.app.desktop.close().await;
        self.app.goals.close().await;
        self.app.session.close().await;
        if let Some(usage) = self.app.usage.lock().await.as_ref() {
            usage.close().await;
        }
        self.app.submissions.operations.release()?;
        served.map_err(|e| BridgeError::unavailable(e.to_string()))??;
        Ok(())
    }
}
async fn handle(State(app): State<Arc<Bridge>>, request: Request<Body>) -> Response<Body> {
    app.serve_request(request, None).await
}
pub fn json_response(status: u16, value: Value) -> Response<Body> {
    let mut response = (StatusCode::from_u16(status).unwrap(), Json(value)).into_response();
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
async fn body(request: Request<Body>) -> Result<Value> {
    let bytes = to_bytes(request.into_body(), 1024 * 1024)
        .await
        .map_err(|_| BridgeError::new(413, "输入内容超过 1 MB。"))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| BridgeError::invalid("请求内容不是有效的 JSON。"))?;
    if !value.is_object() {
        return Err(BridgeError::invalid("请求内容不是有效的 JSON。"));
    }
    Ok(value)
}
fn not_allowed() -> BridgeError {
    BridgeError::new(405, "不支持这种操作。")
}
async fn file_response(
    path: &Path,
    mime: &str,
    head: bool,
    cache: &str,
    download: Option<&str>,
) -> Result<Response<Body>> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| BridgeError::new(404, "文件不存在。"))?;
    let stat = file.metadata().await?;
    if !stat.is_file() {
        return Err(BridgeError::new(404, "文件不存在。"));
    }
    let mut builder = Response::builder()
        .header("content-type", mime)
        .header("content-length", stat.len())
        .header("cache-control", cache)
        .header("x-content-type-options", "nosniff");
    if let Some(filename) = download {
        builder = builder.header(
            "content-disposition",
            format!(
                "attachment; filename*=UTF-8''{}",
                percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC)
            ),
        );
    }
    let body = if head {
        Body::empty()
    } else {
        Body::from_stream(tokio_util::io::ReaderStream::new(file))
    };
    Ok(builder.body(body).unwrap())
}
fn event(name: &str, value: &Value) -> Event {
    Event::default().event(name).data(value.to_string())
}
fn record_file_change(files: &mut HashSet<String>, item: &Value) {
    if item["type"] != "fileChange" || item["status"] != "completed" {
        return;
    }
    for change in item["changes"].as_array().into_iter().flatten() {
        if let Some(path) = change["path"]
            .as_str()
            .filter(|path| !path.is_empty() && !files.contains(*path))
        {
            files.insert(path.to_owned());
        }
    }
}
fn stream_update(
    previous: &Value,
    snapshot: &Value,
    appends: TextAppends,
    mut changed_files: Option<&mut HashSet<String>>,
) -> Option<Value> {
    let min_append_offset = match appends {
        TextAppends::Disabled => None,
        TextAppends::WithTurnState => Some(512),
        TextAppends::Compact => Some(0),
    };
    let previous_file_count = changed_files.as_ref().map(|files| files.len());
    let mut old = HashMap::new();
    for turn in previous["turns"].as_array().into_iter().flatten() {
        for item in turn["items"].as_array().into_iter().flatten() {
            old.insert((turn["id"].to_string(), item["id"].to_string()), item);
        }
    }
    let mut updates = vec![];
    let mut text_appends = vec![];
    for turn in snapshot["turns"].as_array().into_iter().flatten() {
        for item in turn["items"].as_array().into_iter().flatten() {
            let previous = old
                .get(&(turn["id"].to_string(), item["id"].to_string()))
                .copied();
            if previous == Some(item) {
                continue;
            }
            if let Some(files) = changed_files.as_deref_mut() {
                record_file_change(files, item);
            }
            if let Some(mut append) = previous
                .zip(min_append_offset)
                .and_then(|(old, min_offset)| text_append(old, item, min_offset))
            {
                append["turnId"] = turn["id"].clone();
                text_appends.push(append);
            } else {
                updates.push(json!({"turnId":turn["id"],"item":item}));
            }
        }
    }
    let meta = |snapshot: &Value| {
        let mut value = crate::state::select(snapshot, &["thread", "connection"]);
        value["interactions"] = snapshot.get("interactions").cloned().unwrap_or(json!([]));
        value["turns"] = json!(snapshot["turns"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|turn| {
                let mut value = Value::Object(
                    turn.as_object()
                        .unwrap()
                        .iter()
                        .filter(|(field, _)| *field != "items")
                        .map(|(field, value)| (field.clone(), value.clone()))
                        .collect(),
                );
                value["itemOrder"] = json!(turn["items"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|i| &i["id"])
                    .collect::<Vec<_>>());
                value
            })
            .collect::<Vec<_>>());
        value
    };
    let mut result = meta(snapshot);
    if updates.is_empty()
        && (text_appends.is_empty() || appends == TextAppends::Compact)
        && result == meta(previous)
    {
        if text_appends.is_empty() {
            return None;
        }
        return Some(json!({"connection":result["connection"],"textAppends":text_appends}));
    }
    result["updates"] = json!(updates);
    if let Some(count) = changed_files.map(|files| files.len()) {
        if Some(count) != previous_file_count {
            result["changedFileCount"] = json!(count);
        }
    }
    if min_append_offset.is_some() {
        result["textAppends"] = json!(text_appends);
    }
    Some(result)
}
