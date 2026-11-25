mod accounts;
mod desktop_approval;
mod desktop_link;
mod passkeys;

use accounts::{digest, random_key, Account, ListedDesktop, SessionKey, SESSION_SECONDS};
pub use accounts::{AccountKeys, Accounts};
use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures_util::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    collections::{hash_map::Entry, HashMap},
    convert::Infallible,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const MAX_BATCH_BYTES: usize = 8 * (65_535 + 2);
const MAX_SESSIONS: usize = 32;
const SESSION_HEADER: &str = "x-codex-relay-session";
const DESKTOP_NAME_HEADER: &str = "x-codex-desktop-name";
const ACCOUNT_COOKIE: &str = "__Host-codex-relay";
type DesktopKey = (String, String);
type OnlineDesktops = Arc<Mutex<HashMap<DesktopKey, Arc<Desktop>>>>;

#[derive(Clone)]
struct Relay {
    origin: Arc<str>,
    accounts: Arc<Accounts>,
    passkeys: Option<Arc<passkeys::Passkeys>>,
    approvals: desktop_approval::Approvals,
    desktop_links: desktop_link::DesktopLinks,
    online: OnlineDesktops,
}

struct Desktop {
    events: mpsc::Sender<DesktopEvent>,
    sessions: Mutex<HashMap<Uuid, MobileSession>>,
    closed: CancellationToken,
}

struct MobileSession {
    sender: mpsc::Sender<Bytes>,
    key: SessionKey,
    closed: CancellationToken,
}

enum DesktopEvent {
    Open(Uuid),
    Batch(Uuid, Bytes),
    Close(Uuid),
    Approval { request: Uuid, proof: String },
}

struct DesktopRegistration {
    relay: Relay,
    key: DesktopKey,
    desktop: Arc<Desktop>,
}

impl Drop for DesktopRegistration {
    fn drop(&mut self) {
        self.desktop.closed.cancel();
        self.relay.online.lock().unwrap().remove(&self.key);
    }
}

struct MobileRegistration {
    desktop: Arc<Desktop>,
    session: Uuid,
    close: Option<mpsc::OwnedPermit<DesktopEvent>>,
}

impl Drop for MobileRegistration {
    fn drop(&mut self) {
        let registered = self.desktop.sessions.lock().unwrap().remove(&self.session);
        if registered.is_some() && !self.desktop.closed.is_cancelled() {
            self.close
                .take()
                .unwrap()
                .send(DesktopEvent::Close(self.session));
        }
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_desktop_name(name: &str) -> bool {
    !name.trim().is_empty() && name.chars().count() <= 48 && !name.chars().any(char::is_control)
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

fn session_id(headers: &HeaderMap) -> Result<Uuid, StatusCode> {
    headers
        .get(SESSION_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .ok_or(StatusCode::BAD_REQUEST)
}

impl Relay {
    fn desktop_key(&self, headers: &HeaderMap, id: String) -> Result<DesktopKey, StatusCode> {
        let token = bearer_token(headers)
            .filter(|value| {
                value
                    .strip_prefix("cm_desktop_")
                    .or_else(|| value.strip_prefix("cm_device_"))
                    .is_some_and(valid_key)
                    && valid_id(&id)
            })
            .ok_or(StatusCode::FORBIDDEN)?;
        let owner = self
            .accounts
            .desktop_owner(token, &id)
            .map_err(database_error)?
            .ok_or(StatusCode::FORBIDDEN)?;
        Ok((owner, id))
    }

    fn mobile_allowed(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::ORIGIN)
            .and_then(|origin| origin.to_str().ok())
            == Some(&self.origin)
    }

    fn mobile_session(&self, key: SessionKey) -> Result<(Account, u64), StatusCode> {
        self.accounts
            .session(key)
            .map_err(database_error)?
            .ok_or(StatusCode::UNAUTHORIZED)
    }

    fn desktop(&self, key: &DesktopKey) -> Result<Arc<Desktop>, StatusCode> {
        self.online
            .lock()
            .unwrap()
            .get(key)
            .cloned()
            .ok_or(StatusCode::NOT_FOUND)
    }

    fn account_view(&self, account: Account) -> Result<AccountView, StatusCode> {
        let mut desktops = self
            .accounts
            .desktops(&account.id)
            .map_err(database_error)?;
        let online = self.online.lock().unwrap();
        for desktop in &mut desktops {
            desktop.online = online.contains_key(&(account.id.clone(), desktop.id.clone()));
        }
        drop(online);
        let has_passkey = self
            .accounts
            .has_passkey(&account.id)
            .map_err(database_error)?;
        Ok(AccountView {
            account,
            desktops,
            has_passkey,
        })
    }

    fn signed_in(
        &self,
        account: Account,
        key: &str,
        previous: Option<SessionKey>,
    ) -> Result<Response, StatusCode> {
        if let Some(previous) = previous {
            self.close_session(previous);
        }
        Ok((
            [
                (
                    header::SET_COOKIE,
                    secure_cookie(ACCOUNT_COOKIE, key, SESSION_SECONDS),
                ),
                (header::CACHE_CONTROL, "private, no-store".parse().unwrap()),
            ],
            Json(self.account_view(account)?),
        )
            .into_response())
    }

    fn close_session(&self, key: SessionKey) {
        let online = self.online.lock().unwrap();
        for desktop in online.values() {
            for session in desktop.sessions.lock().unwrap().values() {
                if session.key == key {
                    session.closed.cancel();
                }
            }
        }
    }
}

fn database_error(error: rusqlite::Error) -> StatusCode {
    eprintln!("Codex Mobile account database: {error}");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
}

fn session_key(headers: &HeaderMap) -> Option<SessionKey> {
    cookie_key(headers, ACCOUNT_COOKIE)
}

fn cookie_key(headers: &HeaderMap, cookie_name: &str) -> Option<SessionKey> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|header| header.to_str().ok())
        .flat_map(|header| header.split(';'))
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find(|(name, token)| *name == cookie_name && valid_key(token))
        .map(|(_, token)| digest(token))
}

fn secure_cookie(name: &str, key: &str, max_age: i64) -> axum::http::HeaderValue {
    format!("{name}={key}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={max_age}")
        .parse()
        .unwrap()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Login {
    login_key: String,
}

#[derive(Serialize)]
struct AccountView {
    #[serde(flatten)]
    account: Account,
    desktops: Vec<ListedDesktop>,
    #[serde(rename = "hasPasskey")]
    has_passkey: bool,
}

async fn get_account(
    State(relay): State<Relay>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    let key = session_key(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let (account, _) = relay.mobile_session(key)?;
    Ok((
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(relay.account_view(account)?),
    ))
}

async fn login(
    State(relay): State<Relay>,
    headers: HeaderMap,
    Json(login): Json<Login>,
) -> Result<Response, StatusCode> {
    if !relay.mobile_allowed(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    if !login
        .login_key
        .strip_prefix("cm_login_")
        .is_some_and(valid_key)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let key = random_key();
    let previous = session_key(&headers);
    let account = relay
        .accounts
        .login(&login.login_key, digest(&key), previous)
        .map_err(database_error)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    relay.signed_in(account, &key, previous)
}

async fn logout(
    State(relay): State<Relay>,
    headers: HeaderMap,
) -> Result<impl IntoResponse, StatusCode> {
    if !relay.mobile_allowed(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    if let Some(key) = session_key(&headers) {
        relay.accounts.logout(key).map_err(database_error)?;
        relay.close_session(key);
    }
    Ok((
        [(header::SET_COOKIE, secure_cookie(ACCOUNT_COOKIE, "", 0))],
        StatusCode::NO_CONTENT,
    ))
}

fn event_stream(events: impl Stream<Item = Event> + Send + 'static) -> Response {
    let mut response = Sse::new(events.map(Ok::<_, Infallible>))
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .event(Event::default().event("heartbeat").data("0")),
        )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CACHE_CONTROL,
        "no-store, no-transform".parse().unwrap(),
    );
    headers.insert("x-accel-buffering", "no".parse().unwrap());
    headers.insert("referrer-policy", "no-referrer".parse().unwrap());
    response
}

async fn desktop_events(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    let key = relay.desktop_key(&headers, id)?;
    let name = headers
        .get(DESKTOP_NAME_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|encoded| encoded.len() <= 256)
        .and_then(|encoded| URL_SAFE_NO_PAD.decode(encoded).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|name| valid_desktop_name(name))
        .ok_or(StatusCode::BAD_REQUEST)?;
    let (events, receiver) = mpsc::channel(128);
    let desktop = Arc::new(Desktop {
        events,
        sessions: Mutex::default(),
        closed: CancellationToken::new(),
    });
    match relay.online.lock().unwrap().entry(key.clone()) {
        Entry::Vacant(entry) => {
            relay
                .accounts
                .register_desktop(&key.0, &key.1, &name)
                .map_err(database_error)?;
            entry.insert(desktop.clone());
        }
        Entry::Occupied(_) => return Err(StatusCode::CONFLICT),
    }
    let guard = DesktopRegistration {
        relay,
        key,
        desktop,
    };
    let events = ReceiverStream::new(receiver).map(move |event| {
        let _registration = &guard;
        match event {
            DesktopEvent::Open(id) => Event::default().event("open").data(id.simple().to_string()),
            DesktopEvent::Batch(id, bytes) => Event::default()
                .event("frame")
                .id(id.simple().to_string())
                .data(URL_SAFE_NO_PAD.encode(bytes)),
            DesktopEvent::Close(id) => Event::default()
                .event("close")
                .data(id.simple().to_string()),
            DesktopEvent::Approval { request, proof } => Event::default()
                .event("approval")
                .id(request.simple().to_string())
                .data(proof),
        }
    });
    Ok(event_stream(
        futures_util::stream::once(async { Event::default().event("ready").data("ready") })
            .chain(events),
    ))
}

async fn mobile_events(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, StatusCode> {
    if !relay.mobile_allowed(&headers) || !valid_id(&id) {
        return Err(StatusCode::FORBIDDEN);
    }
    let key = session_key(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let (account, _) = relay.mobile_session(key)?;
    let desktop = relay.desktop(&(account.id, id))?;
    let session = Uuid::new_v4();
    let (sender, receiver) = mpsc::channel(8);
    let closed = desktop.closed.child_token();
    let (close, remaining) = {
        let mut sessions = desktop.sessions.lock().unwrap();
        // 与退出账号关闭流使用同一把锁，确保刚建立的连接也能收到取消信号。
        let (_, remaining) = relay.mobile_session(key)?;
        if sessions.len() >= MAX_SESSIONS {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        // 给每个手机的断开事件预留一个位置，转发队列满时也能释放桌面的会话。
        let close = desktop
            .events
            .clone()
            .try_reserve_owned()
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        desktop
            .events
            .try_send(DesktopEvent::Open(session))
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
        sessions.insert(
            session,
            MobileSession {
                sender,
                key,
                closed: closed.clone(),
            },
        );
        (close, remaining)
    };
    let guard = MobileRegistration {
        desktop,
        session,
        close: Some(close),
    };
    let events = ReceiverStream::new(receiver).map(move |bytes| {
        let _registration = &guard;
        Event::default()
            .event("frame")
            .data(URL_SAFE_NO_PAD.encode(bytes))
    });
    let ready = futures_util::stream::once(async move {
        Event::default()
            .event("ready")
            .data(session.simple().to_string())
    });
    Ok(event_stream(ready.chain(events).take_until(async move {
        tokio::select! {
            _ = closed.cancelled_owned() => {}
            _ = tokio::time::sleep(Duration::from_secs(remaining)) => {}
        }
    })))
}

async fn mobile_send(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    if !relay.mobile_allowed(&headers) {
        return Err(StatusCode::FORBIDDEN);
    }
    let key = session_key(&headers).ok_or(StatusCode::UNAUTHORIZED)?;
    let (account, _) = relay.mobile_session(key)?;
    let session = session_id(&headers)?;
    let desktop = relay.desktop(&(account.id, id))?;
    let sessions = desktop.sessions.lock().unwrap();
    if !sessions
        .get(&session)
        .is_some_and(|session| session.key == key && !session.closed.is_cancelled())
    {
        return Err(StatusCode::NOT_FOUND);
    }
    desktop
        .events
        .try_send(DesktopEvent::Batch(session, body))
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn desktop_send(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, StatusCode> {
    let key = relay.desktop_key(&headers, id)?;
    let session = session_id(&headers)?;
    let desktop = relay.desktop(&key)?;
    let sender = desktop
        .sessions
        .lock()
        .unwrap()
        .get(&session)
        .filter(|session| !session.closed.is_cancelled())
        .map(|session| session.sender.clone())
        .ok_or(StatusCode::NOT_FOUND)?;
    // 等待手机下行队列腾出空间只阻塞当前会话。
    tokio::time::timeout(Duration::from_secs(10), sender.send(body))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .map_err(|_| StatusCode::NOT_FOUND)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn desktop_close(
    State(relay): State<Relay>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let key = relay.desktop_key(&headers, id)?;
    let session = session_id(&headers)?;
    let desktop = relay.desktop(&key)?;
    // 关闭发送端，让手机先收完已经提交的下行帧。
    desktop.sessions.lock().unwrap().remove(&session);
    Ok(StatusCode::NO_CONTENT)
}

pub fn router(origin: String, accounts: Accounts) -> Router {
    let relay = Relay {
        passkeys: passkeys::Passkeys::new(&origin).map(Arc::new),
        approvals: Default::default(),
        desktop_links: Default::default(),
        origin: origin.into(),
        accounts: Arc::new(accounts),
        online: Arc::default(),
    };
    Router::new()
        .route(
            "/relay/account",
            get(get_account).post(login).delete(logout),
        )
        .merge(passkeys::routes())
        .merge(desktop_approval::routes())
        .merge(desktop_link::routes())
        .route("/relay/desktop/{id}", get(desktop_events))
        .route("/relay/desktop/{id}/send", post(desktop_send))
        .route("/relay/desktop/{id}/close", post(desktop_close))
        .route("/relay/mobile/{id}/session", post(mobile_events))
        .route("/relay/mobile/{id}/send", post(mobile_send))
        .route("/relay/health", get(|| async { StatusCode::NO_CONTENT }))
        .layer(DefaultBodyLimit::max(MAX_BATCH_BYTES))
        .with_state(relay)
}
