#![allow(dead_code)]

pub mod process;

use codex_mobile_bridge::{paths::Paths, transport::Pipe};
use futures_util::{SinkExt, StreamExt};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::{broadcast, Mutex},
    task::JoinHandle,
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

pub struct Fixture {
    pub temp: tempfile::TempDir,
    pub paths: Paths,
}
impl Fixture {
    pub fn runtime(&self, endpoint: &str) -> codex_mobile_bridge::runtime::RuntimeInfo {
        let process = codex_mobile_bridge::process_identity::ProcessIdentity::current().unwrap();
        let runtime = codex_mobile_bridge::runtime::RuntimeInfo {
            format: 2,
            instance_id: uuid::Uuid::new_v4().to_string(),
            host: "local".into(),
            contract: codex_mobile_bridge::runtime::CONTRACT.into(),
            state: "ready".into(),
            url: endpoint.into(),
            auth_token: codex_mobile_bridge::auth::random_token(),
            desktop: process.clone(),
            server: process.clone(),
            monitor: process.clone(),
            codex_home: std::fs::canonicalize(&self.paths.codex_home).unwrap(),
            cli_path: process.executable,
        };
        runtime.publish(&self.paths.runtime_home).unwrap();
        runtime
    }
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let paths = Paths {
            codex_home: root.join("codex"),
            data_home: root.join("data"),
            runtime_home: root.join("runtime"),
            web_root: root.join("web"),
        };
        for path in [
            &paths.codex_home,
            &paths.data_home,
            &paths.runtime_home,
            &paths.web_root,
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(
            paths.web_root.join("index.html"),
            "<!doctype html><title>Isolated fixture</title>",
        )
        .unwrap();
        std::fs::write(
            paths.codex_home.join("config.toml"),
            "model = 'desktop-default'\nmodel_reasoning_effort = 'high'\n",
        )
        .unwrap();
        std::fs::write(paths.codex_home.join(".codex-global-state.json"), json!({"unrelated":"preserved","queued-follow-ups":{"chat":[],"other":[{"id":"untouched"}]}}).to_string()).unwrap();
        let state = Connection::open(paths.codex_home.join("state_5.sqlite")).unwrap();
        state.execute_batch("CREATE TABLE threads (id TEXT PRIMARY KEY, name TEXT, title TEXT, cwd TEXT, model TEXT, reasoning_effort TEXT, rollout_path TEXT, history_mode TEXT, project_id TEXT, recency_at_ms INTEGER, updated_at_ms INTEGER, archived INTEGER, source TEXT, thread_source TEXT, tokens_used INTEGER, attached_thread_parent_id TEXT);").unwrap();
        state.execute_batch("CREATE TABLE projects (id TEXT PRIMARY KEY,name TEXT,metadata TEXT,position INTEGER,created_at_ms INTEGER,updated_at_ms INTEGER); CREATE TABLE project_roots (project_id TEXT,position INTEGER,path TEXT); INSERT INTO projects VALUES ('project','样例项目','{}',0,100,100); INSERT INTO project_roots VALUES ('project',0,'/fixture/project');").unwrap();
        for id in ["chat", "other"] {
            state.execute("INSERT INTO threads VALUES (?,NULL,?,?,'desktop-default','high',NULL,'paginated',NULL,100,100,0,'vscode','user',0,NULL)",params![id,format!("样例 {id}"),root.to_string_lossy()]).unwrap();
        }
        let history = Connection::open(paths.codex_home.join("thread_history_1.sqlite")).unwrap();
        history.execute_batch("CREATE TABLE thread_turns (thread_id TEXT,turn_id TEXT,rollout_ordinal INTEGER,status TEXT,error_json TEXT,started_at INTEGER,completed_at INTEGER); CREATE TABLE thread_items (thread_id TEXT,turn_id TEXT,item_id TEXT,rollout_ordinal INTEGER,item_json TEXT,item_type TEXT,updated_at_ordinal INTEGER);").unwrap();
        for n in 1..=3 {
            let turn = format!("turn-{n}");
            history
                .execute(
                    "INSERT INTO thread_turns VALUES ('chat',?,?,'completed',NULL,?,?)",
                    params![turn, n * 10, n, n],
                )
                .unwrap();
            let item = json!({"id":format!("command-{n}"),"type":"commandExecution","status":"completed","aggregatedOutput":"完整输出🙂\n".repeat(1000)});
            history
                .execute(
                    "INSERT INTO thread_items VALUES ('chat',?,?,?,?, 'commandExecution',?)",
                    params![
                        turn,
                        format!("command-{n}"),
                        n * 10 + 1,
                        item.to_string(),
                        n * 10 + 2
                    ],
                )
                .unwrap();
        }
        Self { temp, paths }
    }
    pub fn queue(&self) -> Value {
        serde_json::from_slice(
            &std::fs::read(self.paths.codex_home.join(".codex-global-state.json")).unwrap(),
        )
        .unwrap()
    }
}

pub struct MockDesktop {
    pub calls: Arc<Mutex<Vec<Value>>>,
    runtime: codex_mobile_bridge::runtime::RuntimeInfo,
    pub threads: Mutex<HashMap<String, Value>>,
    pub loaded_threads: Mutex<HashSet<String>>,
    pub native_threads: Mutex<Vec<Value>>,
    pub defer_created_catalog: AtomicBool,
    pub empty_created_rollout: AtomicBool,
    pub empty_created_resume_once: AtomicBool,
    pub projects: Mutex<Vec<Value>>,
    pub browser_result: Mutex<Value>,
    pub queues: Mutex<HashMap<String, Vec<Value>>>,
    pub pending_turn_threads: Mutex<HashSet<String>>,
    pub reject_queue_delete: AtomicBool,
    pub reject_queue_list: AtomicBool,
    pub reject_init: AtomicBool,
    pub user_agent: Mutex<String>,
    pub missing_desktop_tools: AtomicBool,
    pub wrong_home: AtomicBool,
    pub fail_next: AtomicBool,
    pub drop_next: AtomicBool,
    pub delay_ms: AtomicU64,
    pub connections: AtomicU64,
    pub broadcasts: broadcast::Sender<Value>,
    pub pending_requests: Mutex<HashMap<String, Value>>,
    codex_home: PathBuf,
    other_home: PathBuf,
    next_turn: AtomicU64,
    cancel: CancellationToken,
    tasks: std::sync::Mutex<Vec<JoinHandle<()>>>,
}
impl MockDesktop {
    pub async fn start(fixture: &Fixture) -> Arc<Self> {
        Self::start_mode(fixture, false).await
    }
    pub async fn start_control_fixture(fixture: &Fixture) -> Arc<Self> {
        Self::start_mode(fixture, true).await
    }
    async fn start_mode(fixture: &Fixture, authenticated: bool) -> Arc<Self> {
        let (broadcasts, _) = broadcast::channel(2048);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let other_home = fixture.temp.path().join("other-codex");
        std::fs::create_dir_all(&other_home).unwrap();
        let runtime = fixture.runtime(&endpoint);
        let token = runtime.auth_token.clone();
        let mock = Arc::new(Self {
            runtime,
            calls: Arc::new(Mutex::new(vec![])),
            threads: Mutex::new(
                ["chat", "other"]
                    .into_iter()
                    .map(|id| (id.to_owned(), official_thread(id, false)))
                    .collect(),
            ),
            loaded_threads: Mutex::new(HashSet::new()),
            native_threads: Mutex::new(vec![
                native_thread("chat", json!("project")),
                native_thread("other", Value::Null),
            ]),
            defer_created_catalog: AtomicBool::new(false),
            empty_created_rollout: AtomicBool::new(false),
            empty_created_resume_once: AtomicBool::new(false),
            projects: Mutex::new(vec![
                json!({"projectId":"project","projectKind":"local","hostId":"local","label":"样例项目","path":"/fixture/project","isGitRepository":false}),
            ]),
            browser_result: Mutex::new(
                json!({"content":[{"type":"text","text":"Public CUA documentation"},{"type":"text","text":json!({"apps":[],"browsers":[]}).to_string()}],"isError":false}),
            ),
            queues: Mutex::new(HashMap::new()),
            pending_turn_threads: Mutex::new(HashSet::new()),
            reject_queue_delete: AtomicBool::new(false),
            reject_queue_list: AtomicBool::new(false),
            reject_init: AtomicBool::new(false),
            user_agent: Mutex::new("Codex Desktop/0.171.0-alpha.13 (fixture)".into()),
            missing_desktop_tools: AtomicBool::new(false),
            wrong_home: AtomicBool::new(false),
            fail_next: AtomicBool::new(false),
            drop_next: AtomicBool::new(false),
            delay_ms: AtomicU64::new(0),
            connections: AtomicU64::new(0),
            broadcasts,
            pending_requests: Mutex::new(HashMap::new()),
            codex_home: fixture.paths.codex_home.clone(),
            other_home,
            next_turn: AtomicU64::new(1),
            cancel: CancellationToken::new(),
            tasks: std::sync::Mutex::new(vec![]),
        });
        let weak = Arc::downgrade(&mock);
        let cancel = mock.cancel.clone();
        let task = tokio::spawn(async move {
            loop {
                let accepted = tokio::select! {_=cancel.cancelled()=>break,accepted=listener.accept()=>accepted};
                let Ok((socket, _)) = accepted else {
                    break;
                };
                let child_mock = weak.clone();
                let cancel = cancel.clone();
                let expected = format!("Bearer {token}");
                let task = tokio::spawn(async move {
                    let Ok(socket) = tokio_tungstenite::accept_hdr_async(
                        socket,
                        |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                         response| {
                            if authenticated
                                && request
                                    .headers()
                                    .get("authorization")
                                    .and_then(|value| value.to_str().ok())
                                    != Some(expected.as_str())
                            {
                                return Err(
                                    tokio_tungstenite::tungstenite::http::Response::builder()
                                        .status(401)
                                        .body(Some("fixture authentication required".into()))
                                        .unwrap(),
                                );
                            }
                            Ok(response)
                        },
                    )
                    .await
                    else {
                        return;
                    };
                    let Some(mock) = child_mock.upgrade() else {
                        return;
                    };
                    mock.connections.fetch_add(1, Ordering::AcqRel);
                    let events = mock.broadcasts.subscribe();
                    drop(mock);
                    serve(socket, child_mock, events, cancel).await;
                });
                if let Some(mock) = weak.upgrade() {
                    mock.tasks.lock().unwrap().push(task);
                }
            }
        });
        mock.tasks.lock().unwrap().push(task);
        mock
    }
    pub async fn count(&self, method: &str) -> usize {
        self.calls
            .lock()
            .await
            .iter()
            .filter(|c| c["method"] == method)
            .count()
    }
    pub async fn server_request(&self, request: Value) {
        self.pending_requests
            .lock()
            .await
            .insert(request["id"].to_string(), request.clone());
        self.broadcasts.send(request).unwrap();
    }
    pub fn disconnect(&self) {
        let _ = self.broadcasts.send(Value::Null);
    }
    pub async fn set_thread(&self, id: &str, state: Value) {
        self.threads.lock().await.insert(id.to_owned(), state);
    }
    async fn respond(&self, value: &Value) -> Result<Option<Value>, String> {
        let params = &value["params"];
        let method = value["method"].as_str().unwrap_or("");
        if method.is_empty() {
            if let Some(request) = self
                .pending_requests
                .lock()
                .await
                .remove(&value["id"].to_string())
            {
                let _ = self.broadcasts.send(json!({"method":"serverRequest/resolved","params":{"threadId":request["params"]["threadId"],"requestId":request["id"]}}));
            }
            return Ok(None);
        }
        if method == "initialized" {
            return Ok(None);
        }
        if method == "thread/goal/get" {
            return Ok(Some(json!({"goal":null})));
        }
        if method == "thread/unsubscribe" {
            return Ok(Some(json!({"status":"unsubscribed"})));
        }
        if method == "initialize" || method == "codexMobile/attach" {
            if self.reject_init.load(Ordering::Acquire) {
                return Err("fixture initialize rejected".into());
            }
            assert_eq!(params["clientInfo"]["name"], "codex-mobile");
            assert_eq!(params["capabilities"]["experimentalApi"], true);
            assert_eq!(
                params["capabilities"]["mcpServerOpenaiFormElicitation"],
                true
            );
            return Ok(Some(
                json!({"userAgent":*self.user_agent.lock().await,"codexHome":if self.wrong_home.load(Ordering::Acquire){&self.other_home}else{&self.codex_home}}),
            ));
        }
        let id = params["threadId"].as_str().unwrap_or("chat");
        if method == "codexMobile/thread/route" {
            return Ok(Some(
                json!({"threadId":id,"instanceId":self.runtime.instance_id,
                "available":self.threads.lock().await.contains_key(id) && uuid::Uuid::parse_str(id).is_ok()}),
            ));
        }
        let result = match method {
            "thread/loaded/list" => {
                json!({"data":self.loaded_threads.lock().await.iter().collect::<Vec<_>>(),"nextCursor":null})
            }
            "thread/resume" => {
                if id == "created" && self.empty_created_resume_once.swap(false, Ordering::AcqRel) {
                    return Err(
                        "failed to read thread: rollout at /fixture/pending.jsonl is empty".into(),
                    );
                }
                let mut state = self
                    .threads
                    .lock()
                    .await
                    .get(id)
                    .cloned()
                    .ok_or_else(|| "thread not found".to_owned())?;
                self.loaded_threads.lock().await.insert(id.to_owned());
                if params.get("initialTurnsPage").is_some() {
                    let turns = state["thread"]["turns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .rev()
                        .take(5)
                        .cloned()
                        .collect::<Vec<_>>();
                    state["initialTurnsPage"] =
                        json!({"data":turns,"nextCursor":null,"backwardsCursor":null});
                }
                if params["excludeTurns"] == true {
                    state["thread"]["turns"] = json!([]);
                }
                state
            }
            "thread/read" => {
                if id == "created" && self.empty_created_rollout.load(Ordering::Acquire) {
                    return Err("failed to read thread: rollout is empty".into());
                }
                json!({"thread":self.threads.lock().await.get(id).ok_or_else(||"thread not found".to_owned())?["thread"]})
            }
            "thread/start" => {
                if params["historyMode"] == "legacy" {
                    let id = uuid::Uuid::new_v4().to_string();
                    let mut state = official_thread(&id, false);
                    state["thread"]["cwd"] = params["cwd"].clone();
                    state["cwd"] = params["cwd"].clone();
                    state["thread"]["turns"] = json!([]);
                    if let Some(model) = params.get("model") {
                        state["model"] = model.clone();
                    }
                    self.threads.lock().await.insert(id, state.clone());
                    return Ok(Some(state));
                }
                assert_eq!(params["ephemeral"], true);
                assert_eq!(params["threadSource"], "codex-mobile-control");
                let id = "fixture-control";
                let mut state = official_thread(id, false);
                state["thread"]["turns"] = json!([]);
                self.threads.lock().await.insert(id.into(), state.clone());
                state
            }
            "mcpServerStatus/list" => {
                if self.missing_desktop_tools.load(Ordering::Acquire) {
                    json!({"data":[]})
                } else {
                    json!({"data":[{"name":"codex_app","authStatus":"notLoggedIn","tools":{"list_projects":{},"list_threads":{}}}]})
                }
            }
            "mcpServer/tool/call" => {
                let args = &params["arguments"];
                if params["server"] == "cua_repl" {
                    assert_eq!(params["tool"], "js");
                    let thread = params["threadId"].as_str().unwrap();
                    assert!(self.threads.lock().await.contains_key(thread));
                    if !self.loaded_threads.lock().await.contains(thread) {
                        return Err("thread is not loaded".into());
                    }
                    assert_eq!(args["code"], "await cua.getState();");
                    assert!(args.get("completion_summary").is_none());
                    return Ok(Some(self.browser_result.lock().await.clone()));
                }
                assert_eq!(params["server"], "codex_app");
                let payload = match params["tool"].as_str().unwrap_or("") {
                    "list_projects" => {
                        json!({"schemaVersion":2,"projects":*self.projects.lock().await})
                    }
                    "list_threads" => {
                        let rows = self.native_threads.lock().await.clone();
                        let limit = args["limit"].as_u64().unwrap_or(50) as usize;
                        if args["mode"] == "history" {
                            assert_eq!(args["source"], "codex");
                            assert_eq!(args["hostId"], "local");
                            assert_eq!(args["archived"], false);
                            let rows = rows
                                .into_iter()
                                .filter(|row| {
                                    row["kind"] == "codex"
                                        && row["hostId"] == "local"
                                        && row["archived"] != true
                                })
                                .collect::<Vec<_>>();
                            let offset = args["cursor"]
                                .as_str()
                                .map(|cursor| {
                                    cursor
                                        .strip_prefix("native:")
                                        .unwrap()
                                        .parse::<usize>()
                                        .unwrap()
                                })
                                .unwrap_or(0);
                            let end = (offset + limit).min(rows.len());
                            json!({"threads":rows[offset..end],"nextCursor":(end<rows.len()).then(||format!("native:{end}"))})
                        } else {
                            json!({"pinnedThreads":rows.iter().filter(|row|row.get("pinnedIndex").is_some()).cloned().collect::<Vec<_>>(),
                                "threads":rows.into_iter().filter(|row|row.get("pinnedIndex").is_none()).take(limit).collect::<Vec<_>>()})
                        }
                    }
                    "create_thread" => {
                        let mut state = official_thread("created", false);
                        state["thread"]["turns"] = json!([]);
                        self.threads.lock().await.insert("created".into(), state);
                        let mut row = native_thread("created", args["target"]["projectId"].clone());
                        row["title"] = json!("原生创建的聊天");
                        if !self.defer_created_catalog.load(Ordering::Acquire) {
                            self.native_threads.lock().await.push(row);
                        }
                        json!({"threadId":"created"})
                    }
                    "navigate_to_codex_page" => json!({"navigated":true}),
                    "move_thread_to_project" => {
                        for row in self
                            .native_threads
                            .lock()
                            .await
                            .iter_mut()
                            .filter(|row| row["id"] == args["threadId"])
                        {
                            row["projectId"] = args["projectId"].clone();
                        }
                        json!({"moved":true})
                    }
                    _ => return Err("unsupported native tool".into()),
                };
                json!({"content":[{"type":"text","text":payload.to_string()}],"isError":false})
            }
            "thread/settings/update" => {
                let mut threads = self.threads.lock().await;
                let state = threads
                    .get_mut(id)
                    .ok_or_else(|| "thread not found".to_owned())?;
                for field in [
                    "model",
                    "cwd",
                    "approvalPolicy",
                    "approvalsReviewer",
                    "collaborationMode",
                ] {
                    if let Some(value) = params.get(field) {
                        state[field] = value.clone();
                    }
                }
                if let Some(value) = params.get("effort") {
                    state["reasoningEffort"] = value.clone();
                }
                if let Some(profile) = params["permissions"].as_str() {
                    state["activePermissionProfile"] = json!({"id":profile});
                    state["sandbox"] = json!({"type":match profile{":read-only"=>"readOnly",":workspace"=>"workspaceWrite",_=>"dangerFullAccess"}});
                }
                let mut settings = state.clone();
                settings["effort"] = state["reasoningEffort"].clone();
                settings["sandboxPolicy"] = state["sandbox"].clone();
                let _=self.broadcasts.send(json!({"method":"thread/settings/updated","params":{"threadId":id,"threadSettings":settings}}));
                json!({})
            }
            "turn/start" => self.start_turn(id, params).await?,
            "turn/steer" => {
                let mut threads = self.threads.lock().await;
                let state = threads
                    .get_mut(id)
                    .ok_or_else(|| "thread not found".to_owned())?;
                let turn = state["thread"]["turns"]
                    .as_array_mut()
                    .unwrap()
                    .last_mut()
                    .unwrap();
                if turn["id"] != params["expectedTurnId"] || turn["status"] != "inProgress" {
                    return Err("turn is not active".into());
                }
                let item = json!({"id":params["clientUserMessageId"],"clientId":params["clientUserMessageId"],"type":"userMessage","content":params["input"]});
                turn["items"].as_array_mut().unwrap().push(item.clone());
                let _=self.broadcasts.send(json!({"method":"item/started","params":{"threadId":id,"turnId":turn["id"],"item":item}}));
                json!({"turnId":turn["id"]})
            }
            "turn/interrupt" => {
                let mut threads = self.threads.lock().await;
                let state = threads
                    .get_mut(id)
                    .ok_or_else(|| "thread not found".to_owned())?;
                let turn = state["thread"]["turns"]
                    .as_array_mut()
                    .unwrap()
                    .last_mut()
                    .unwrap();
                turn["status"] = json!("interrupted");
                let turn = turn.clone();
                state["thread"]["status"] = json!({"type":"idle"});
                let _ = self
                    .broadcasts
                    .send(json!({"method":"turn/completed","params":{"threadId":id,"turn":turn}}));
                json!({})
            }
            "thread/queue/list" => {
                if self.reject_queue_list.load(Ordering::Acquire) {
                    return Err("fixture queue list rejected".into());
                }
                json!({"data":self.queues.lock().await.get(id).cloned().unwrap_or_default(),"nextCursor":null})
            }
            "thread/queue/add" => {
                let row = json!({"id":format!("queued-{}",params["clientUserMessageId"].as_str().unwrap()),"clientUserMessageId":params["clientUserMessageId"],"input":params["input"]});
                self.queues
                    .lock()
                    .await
                    .entry(id.into())
                    .or_default()
                    .push(row.clone());
                self.queue_changed(id);
                json!({"queuedSubmission":row})
            }
            "thread/queue/update" => {
                let mut queues = self.queues.lock().await;
                let row = queues
                    .entry(id.into())
                    .or_default()
                    .iter_mut()
                    .find(|row| row["id"] == params["queuedSubmissionId"])
                    .ok_or_else(|| "queued submission not found".to_owned())?;
                row["input"] = params["input"].clone();
                let row = row.clone();
                self.queue_changed(id);
                json!({"queuedSubmission":row})
            }
            "thread/queue/delete" => {
                if self.reject_queue_delete.swap(false, Ordering::AcqRel) {
                    return Err("fixture queue delete rejected".into());
                }
                let mut queues = self.queues.lock().await;
                let rows = queues.entry(id.into()).or_default();
                let deleted = rows
                    .iter()
                    .position(|row| row["id"] == params["queuedSubmissionId"])
                    .map(|index| rows.remove(index))
                    .is_some();
                if deleted {
                    self.queue_changed(id);
                }
                json!({"deleted":deleted})
            }
            "thread/queue/start" => {
                let threads = self.threads.lock().await;
                let state = threads
                    .get(id)
                    .ok_or_else(|| "thread not found".to_owned())?;
                if state["thread"]["status"]["type"] == "active"
                    || state["thread"]["turns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|turn| turn["status"] == "inProgress")
                    || self.pending_turn_threads.lock().await.contains(id)
                {
                    return Err("thread already has an active or pending turn".into());
                }
                drop(threads);
                let row = {
                    let mut queues = self.queues.lock().await;
                    let rows = queues.entry(id.into()).or_default();
                    let index = rows
                        .iter()
                        .position(|row| row["id"] == params["queuedSubmissionId"])
                        .ok_or_else(|| "queued submission not found".to_owned())?;
                    rows.remove(index)
                };
                self.queue_changed(id);
                self.start_turn(
                    id,
                    &json!({"input":row["input"],"clientUserMessageId":row["clientUserMessageId"]}),
                )
                .await?
            }
            "thread/queue/reorder" => {
                let ids = params["queuedSubmissionIds"].as_array().unwrap();
                let mut queues = self.queues.lock().await;
                let rows = queues.entry(id.into()).or_default();
                rows.sort_by_key(|row| ids.iter().position(|id| *id == row["id"]).unwrap());
                self.queue_changed(id);
                json!({})
            }
            _ => return Err(format!("unsupported method {method}")),
        };
        Ok(Some(result))
    }
    async fn start_turn(&self, id: &str, params: &Value) -> Result<Value, String> {
        let turn = json!({"id":format!("official-turn-{}",self.next_turn.fetch_add(1,Ordering::AcqRel)),"status":"inProgress","items":[{"id":params["clientUserMessageId"],"type":"userMessage","content":params["input"]}],"itemsView":"full"});
        let mut threads = self.threads.lock().await;
        let state = threads
            .get_mut(id)
            .ok_or_else(|| "thread not found".to_owned())?;
        state["thread"]["turns"]
            .as_array_mut()
            .unwrap()
            .push(turn.clone());
        state["thread"]["status"] = json!({"type":"active","activeFlags":[]});
        let _ = self
            .broadcasts
            .send(json!({"method":"turn/started","params":{"threadId":id,"turn":turn}}));
        let _=self.broadcasts.send(json!({"method":"thread/status/changed","params":{"threadId":id,"status":state["thread"]["status"]}}));
        Ok(json!({"turn":turn}))
    }
    fn queue_changed(&self, id: &str) {
        let _ = self
            .broadcasts
            .send(json!({"method":"thread/queue/changed","params":{"threadId":id}}));
    }
}
impl Drop for MockDesktop {
    fn drop(&mut self) {
        self.cancel.cancel();
        for task in self.tasks.get_mut().unwrap().iter() {
            task.abort();
        }
    }
}

fn mutation(value: &Value) -> bool {
    let method = value["method"].as_str().unwrap_or("");
    matches!(
        method,
        "thread/settings/update"
            | "turn/start"
            | "turn/steer"
            | "turn/interrupt"
            | "thread/queue/add"
            | "thread/queue/update"
            | "thread/queue/delete"
            | "thread/queue/reorder"
            | "thread/queue/start"
    ) || (method == "mcpServer/tool/call"
        && matches!(
            value["params"]["tool"].as_str(),
            Some("create_thread" | "move_thread_to_project")
        ))
}

async fn serve(
    mut socket: WebSocketStream<tokio::net::TcpStream>,
    mock: std::sync::Weak<MockDesktop>,
    mut events: broadcast::Receiver<Value>,
    cancel: CancellationToken,
) {
    loop {
        let incoming = tokio::select! {
            _=cancel.cancelled()=>break,
            event=events.recv()=>{
                let Ok(event)=event else{break;};
                if event.is_null(){let _=socket.close(None).await;break;}
                if socket.send(Message::Text(event.to_string().into())).await.is_err(){break;}
                continue;
            },
            frame=socket.next()=>frame,
        };
        let Some(Ok(frame)) = incoming else {
            break;
        };
        if matches!(frame, Message::Close(_)) {
            break;
        }
        let Ok(text) = frame.to_text() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            break;
        };
        let Some(mock) = mock.upgrade() else {
            break;
        };
        mock.calls.lock().await.push(value.clone());
        let mutating = mutation(&value);
        if mutating {
            tokio::time::sleep(Duration::from_millis(mock.delay_ms.load(Ordering::Acquire))).await;
        }
        let rejected = mutating && mock.fail_next.swap(false, Ordering::AcqRel);
        let result = if rejected {
            Err("fixture rejected".to_owned())
        } else {
            mock.respond(&value).await
        };
        if mutating && mock.drop_next.swap(false, Ordering::AcqRel) {
            let _ = socket.close(None).await;
            break;
        }
        let response = match result {
            Ok(Some(result)) => json!({"jsonrpc":"2.0","id":value["id"],"result":result}),
            Ok(None) => continue,
            Err(message) => {
                json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32000,"message":message}})
            }
        };
        if socket
            .send(Message::Text(response.to_string().into()))
            .await
            .is_err()
        {
            break;
        }
    }
}

pub fn official_thread(id: &str, active: bool) -> Value {
    json!({
        "thread":{"id":id,"name":format!("样例 {id}"),"cwd":"/fixture/project","projectId":"raw-server-project",
            "status":if active{json!({"type":"active","activeFlags":[]})}else{json!({"type":"idle"})},
            "turns":[{"id":format!("turn-{id}"),"status":if active{"inProgress"}else{"completed"},"itemsView":"full","items":[{"id":"answer","type":"agentMessage","text":format!("实时内容🙂 {id}")}]}]},
        "model":"desktop-default","reasoningEffort":"high","cwd":"/fixture/project",
        "collaborationMode":{"mode":"default","settings":{"model":"desktop-default","reasoning_effort":"high","developer_instructions":null}},
        "approvalPolicy":"never","approvalsReviewer":"user","sandbox":{"type":"dangerFullAccess"},"activePermissionProfile":{"id":":danger-full-access"}
    })
}

pub fn native_thread(id: &str, project: Value) -> Value {
    json!({"id":id,"title":format!("原生标题 {id}"),"kind":"codex","hostId":"local","status":"idle","cwd":"/fixture/project","projectId":project,"updatedAt":100})
}

pub struct Listener {
    #[cfg(unix)]
    inner: tokio::net::UnixListener,
    #[cfg(windows)]
    inner: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
    #[cfg(windows)]
    path: PathBuf,
}
impl Listener {
    pub fn bind(path: &Path) -> Self {
        #[cfg(unix)]
        {
            Self {
                inner: tokio::net::UnixListener::bind(path).unwrap(),
            }
        }
        #[cfg(windows)]
        {
            Self {
                inner: Some(
                    tokio::net::windows::named_pipe::ServerOptions::new()
                        .first_pipe_instance(true)
                        .create(path)
                        .unwrap(),
                ),
                path: path.to_path_buf(),
            }
        }
    }
    pub async fn accept(&mut self) -> Pipe {
        #[cfg(unix)]
        {
            Box::new(self.inner.accept().await.unwrap().0)
        }
        #[cfg(windows)]
        {
            self.inner.as_ref().unwrap().connect().await.unwrap();
            let connected = self.inner.take().unwrap();
            self.inner = Some(
                tokio::net::windows::named_pipe::ServerOptions::new()
                    .create(&self.path)
                    .unwrap(),
            );
            Box::new(connected)
        }
    }
}

pub async fn live(
    desktop: &Arc<codex_mobile_bridge::desktop::Desktop>,
) -> tokio::sync::watch::Receiver<codex_mobile_bridge::desktop::LiveView> {
    let mut receiver = desktop.subscribe("chat");
    tokio::time::timeout(Duration::from_secs(3), async {
        while receiver.borrow().snapshot.is_none() {
            receiver.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    receiver
}
