//! 保留官方 stdio 的单实例代理；手机只接入代理，不再初始化或启动官方后端。
use crate::{runtime::RuntimeInfo, transport::Pipe, Result};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::mpsc,
};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{Request, Response},
        Message,
    },
};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

struct Client {
    sender: mpsc::Sender<Value>,
    subscribed: HashSet<String>,
    cancel: CancellationToken,
    attached: bool,
}
struct Pending {
    client: Option<String>,
    id: Value,
    method: String,
    thread: Option<String>,
    ephemeral: bool,
}
struct ServerRequest {
    message: Value,
    recipients: HashSet<Option<String>>,
}
struct ImageCreation {
    client: String,
    output: String,
    input: Value,
    message_id: String,
    thread: Option<String>,
}
enum Event {
    Connected(String, Client),
    Message(String, Value),
    Disconnected(String),
}
struct Router {
    instance: String,
    next_id: u64,
    clients: HashMap<String, Client>,
    pending: HashMap<String, Pending>,
    server_requests: HashMap<String, ServerRequest>,
    desktop_threads: HashSet<String>,
    image_creations: HashMap<String, ImageCreation>,
}
impl Router {
    fn new(instance: String) -> Self {
        Self {
            instance,
            next_id: 0,
            clients: HashMap::new(),
            pending: HashMap::new(),
            server_requests: HashMap::new(),
            desktop_threads: HashSet::new(),
            image_creations: HashMap::new(),
        }
    }
    fn deliver(&self, client: &str, message: Value) {
        if let Some(peer) = self.clients.get(client) {
            if peer.sender.try_send(message).is_err() {
                peer.cancel.cancel();
            }
        }
    }
    fn disconnect(&mut self, id: &str) {
        self.clients.remove(id);
        self.pending
            .retain(|_, pending| pending.client.as_deref() != Some(id));
        for request in self.server_requests.values_mut() {
            request.recipients.remove(&Some(id.into()));
        }
    }
    fn outgoing(
        &mut self,
        client: Option<&str>,
        mut message: Value,
        initialized: Option<&Value>,
    ) -> Option<Value> {
        let method = message["method"].as_str().map(str::to_owned);
        if let Some(method) = &method {
            if let Some(client) = client {
                let id = message["id"].clone();
                if method != "codexMobile/attach" && !self.clients.get(client)?.attached {
                    self.deliver(client, json!({"id":id,"error":{"code":-32600,"message":"请先完成当前共享连接的认证与初始化。"}}));
                    return None;
                }
                if method == "codexMobile/attach" && initialized.is_some() {
                    self.clients.get_mut(client)?.attached = true;
                }
                let result = match method.as_str() {
                    "codexMobile/attach" => Some(
                        initialized
                            .cloned()
                            .map(|mut value| {
                                value["codexMobileNativeImages"] = json!(true);
                                Ok(value)
                            })
                            .unwrap_or_else(|| Err("官方桌面尚未完成初始化。")),
                    ),
                    "initialize" | "initialized" => Some(Err("手机不能再次初始化官方桌面后端。")),
                    "thread/start" | "thread/fork" => {
                        Some(Err("新建必须使用官方桌面入口，不能创建替代聊天。"))
                    }
                    "codexMobile/createImage/prepare" => {
                        Some(self.prepare_image_creation(client, &message["params"]))
                    }
                    "codexMobile/createImage/finish" => Some(Ok({
                        let key = message["params"]["parentTurnId"]
                            .as_str()
                            .unwrap_or_default();
                        let thread = self
                            .image_creations
                            .get(key)
                            .filter(|creation| creation.client == client)
                            .and_then(|creation| creation.thread.clone());
                        if thread.is_some() {
                            self.image_creations.remove(key);
                        }
                        json!({"threadId":thread})
                    })),
                    "codexMobile/thread/route" => Some(Ok(
                        json!({"threadId":message["params"]["threadId"], "instanceId":self.instance,
                        "available":message["params"]["threadId"].as_str().is_some_and(|id| self.desktop_threads.contains(id))}),
                    )),
                    "thread/unsubscribe" => {
                        if let Some(thread) = message["params"]["threadId"].as_str() {
                            self.clients.get_mut(client)?.subscribed.remove(thread);
                            for request in self
                                .server_requests
                                .values_mut()
                                .filter(|request| request.message["params"]["threadId"] == thread)
                            {
                                request.recipients.remove(&Some(client.into()));
                            }
                        }
                        // 多个视图共用一个官方 stdio 客户端，手机退出不能卸载桌面线程。
                        Some(Ok(json!({})))
                    }
                    _ => None,
                };
                if let Some(result) = result {
                    if valid_id(&id) {
                        self.deliver(
                            client,
                            match result {
                                Ok(result) => json!({"id":id,"result":result}),
                                Err(error) => {
                                    json!({"id":id,"error":{"code":-32600,"message":error}})
                                }
                            },
                        );
                    }
                    return None;
                }
                if let Some(thread) = message["params"]["threadId"].as_str() {
                    if !matches!(
                        method.as_str(),
                        "thread/read" | "thread/turns/list" | "thread/items/list"
                    ) && !self.desktop_threads.contains(thread)
                    {
                        self.deliver(client, json!({"id":id,"error":{"code":-32600,"message":"此聊天未由当前桌面 App Server 确认，不能恢复或写入。"}}));
                        return None;
                    }
                }
            }
            // 创建仍由官方工具完成；只替换本次已登记首轮的内容，保留项目、权限及官方生命周期。
            if client.is_none() && method == "turn/start" {
                let key = message["params"]["parentTurnId"]
                    .as_str()
                    .unwrap_or_default();
                if let Some(creation) = self.image_creations.get_mut(key) {
                    let params = &mut message["params"];
                    if creation.thread.is_some()
                        || params["toolOutput"]["name"] != "create_thread"
                        || params["toolOutput"]["namespace"] != "codex_app"
                        || params["toolOutput"]["output"] != creation.output
                        || params["input"] != json!([])
                        || !params["threadId"]
                            .as_str()
                            .is_some_and(|id| self.desktop_threads.contains(id))
                    {
                        // 不把缺失图片的首轮送给模型；回执沿原官方请求返回。
                        return Some(
                            json!({"id":message["id"],"error":{"code":-32600,"message":"原生图片首轮与官方创建请求不一致，未发送。"}}),
                        );
                    }
                    creation.thread = params["threadId"].as_str().map(str::to_owned);
                    params["input"] = creation.input.clone();
                    params["clientUserMessageId"] = json!(creation.message_id);
                    params.as_object_mut().unwrap().remove("toolOutput");
                }
            }
            if valid_id(&message["id"]) {
                self.next_id += 1;
                let id = format!("codex-mobile:{}:{}", self.instance, self.next_id);
                self.pending.insert(
                    id.clone(),
                    Pending {
                        client: client.map(str::to_owned),
                        id: message["id"].clone(),
                        method: method.clone(),
                        thread: message["params"]["threadId"].as_str().map(str::to_owned),
                        ephemeral: message["params"]["ephemeral"] == true,
                    },
                );
                message["id"] = json!(id);
            }
            return Some(message);
        }
        // 服务请求和客户端 RPC 的 ID 空间独立；只接受收到该请求的连接的首个回复。
        if valid_id(&message["id"]) {
            let key = message["id"].to_string();
            if self
                .server_requests
                .get(&key)
                .is_some_and(|request| request.recipients.contains(&client.map(str::to_owned)))
            {
                self.server_requests.remove(&key);
                return Some(message);
            }
        }
        None
    }
    fn prepare_image_creation(
        &mut self,
        client: &str,
        params: &Value,
    ) -> std::result::Result<Value, &'static str> {
        let source = params["sourceThreadId"].as_str().unwrap_or_default();
        let key = params["parentTurnId"].as_str().unwrap_or_default();
        let message_id = params["messageId"].as_str().unwrap_or_default();
        let prompt = params["prompt"].as_str().unwrap_or_default();
        if source.is_empty()
            || message_id.len() < 16
            || key != format!("mobile-operation-{message_id}")
            || self.image_creations.contains_key(key)
            || !params["input"]
                .as_array()
                .is_some_and(|input| input.iter().any(|item| item["type"] == "localImage"))
        {
            return Err("图片首轮登记无效，请检查当前官方桌面连接。");
        }
        let escaped = prompt
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        self.image_creations.insert(key.into(), ImageCreation {
            client:client.into(), output:format!("<codex_delegation>\n  <source_thread_id>{source}</source_thread_id>\n  <input>{escaped}</input>\n</codex_delegation>"),
            input:params["input"].clone(), message_id:message_id.into(), thread:None,
        });
        Ok(json!({"prepared":true}))
    }
    fn incoming(&mut self, mut message: Value) -> Option<Value> {
        if message["method"].is_string() {
            if message["method"] == "serverRequest/resolved" {
                self.server_requests
                    .remove(&message["params"]["requestId"].to_string());
            }
            if message["method"] == "turn/completed" || message["method"] == "thread/closed" {
                self.server_requests.retain(|_, request| {
                    request.message["params"]["threadId"] != message["params"]["threadId"]
                        || (message["method"] == "turn/completed"
                            && request.message["params"]["turnId"]
                                != message["params"]["turn"]["id"])
                });
            }
            let thread = message["params"]["threadId"].as_str();
            let recipients: Vec<String> = self
                .clients
                .iter()
                .filter(|(_, client)| {
                    client.attached && thread.is_none_or(|id| client.subscribed.contains(id))
                })
                .map(|(id, _)| id.clone())
                .collect();
            if valid_id(&message["id"]) {
                let mut owners = HashSet::from([None]);
                owners.extend(recipients.iter().cloned().map(Some));
                self.server_requests.insert(
                    message["id"].to_string(),
                    ServerRequest {
                        message: message.clone(),
                        recipients: owners,
                    },
                );
            }
            if message["method"] == "thread/closed" {
                if let Some(thread) = thread {
                    self.desktop_threads.remove(thread);
                }
            }
            for recipient in recipients {
                self.deliver(&recipient, message.clone());
            }
            return Some(message);
        }
        let Some(id) = message["id"].as_str() else {
            return Some(message);
        };
        let Some(pending) = self.pending.remove(id) else {
            // 已断开的手机请求仍可能执行；丢弃其迟到回执，绝不转给桌面或重新投递。
            return (!id.starts_with(&format!("codex-mobile:{}:", self.instance)))
                .then_some(message);
        };
        if message.get("error").is_none() {
            if matches!(pending.method.as_str(), "thread/start" | "thread/resume") {
                if let Some(thread) = message["result"]["thread"]["id"].as_str() {
                    if pending.client.is_none()
                        && !pending.ephemeral
                        && message["result"]["thread"]["ephemeral"] != true
                    {
                        self.desktop_threads.insert(thread.into());
                    }
                    if let Some(client) = pending
                        .client
                        .as_ref()
                        .and_then(|id| self.clients.get_mut(id))
                    {
                        client.subscribed.insert(thread.into());
                    }
                    if let Some(client) = pending.client.as_ref() {
                        let requests = self
                            .server_requests
                            .values_mut()
                            .filter(|request| request.message["params"]["threadId"] == thread)
                            .map(|request| {
                                request.recipients.insert(Some(client.clone()));
                                request.message.clone()
                            })
                            .collect::<Vec<_>>();
                        for request in requests {
                            self.deliver(client, request);
                        }
                    }
                }
            }
            if pending.client.is_none() && pending.method == "thread/unsubscribe" {
                if let Some(thread) = pending.thread {
                    self.desktop_threads.remove(&thread);
                }
            }
        }
        message["id"] = pending.id;
        match pending.client {
            Some(client) => {
                self.deliver(&client, message);
                None
            }
            None => Some(message),
        }
    }
}
fn valid_id(id: &Value) -> bool {
    id.is_string() || id.is_i64() || id.is_u64()
}
async fn write_line(output: &mut (impl AsyncWrite + Unpin), value: &Value) -> Result<()> {
    output.write_all(value.to_string().as_bytes()).await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}
async fn serve_client(
    stream: TcpStream,
    token: String,
    events: mpsc::Sender<Event>,
    cancel: CancellationToken,
) {
    #[allow(
        clippy::result_large_err,
        reason = "tungstenite 回调要求按值返回 HTTP 响应，不能替换为 Box"
    )]
    let upgrade = accept_hdr_async(stream, move |request: &Request, response: Response| {
        let candidate = request
            .headers()
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .unwrap_or("");
        if request.uri().path() != "/"
            || request.uri().query().is_some()
            || !bool::from(token.as_bytes().ct_eq(candidate.as_bytes()))
        {
            let mut denied = tokio_tungstenite::tungstenite::http::Response::new(Some(
                "共享连接认证失败。".into(),
            ));
            *denied.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::UNAUTHORIZED;
            return Err(denied);
        }
        Ok(response)
    });
    let socket = tokio::select! { _=cancel.cancelled()=>return, result=tokio::time::timeout(Duration::from_secs(5),upgrade)=>match result { Ok(Ok(socket))=>socket, _=>return } };
    let id = uuid::Uuid::new_v4().to_string();
    let (sender, mut outgoing) = mpsc::channel(64);
    if events
        .send(Event::Connected(
            id.clone(),
            Client {
                sender,
                subscribed: HashSet::new(),
                cancel: cancel.clone(),
                attached: false,
            },
        ))
        .await
        .is_err()
    {
        return;
    }
    let (mut writer, mut reader) = socket.split();
    loop {
        tokio::select! {
            _=cancel.cancelled()=>break,
            message=outgoing.recv()=>match message {
                Some(message)=>{
                    let sent=tokio::select! {_=cancel.cancelled()=>break, result=writer.send(Message::Text(message.to_string().into()))=>result};
                    if sent.is_err() {break;}
                },
                None=>break,
            },
            message=reader.next()=>match message {
                Some(Ok(Message::Text(text)))=>match serde_json::from_str(&text) {
                    Ok(value)=>if events.send(Event::Message(id.clone(),value)).await.is_err() { break; },
                    Err(_)=>break,
                },
                Some(Ok(Message::Ping(bytes)))=>if writer.send(Message::Pong(bytes)).await.is_err() { break; },
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None=>break,
                _=>{},
            }
        }
    }
    let _ = events.send(Event::Disconnected(id)).await;
}

pub(crate) async fn run<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    mut output: W,
    upstream: Pipe,
    listener: TcpListener,
    record: &mut RuntimeInfo,
    directory: PathBuf,
    first: &str,
) -> Result<()> {
    let (reader, mut writer) = tokio::io::split(upstream);
    let mut reader = BufReader::new(reader).lines();
    let mut input = BufReader::new(input).lines();
    let init: Value = serde_json::from_str(first)?;
    // 第一次 initialize 逐字投递一次，其能力与原始 ID 均不改写。
    writer.write_all(first.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    let mut initialized = None;
    let mut router = Router::new(record.instance_id.clone());
    let (events, mut incoming) = mpsc::channel(64);
    let tasks = TaskTracker::new();
    let cancel = CancellationToken::new();
    let mut heartbeat = tokio::time::interval(Duration::from_millis(250));
    let result = async {
        loop {
            tokio::select! {
                _=heartbeat.tick()=>if !record.desktop.alive() { break; },
                accepted=listener.accept()=>{
                    let (stream,_)=accepted?;
                    tasks.spawn(serve_client(stream,record.auth_token.clone(),events.clone(),cancel.child_token()));
                },
                line=input.next_line()=>match line? {
                    Some(line)=>match serde_json::from_str::<Value>(&line) {
                        Ok(message)=>{
                            let ready = record.state == "starting" && initialized.as_ref().is_some_and(|reply: &Value| {
                                message["method"] == "initialized"
                                    || (reply["userAgent"].as_str().is_some_and(|agent| agent.starts_with("Codex Desktop/0.171.0-alpha.13.2 "))
                                        && message["method"].is_string() && message["method"] != "initialize" && valid_id(&message["id"]))
                            });
                            if let Some(message)=router.outgoing(None,message,initialized.as_ref()) {
                                if message.get("error").is_some() { write_line(&mut output,&message).await?; }
                                else { write_line(&mut writer,&message).await?; }
                            }
                            // 当前桌面在初始化回执后直接请求账号，不再发送 initialized；以真实后续请求确认握手完成。
                            if ready { record.state="ready".into();record.publish(&directory)?; }
                        },
                        // 非 RPC 输入仍交由官方解释，不伪装成成功或重新启动后端。
                        Err(_)=>{ writer.write_all(line.as_bytes()).await?;writer.write_all(b"\n").await?;writer.flush().await?; },
                    },
                    None=>break,
                },
                line=reader.next_line()=>match line? {
                    Some(line)=>{
                        let value: Value=serde_json::from_str(&line).unwrap_or(Value::Null);
                        if initialized.is_none() && value.get("method").is_none() && value["id"]==init["id"] {
                            if crate::runtime::supported_initialize(&value["result"]) && value["result"]["codexHome"].as_str().is_some_and(|home| std::fs::canonicalize(home).ok().as_ref()==Some(&record.codex_home)) {
                                initialized=Some(value["result"].clone());
                            }
                            output.write_all(line.as_bytes()).await?;output.write_all(b"\n").await?;output.flush().await?;
                        } else if value.is_object() {
                            if let Some(message)=router.incoming(value) { write_line(&mut output,&message).await?; }
                        } else { output.write_all(line.as_bytes()).await?;output.write_all(b"\n").await?;output.flush().await?; }
                    },
                    None=>break,
                },
                event=incoming.recv()=>match event {
                    Some(Event::Connected(id,client))=>{router.clients.insert(id,client);},
                    Some(Event::Message(id,message))=>if let Some(message)=router.outgoing(Some(&id),message,if record.state=="ready" { initialized.as_ref() } else { None }) {write_line(&mut writer,&message).await?;},
                    Some(Event::Disconnected(id))=>router.disconnect(&id),
                    None=>break,
                }
            }
        }
        Ok(())
    }.await;
    // 结束手机连接后关闭官方 stdin，沿用原始 EOF 生命周期，不强杀未知任务。
    cancel.cancel();
    drop(incoming);
    tasks.close();
    tasks.wait().await;
    writer.shutdown().await?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn image_creation_uses_the_official_first_turn_once() {
        let mut router = Router::new("instance".into());
        let (phone, mut replies) = client();
        router.clients.insert("phone".into(), phone);
        // 来源授权由桌面入口完成；来源聊天可以尚未订阅控制通道。
        router.desktop_threads.insert("created".into());
        let key = "mobile-operation-image-create-12345";
        let input = json!([{"type":"text","text":"看 <图> & 回答","text_elements":[]},{"type":"localImage","path":"C:/synthetic.png"}]);
        let prepare = json!({"id":1,"method":"codexMobile/createImage/prepare","params":{
            "sourceThreadId":"source","parentTurnId":key,"messageId":"image-create-12345","prompt":"看 <图> & 回答","input":input,
        }});
        assert!(router
            .outgoing(Some("phone"), prepare.clone(), None)
            .is_none());
        assert_eq!(replies.try_recv().unwrap()["result"]["prepared"], true);
        router.outgoing(Some("phone"), prepare, None);
        assert!(replies.try_recv().unwrap()["error"].is_object());
        let output = router.image_creations[key].output.clone();
        let start = json!({"id":2,"method":"turn/start","params":{
            "threadId":"created","parentTurnId":key,"model":"selected-model","input":[],
            "toolOutput":{"name":"create_thread","namespace":"codex_app","output":output,"retain":true},
        }});
        let mut wrong = start.clone();
        wrong["params"]["toolOutput"]["output"] = json!("wrong source or prompt");
        assert!(router.outgoing(None, wrong, None).unwrap()["error"].is_object());
        let sent = router.outgoing(None, start.clone(), None).unwrap();
        assert_eq!(sent["params"]["input"], input);
        assert_eq!(sent["params"]["model"], "selected-model");
        assert!(sent["params"].get("toolOutput").is_none());
        assert_eq!(sent["params"]["clientUserMessageId"], "image-create-12345");
        assert!(router.outgoing(None, start, None).unwrap()["error"].is_object());
        router.outgoing(
            Some("phone"),
            json!({"id":3,"method":"codexMobile/createImage/finish","params":{"parentTurnId":key}}),
            None,
        );
        assert_eq!(replies.try_recv().unwrap()["result"]["threadId"], "created");
        assert!(router.image_creations.is_empty());
        let ordinary = json!({"id":4,"method":"turn/start","params":{"threadId":"created","input":[{"type":"text","text":"ordinary"}]}});
        assert_eq!(
            router.outgoing(None, ordinary.clone(), None).unwrap()["params"],
            ordinary["params"]
        );
    }
    fn client() -> (Client, mpsc::Receiver<Value>) {
        let (sender, receiver) = mpsc::channel(64);
        (
            Client {
                sender,
                subscribed: HashSet::new(),
                cancel: CancellationToken::new(),
                attached: true,
            },
            receiver,
        )
    }
    #[test]
    fn ids_routes_and_approvals_stay_on_their_original_client() {
        let mut router = Router::new("instance".into());
        let (client, mut replies) = client();
        router.clients.insert("phone".into(), client);
        let desktop = router
            .outgoing(
                None,
                json!({"id":7,"method":"thread/resume","params":{"threadId":"same"}}),
                None,
            )
            .unwrap();
        let result = router
            .incoming(json!({"id":desktop["id"],"result":{"thread":{"id":"same"}}}))
            .unwrap();
        assert_eq!(result["id"], 7);
        let phone=router.outgoing(Some("phone"),json!({"id":7,"method":"thread/resume","params":{"threadId":"same","excludeTurns":true}}),None).unwrap();
        assert_ne!(desktop["id"], phone["id"]);
        assert!(router
            .incoming(json!({"id":phone["id"],"result":{"thread":{"id":"same"}}}))
            .is_none());
        assert_eq!(replies.try_recv().unwrap()["id"], 7);
        let desktop_read = router
            .outgoing(
                None,
                json!({"id":"same-rpc","method":"thread/read","params":{"threadId":"same"}}),
                None,
            )
            .unwrap();
        let phone_read = router
            .outgoing(
                Some("phone"),
                json!({"id":"same-rpc","method":"thread/read","params":{"threadId":"same"}}),
                None,
            )
            .unwrap();
        assert_ne!(desktop_read["id"], phone_read["id"]);
        // 两端同 ID 请求同时在途，乱序回执仍保持原调用方与 ID 类型。
        assert!(router
            .incoming(json!({"id":phone_read["id"],"result":{"owner":"phone"}}))
            .is_none());
        assert_eq!(
            replies.try_recv().unwrap(),
            json!({"id":"same-rpc","result":{"owner":"phone"}})
        );
        assert_eq!(
            router
                .incoming(json!({"id":desktop_read["id"],"result":{"owner":"desktop"}}))
                .unwrap(),
            json!({"id":"same-rpc","result":{"owner":"desktop"}})
        );
        let approval = json!({"id":phone["id"],"method":"item/commandExecution/requestApproval","params":{"threadId":"same"}});
        assert_eq!(router.incoming(approval.clone()), Some(approval.clone()));
        assert_eq!(replies.try_recv().unwrap(), approval);
        assert!(router
            .outgoing(
                Some("phone"),
                json!({"id":phone["id"],"result":{"decision":"accept"}}),
                None
            )
            .is_some());
        assert!(router
            .outgoing(
                None,
                json!({"id":phone["id"],"result":{"decision":"decline"}}),
                None
            )
            .is_none());
    }
    #[test]
    fn pending_approval_is_restored_for_the_new_subscription_only() {
        let mut router = Router::new("instance".into());
        router.desktop_threads.insert("same".into());
        let approval = json!({"id":19,"method":"item/commandExecution/requestApproval","params":{"threadId":"same","turnId":"turn","command":"echo test"}});
        router.incoming(approval.clone());
        for id in ["old", "new"] {
            let (peer, mut replies) = client();
            router.clients.insert(id.into(), peer);
            let resume = router
                .outgoing(
                    Some(id),
                    json!({"id":1,"method":"thread/resume","params":{"threadId":"same"}}),
                    None,
                )
                .unwrap();
            router.incoming(json!({"id":resume["id"],"result":{"thread":{"id":"same"}}}));
            assert_eq!(replies.try_recv().unwrap(), approval);
            assert_eq!(replies.try_recv().unwrap()["id"], 1);
            if id == "old" {
                router.disconnect(id);
            }
        }
        assert!(router
            .outgoing(
                Some("old"),
                json!({"id":19,"result":{"decision":"accept"}}),
                None
            )
            .is_none());
        assert!(router
            .outgoing(
                Some("new"),
                json!({"id":19,"result":{"decision":"decline"}}),
                None
            )
            .is_some());
        assert!(router
            .outgoing(None, json!({"id":19,"result":{"decision":"accept"}}), None)
            .is_none());
        router.incoming(approval);
        router.incoming(json!({"method":"turn/completed","params":{"threadId":"same","turn":{"id":"turn","status":"interrupted"}}}));
        assert!(router.server_requests.is_empty());
    }

    #[test]
    fn mobile_disconnect_or_unsubscribe_never_unloads_desktop_or_replays() {
        let mut router = Router::new("instance".into());
        let (client, mut replies) = client();
        router.clients.insert("phone".into(), client);
        router.desktop_threads.insert("same".into());
        router
            .clients
            .get_mut("phone")
            .unwrap()
            .subscribed
            .insert("same".into());
        let approval = json!({"id":19,"method":"item/commandExecution/requestApproval","params":{"threadId":"same"}});
        router.incoming(approval.clone());
        assert_eq!(replies.try_recv().unwrap(), approval);
        assert!(router.outgoing(Some("phone"),json!({"id":"unsubscribe","method":"thread/unsubscribe","params":{"threadId":"same"}}),None).is_none());
        assert_eq!(replies.try_recv().unwrap()["result"], json!({}));
        assert!(router.desktop_threads.contains("same"));
        assert!(router
            .outgoing(
                Some("phone"),
                json!({"id":19,"result":{"decision":"accept"}}),
                None
            )
            .is_none());
        assert!(router
            .outgoing(None, json!({"id":19,"result":{"decision":"decline"}}), None)
            .is_some());
        let pending = router
            .outgoing(
                Some("phone"),
                json!({"id":1,"method":"turn/start","params":{"threadId":"same"}}),
                None,
            )
            .unwrap();
        router.disconnect("phone");
        assert!(router
            .incoming(json!({"id":pending["id"],"result":{"turn":{"id":"accepted"}}}))
            .is_none());
        assert!(router.pending.is_empty());
    }
    #[test]
    fn history_visibility_cannot_create_resume_or_change_an_unobserved_route() {
        let mut router = Router::new("instance".into());
        let (client, mut replies) = client();
        router.clients.insert("phone".into(), client);
        for method in [
            "thread/start",
            "thread/fork",
            "thread/resume",
            "turn/start",
            "turn/addUserMessage",
        ] {
            assert!(router
                .outgoing(
                    Some("phone"),
                    json!({"id":1,"method":method,"params":{"threadId":"durable"}}),
                    None
                )
                .is_none());
            assert_eq!(replies.try_recv().unwrap()["error"]["code"], -32600);
        }
        assert!(router.pending.is_empty());
        let message = json!({"id":2,"method":"turn/addUserMessage","params":{"threadId":"durable","source":"original","input":[{"type":"text","text":"原文"}]}});
        let forwarded = router.outgoing(None, message.clone(), None).unwrap();
        assert_eq!(forwarded["method"], message["method"]);
        assert_eq!(forwarded["params"], message["params"]);
    }

    #[test]
    fn unresolved_approvals_cannot_cross_attach_resolution_or_connection_lifetimes() {
        let mut router = Router::new("instance".into());
        let (mut peer, mut replies) = client();
        peer.attached = false;
        router.clients.insert("phone".into(), peer);
        let approval = json!({"id":19,"method":"account/requestApproval","params":{}});
        router.incoming(approval.clone());
        assert!(replies.try_recv().is_err());
        let reply = json!({"id":19,"result":{"decision":"accept"}});
        assert!(router
            .outgoing(Some("phone"), reply.clone(), None)
            .is_none());
        router.outgoing(
            Some("phone"),
            json!({"id":"attach","method":"codexMobile/attach"}),
            Some(&json!({})),
        );
        assert_eq!(
            replies.try_recv().unwrap()["result"],
            json!({"codexMobileNativeImages":true})
        );
        // 完成 attach 不授予此前没有收到的服务请求回复权。
        assert!(router
            .outgoing(Some("phone"), reply.clone(), None)
            .is_none());

        router.incoming(approval.clone());
        assert_eq!(replies.try_recv().unwrap(), approval);
        router.incoming(json!({"method":"serverRequest/resolved","params":{"requestId":19}}));
        assert!(router
            .outgoing(Some("phone"), reply.clone(), None)
            .is_none());
        assert!(router.outgoing(None, reply.clone(), None).is_none());
        router.incoming(approval.clone());
        router.disconnect("phone");
        let (peer, _) = client();
        router.clients.insert("reconnected".into(), peer);
        assert!(router
            .outgoing(Some("reconnected"), reply.clone(), None)
            .is_none());
        assert_eq!(router.outgoing(None, reply, None).unwrap()["id"], 19);
    }

    #[tokio::test]
    async fn authenticated_wire_uses_one_stdio_initialize_and_keeps_desktop_subscription() {
        authenticated_wire_handshake(false).await;
        authenticated_wire_handshake(true).await;
    }

    async fn authenticated_wire_handshake(without_notification: bool) {
        use crate::{app_server::AppServerConnection, process_identity::ProcessIdentity};
        use std::sync::{Arc, Mutex};
        let temp = tempfile::tempdir().unwrap();
        let home = std::fs::canonicalize(temp.path()).unwrap();
        let process = ProcessIdentity::current().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let token = crate::auth::random_token();
        let mut record = RuntimeInfo {
            format: 2,
            instance_id: uuid::Uuid::new_v4().to_string(),
            host: "local".into(),
            contract: crate::runtime::CONTRACT.into(),
            state: "starting".into(),
            url: endpoint.clone(),
            auth_token: token.clone(),
            desktop: process.clone(),
            server: process.clone(),
            monitor: process.clone(),
            codex_home: home.clone(),
            cli_path: process.executable,
        };
        let (desktop, broker_desktop) = tokio::io::duplex(131072);
        let (upstream, native) = tokio::io::duplex(131072);
        let (input, output) = tokio::io::split(broker_desktop);
        let first = "{ \"id\": 7, \"method\": \"initialize\", \"params\": {\"capabilities\":{\"explicitGatewayOauth\":true}} }";
        let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
        let observed = calls.clone();
        let native_task = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(native);
            let mut reader = BufReader::new(reader).lines();
            let initial = reader.next_line().await.unwrap().unwrap();
            assert_eq!(initial, first);
            observed
                .lock()
                .unwrap()
                .push(serde_json::from_str(&initial).unwrap());
            let agent = if without_notification {
                "Codex Desktop/0.171.0-alpha.13.2 (fixture)"
            } else {
                "Codex Desktop/0.171.0-alpha.13 (fixture)"
            };
            write_line(
                &mut writer,
                &json!({"id":7,"result":{"userAgent":agent,"codexHome":home}}),
            )
            .await
            .unwrap();
            while let Some(line) = reader.next_line().await.unwrap() {
                let value: Value = serde_json::from_str(&line).unwrap();
                observed.lock().unwrap().push(value.clone());
                if value.get("id").is_some() {
                    write_line(&mut writer,&json!({"id":value["id"],"result":{"thread":{"id":value["params"]["threadId"]}}})).await.unwrap();
                }
            }
        });
        let directory = temp.path().join("runtime");
        let task = tokio::spawn(async move {
            run(
                input,
                output,
                Box::new(upstream),
                listener,
                &mut record,
                directory,
                first,
            )
            .await
        });
        let (desktop_reader, mut desktop_writer) = tokio::io::split(desktop);
        let mut desktop_reader = BufReader::new(desktop_reader).lines();
        let initialized: Value =
            serde_json::from_str(&desktop_reader.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(initialized["id"], 7);
        assert!(
            AppServerConnection::connect_broker(&endpoint, &token)
                .await
                .err()
                .unwrap()
                .before_send
        );
        if !without_notification {
            write_line(
                &mut desktop_writer,
                &json!({"method":"initialized","params":{}}),
            )
            .await
            .unwrap();
        }
        assert!(
            AppServerConnection::connect_broker(&endpoint, "wrong-token")
                .await
                .err()
                .unwrap()
                .before_send
        );
        write_line(
            &mut desktop_writer,
            &json!({"id":7,"method":"thread/resume","params":{"threadId":"chat"}}),
        )
        .await
        .unwrap();
        let resumed: Value =
            serde_json::from_str(&desktop_reader.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(resumed["id"], 7);
        let phone = AppServerConnection::connect_broker(&endpoint, &token)
            .await
            .unwrap();
        let mut expected = initialized["result"].clone();
        expected["codexMobileNativeImages"] = json!(true);
        assert_eq!(phone.initialized, expected);
        assert_eq!(
            phone
                .rpc("codexMobile/thread/route", json!({"threadId":"chat"}))
                .await
                .unwrap()["available"],
            true
        );
        assert_eq!(
            phone
                .rpc(
                    "thread/resume",
                    json!({"threadId":"chat","excludeTurns":true})
                )
                .await
                .unwrap()["thread"]["id"],
            "chat"
        );
        phone
            .rpc("thread/unsubscribe", json!({"threadId":"chat"}))
            .await
            .unwrap();
        assert_eq!(
            phone
                .rpc("codexMobile/thread/route", json!({"threadId":"chat"}))
                .await
                .unwrap()["available"],
            true
        );
        phone.close();
        desktop_writer.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        native_task.await.unwrap();
        let calls = calls.lock().unwrap();
        assert_eq!(
            calls
                .iter()
                .filter(|value| value["method"] == "initialize")
                .count(),
            1
        );
        assert_eq!(
            calls
                .iter()
                .filter(|value| value["method"] == "initialized")
                .count(),
            usize::from(!without_notification)
        );
        assert!(calls.iter().all(|value| !matches!(
            value["method"].as_str(),
            Some("codexMobile/attach" | "thread/start" | "thread/unsubscribe")
        )));
    }

    #[tokio::test]
    #[ignore = "需要明确指定官方 CLI，只在独立 CODEX_HOME 验证只读 stdio"]
    async fn official_stdio_readonly_canary() {
        use crate::{process_identity::ProcessIdentity, transport::StdioPipe};
        let cli = PathBuf::from(
            std::env::var_os("CODEX_MOBILE_TEST_OFFICIAL_CLI").expect("必须指定官方 CLI"),
        );
        assert!(cli.is_absolute() && cli.is_file());
        let artifacts = PathBuf::from(
            std::env::var_os("CODEX_MOBILE_TEST_ARTIFACTS").expect("必须指定验收产物目录"),
        );
        assert!(artifacts.is_absolute());
        std::fs::create_dir_all(&artifacts).unwrap();
        let home = tempfile::tempdir_in(&artifacts).unwrap();
        let codex_home = std::fs::canonicalize(home.path()).unwrap();
        let mut child = tokio::process::Command::new(&cli);
        child
            .args(["app-server"])
            .env("CODEX_HOME", &codex_home)
            .env_remove("CODEX_APP_SERVER_FORCE_CLI")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        child.creation_flags(0x08000000);
        let mut child = child.spawn().unwrap();
        let server = ProcessIdentity::read(child.id().unwrap()).unwrap();
        let stderr = child.stderr.take().unwrap();
        let stderr_task = tokio::spawn(async move {
            let mut stderr = stderr;
            let mut bytes = Vec::new();
            tokio::io::AsyncReadExt::read_to_end(&mut stderr, &mut bytes)
                .await
                .unwrap();
            bytes
        });
        let process = ProcessIdentity::current().unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut record = RuntimeInfo {
            format: 2,
            instance_id: uuid::Uuid::new_v4().to_string(),
            host: "local".into(),
            contract: crate::runtime::CONTRACT.into(),
            state: "starting".into(),
            url: format!("ws://{}", listener.local_addr().unwrap()),
            auth_token: crate::auth::random_token(),
            desktop: process.clone(),
            server,
            monitor: process,
            codex_home,
            cli_path: cli,
        };
        let upstream = StdioPipe {
            reader: child.stdout.take().unwrap(),
            writer: child.stdin.take().unwrap(),
        };
        let (desktop, broker_desktop) = tokio::io::duplex(131072);
        let (input, output) = tokio::io::split(broker_desktop);
        let directory = home.path().join("runtime");
        let endpoint = record.url.clone();
        let token = record.auth_token.clone();
        let first = json!({"id":17,"method":"initialize","params":{"clientInfo":{"name":"stdio-readonly-canary","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true,"explicitGatewayOauth":true}}}).to_string();
        let task = tokio::spawn(async move {
            run(
                input,
                output,
                Box::new(upstream),
                listener,
                &mut record,
                directory,
                &first,
            )
            .await
        });
        let (reader, mut writer) = tokio::io::split(desktop);
        let mut reader = BufReader::new(reader).lines();
        let initialized: Value = serde_json::from_str(
            &tokio::time::timeout(Duration::from_secs(15), reader.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(initialized["id"], 17);
        assert!(
            initialized["result"]["userAgent"].is_string(),
            "{initialized}"
        );
        write_line(&mut writer, &json!({"method":"initialized","params":{}}))
            .await
            .unwrap();
        write_line(
            &mut writer,
            &json!({"id":19,"method":"thread/loaded/list","params":{}}),
        )
        .await
        .unwrap();
        let mut notifications = Vec::new();
        let loaded = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let message: Value =
                    serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap();
                if message["id"] == 19 {
                    break message;
                }
                assert!(message["method"].is_string(), "非目标回执或通知：{message}");
                notifications.push(message);
            }
        })
        .await
        .unwrap();
        assert_eq!(loaded["id"], 19);
        assert_eq!(loaded["result"]["data"], json!([]));
        let phone = crate::app_server::AppServerConnection::connect_broker(&endpoint, &token)
            .await
            .unwrap();
        assert_eq!(phone.initialized, initialized["result"]);
        let phone_loaded = phone.rpc("thread/loaded/list", json!({})).await.unwrap();
        assert_eq!(phone_loaded["data"], json!([]));
        let unknown_route = phone
            .rpc("codexMobile/thread/route", json!({"threadId":"unknown"}))
            .await
            .unwrap();
        assert_eq!(unknown_route["available"], false);
        phone.close();
        writer.shutdown().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success());
        let stderr = stderr_task.await.unwrap();
        std::fs::write(artifacts.join("native-stdio-stderr.log"), stderr).unwrap();
        std::fs::write(artifacts.join("native-stdio-result.json"),serde_json::to_vec_pretty(&json!({"test":"隔离官方 CLI 单实例双连接只读 stdio，不是完整桌面控制验收","initialized":initialized,"loaded":loaded,"phoneLoaded":phone_loaded,"unknownRoute":unknown_route,"notifications":notifications,"officialInitializeCount":1,"createdChats":0,"promptsSent":0,"normalEofExit":true})).unwrap()).unwrap();
    }
}
