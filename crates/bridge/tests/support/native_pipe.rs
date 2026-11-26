use crate::{app_server::AppServerConnection, native::NativeOwner, paths::Paths};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Mutex, Notify, Semaphore},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

pub const CHAT: &str = "11111111-1111-4111-8111-111111111111";
pub const OTHER: &str = "22222222-2222-4222-8222-222222222222";
pub const CREATED: &str = "33333333-3333-4333-8333-333333333333";

/// 仅模拟官方管道另一端，Bridge/Session/Desktop/Operations均使用原实现。
pub struct NativePipe {
    pub calls: Mutex<Vec<Value>>,
    pub reads: Mutex<HashMap<String, Value>>,
    pub projects: Mutex<Value>,
    pub hold_reads: AtomicBool,
    pub release_reads: Semaphore,
    pub hold_sends: AtomicBool,
    pub release_sends: Semaphore,
    pub send_reply: Mutex<Option<Value>>,
    pub navigation_reply: Mutex<Value>,
    pub call_received: Notify,
    cancel: CancellationToken,
}

impl NativePipe {
    pub async fn start(paths: &Paths) -> (Arc<Self>, Arc<AppServerConnection>) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let peer = Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            reads: Mutex::new([CHAT, OTHER, CREATED].into_iter().map(|id| (id.into(), json!({
                "schemaVersion":1,"thread":{"id":id,"kind":"codex","hostId":"local",
                "title":"隔离原生聊天","projectId":"project","status":"idle"},"turns":[]
            }))).collect()),
            projects: Mutex::new(json!({"projects":[{"projectId":"project","projectKind":"local","hostId":"local","label":"隔离项目","path":"/fixture/project"}]})),
            hold_reads: AtomicBool::new(false), release_reads: Semaphore::new(0),
            hold_sends: AtomicBool::new(false), release_sends: Semaphore::new(0),
            send_reply: Mutex::new(None), call_received: Notify::new(),
            navigation_reply: Mutex::new(json!({"navigated":true})),
            cancel: CancellationToken::new(),
        });
        let weak = Arc::downgrade(&peer);
        let cancel = peer.cancel.clone();
        tokio::spawn(async move {
            let (mut input, output) = tokio::io::split(server);
            let output = Arc::new(Mutex::new(output));
            let mut replies = JoinSet::new();
            loop {
                let size = tokio::select! {
                    _ = cancel.cancelled() => break,
                    size = input.read_u32_le() => match size { Ok(size) => size, Err(_) => break },
                };
                let mut bytes = vec![0; size as usize];
                if input.read_exact(&mut bytes).await.is_err() {
                    break;
                }
                let request: Value = serde_json::from_slice(&bytes).unwrap();
                let Some(peer) = weak.upgrade() else {
                    break;
                };
                peer.calls.lock().await.push(request.clone());
                peer.call_received.notify_waiters();
                let output = output.clone();
                replies.spawn(async move {
                    let result = peer.respond(&request).await;
                    let bytes = serde_json::to_vec(
                        &json!({"jsonrpc":"2.0","id":request["id"],"result":result}),
                    )
                    .unwrap();
                    let mut output = output.lock().await;
                    if output.write_u32_le(bytes.len() as u32).await.is_ok() {
                        let _ = output.write_all(&bytes).await;
                    }
                });
            }
            replies.abort_all();
        });
        let process = crate::process_identity::ProcessIdentity::current().unwrap();
        let connection = AppServerConnection::connect_native_pipe(
            Box::new(client),
            NativeOwner {
                pipe: paths.runtime_home.join("synthetic-native-pipe"),
                desktop: process.clone(),
                server: process,
            },
        )
        .await
        .unwrap();
        (peer, connection)
    }

    async fn respond(&self, request: &Value) -> Value {
        if request["method"] == "tools/list" {
            let tools = [("read_thread", vec!["threadId"]), ("send_message_to_thread", vec!["threadId", "prompt"]),
                ("create_thread", vec!["prompt", "target"]), ("list_projects", vec![]), ("list_threads", vec![]),
                ("navigate_to_codex_page", vec!["threadId"])]
                .map(|(name, required)| {
                    let mut properties = required.iter().map(|key| ((*key).into(), json!({"type":"string"}))).collect::<serde_json::Map<_,_>>();
                    if matches!(name, "create_thread" | "send_message_to_thread") {
                        properties.insert("model".into(), json!({"type":"string"}));
                        properties.insert("thinking".into(), json!({"type":"string","enum":["low","high"]}));
                    }
                    json!({"name":name,"namespace":"codex_app","inputSchema":{"type":"object","additionalProperties":false,"required":required,"properties":properties}})
                });
            return json!({"tools":tools});
        }
        assert_eq!(request["method"], "tools/call");
        let args = &request["params"]["arguments"];
        let payload = match request["params"]["tool"].as_str().unwrap() {
            "read_thread" => {
                if self.hold_reads.load(Ordering::Acquire) {
                    self.release_reads.acquire().await.unwrap().forget();
                }
                self.reads
                    .lock()
                    .await
                    .get(args["threadId"].as_str().unwrap())
                    .cloned()
                    .unwrap_or(Value::Null)
            }
            "list_projects" => self.projects.lock().await.clone(),
            "list_threads" => json!({"threads":[],"nextCursor":null}),
            "create_thread" => json!({"threadId":CREATED,"hostId":"local"}),
            "send_message_to_thread" => {
                if self.hold_sends.load(Ordering::Acquire) {
                    self.release_sends.acquire().await.unwrap().forget();
                }
                if let Some(reply) = self.send_reply.lock().await.clone() {
                    return reply;
                }
                json!({"threadId":args["threadId"],"status":"accepted"})
            }
            "navigate_to_codex_page" => self.navigation_reply.lock().await.clone(),
            name => panic!("未预期的合成工具调用：{name}"),
        };
        json!({"success":true,"contentItems":[{"type":"inputText","text":payload.to_string()}]})
    }

    pub async fn count(&self, tool: &str) -> usize {
        self.calls
            .lock()
            .await
            .iter()
            .filter(|call| call["params"]["tool"] == tool)
            .count()
    }

    pub async fn wait_calls(&self, tool: &str, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                let notified = self.call_received.notified();
                if self.count(tool).await >= expected {
                    return;
                }
                notified.await;
            }
        })
        .await
        .unwrap();
    }
}

impl Drop for NativePipe {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
