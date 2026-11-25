use crate::{
    app_server::{AppServerConnection, AppServerEvent},
    paths::Paths,
    session::Session,
    BridgeError, Result,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::sync::{broadcast, Mutex, RwLock};
use tokio_util::sync::CancellationToken;

struct GoalConnection {
    id: String,
    applied: HashMap<String, u64>,
}

pub struct Goals {
    session: Arc<Session>,
    owns_session: bool,
    connection: Mutex<Option<GoalConnection>>,
    listening: AtomicBool,
    cancel: CancellationToken,
    pub states: RwLock<HashMap<String, Value>>,
    completed: RwLock<HashMap<String, Value>>,
    pub changes: broadcast::Sender<String>,
    pub reconnects: broadcast::Sender<()>,
}
impl Goals {
    pub fn new(paths: Paths) -> Arc<Self> {
        Self::build(Session::new(paths), true)
    }
    pub fn with_session(session: Arc<Session>) -> Arc<Self> {
        Self::build(session, false)
    }
    fn build(session: Arc<Session>, owns_session: bool) -> Arc<Self> {
        let (changes, _) = broadcast::channel(128);
        let (reconnects, _) = broadcast::channel(8);
        Arc::new(Self {
            session,
            owns_session,
            connection: Mutex::new(None),
            listening: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            states: RwLock::new(HashMap::new()),
            completed: RwLock::new(HashMap::new()),
            changes,
            reconnects,
        })
    }
    fn listen(self: &Arc<Self>) {
        if self.listening.swap(true, Ordering::AcqRel) {
            return;
        }
        let mut events = self.session.events.subscribe();
        let mut connections = self.session.connections.subscribe();
        let weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    connection = connections.recv() => {
                        match connection {
                            Ok(connection) => {
                                let Some(goals) = weak.upgrade() else { break; };
                                goals.remember_connection(&connection).await;
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = events.recv() => {
                        let Some(goals) = weak.upgrade() else { break; };
                        match event {
                            Ok(event) => goals.apply(event).await,
                            Err(broadcast::error::RecvError::Lagged(_)) => goals.resync().await,
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });
    }
    async fn remember_connection(&self, connection: &AppServerConnection) {
        let mut guard = self.connection.lock().await;
        if let Some(previous) = guard
            .as_ref()
            .filter(|previous| previous.id == connection.id)
        {
            let _ = previous;
            return;
        }

        let reconnected = guard.is_some();
        *guard = Some(GoalConnection {
            id: connection.id.clone(),

            applied: HashMap::new(),
        });
        if reconnected {
            self.states.write().await.clear();
            let _ = self.reconnects.send(());
        }
    }
    async fn connection(self: &Arc<Self>) -> Result<Arc<AppServerConnection>> {
        self.session.require("readLive")?;
        if self.cancel.is_cancelled() {
            return Err(BridgeError::unavailable("目标观察已停止。").before_send());
        }
        self.listen();
        let connection = self.session.connect().await?;
        self.remember_connection(&connection).await;
        Ok(connection)
    }
    pub async fn connect(self: &Arc<Self>) -> Result<()> {
        self.connection().await.map(|_| ())
    }
    async fn resync(&self) {
        self.states.write().await.clear();
        let _ = self.reconnects.send(());
    }
    async fn apply(&self, event: AppServerEvent) {
        match event {
            AppServerEvent::Notification {
                connection_id,
                sequence,
                message,
            } => {
                let mut connection = self.connection.lock().await;
                let Some(connection) = connection
                    .as_mut()
                    .filter(|connection| connection.id == connection_id)
                else {
                    return;
                };
                let method = message["method"].as_str();
                if method == Some("codexMobile/resync") {
                    self.resync().await;
                    return;
                }
                if !matches!(method, Some("thread/goal/updated" | "thread/goal/cleared")) {
                    return;
                }
                let Some(id) = message["params"]["threadId"].as_str() else {
                    return;
                };
                if sequence <= connection.applied.get(id).copied().unwrap_or(0) {
                    return;
                }
                connection.applied.insert(id.into(), sequence);
                let goal = if method == Some("thread/goal/cleared") {
                    Value::Null
                } else {
                    message["params"]["goal"].clone()
                };
                self.remember(id, goal).await;
            }
            AppServerEvent::Disconnected { connection_id, .. } => {
                let connection = self.connection.lock().await;
                if connection
                    .as_ref()
                    .is_some_and(|connection| connection.id == connection_id)
                {
                    self.states.write().await.clear();
                }
            }
            AppServerEvent::ServerRequest { .. } => {}
        }
    }
    pub async fn remember_completed(&self, id: &str, goal: &Value) -> Value {
        let mut completed = self.completed.write().await;
        if goal.is_object()
            && completed.get(id).is_none_or(|old| {
                goal["updatedAt"].as_i64().unwrap_or(0) > old["updatedAt"].as_i64().unwrap_or(0)
            })
        {
            completed.insert(id.into(), goal.clone());
        }
        completed.get(id).cloned().unwrap_or(Value::Null)
    }
    async fn state(&self, id: &str, goal: Value) -> Value {
        let completed = self
            .remember_completed(
                id,
                if goal["status"] == "complete" {
                    &goal
                } else {
                    &Value::Null
                },
            )
            .await;
        json!({"goal": goal, "completedGoal": completed})
    }
    async fn remember(&self, id: &str, goal: Value) -> Value {
        let state = self.state(id, goal).await;
        let previous = self.states.write().await.insert(id.into(), state.clone());
        if previous.as_ref() != Some(&state) {
            let _ = self.changes.send(id.into());
        }
        state
    }
    pub async fn request(
        self: &Arc<Self>,
        id: &str,
        method: &str,
        changes: Value,
    ) -> Result<Value> {
        if method != "thread/goal/get" {
            return Err(
                BridgeError::unavailable("手机目标当前仅供查看，请在桌面设置或暂停目标。")
                    .before_send(),
            );
        }
        self.session.require("readLive")?;
        let connection = self.connection().await?;
        // This receiver shares the socket reader. It lets us apply notifications
        // known to precede the response before a slower Session observer runs.
        let mut during_request = connection.subscribe();
        let before = self.states.read().await.get(id).cloned();
        let mut params = changes;
        params["threadId"] = json!(id);

        let (result, sequence) = connection.rpc_with_sequence(method, params).await?;
        loop {
            match during_request.try_recv() {
                Ok(event) => {
                    let after_response = match &event {
                        AppServerEvent::Notification {
                            sequence: received, ..
                        }
                        | AppServerEvent::ServerRequest {
                            sequence: received, ..
                        }
                        | AppServerEvent::Disconnected {
                            sequence: received, ..
                        } => *received > sequence,
                    };
                    self.apply(event).await;
                    if after_response {
                        break;
                    }
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => self.resync().await,
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
            }
        }
        let mut current = self.connection.lock().await;
        let goal = result["goal"].clone();
        let Some(current) = current
            .as_mut()
            .filter(|current| current.id == connection.id && connection.is_alive())
        else {
            return Ok(self.state(id, goal).await);
        };
        let applied = current.applied.entry(id.into()).or_default();
        *applied = (*applied).max(sequence);
        let after = self.states.read().await.get(id).cloned();
        if after != before {
            if let Some(after) = after {
                return Ok(after);
            }
        }
        Ok(self.remember(id, goal).await)
    }
    pub async fn close(&self) {
        self.cancel.cancel();
        if self.owns_session {
            self.session.close().await;
        }
        self.states.write().await.clear();
    }
}
impl Drop for Goals {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
