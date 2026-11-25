use super::{
    link::{
        append_frame, protocol_error, DesktopSender, BATCH_RECORDS, CONNECT_TIMEOUT, FRAME_LIMIT,
    },
    Identity, RemoteAccess, NOISE, PROLOGUE,
};
use crate::{auth::RelayPhone, http::Bridge, images::MAX_IMAGE_BYTES, BridgeError, Result};
use axum::{
    body::Body,
    http::{Method, Request, Uri},
};
use bytes::{Buf, Bytes, BytesMut};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::Arc};
use tokio::{
    sync::{broadcast, mpsc, Semaphore},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

const CHUNK: usize = 48 * 1024;
const AUTH: u8 = 1;
const AUTH_RESULT: u8 = 2;
const REQUEST: u8 = 3;
const REQUEST_DATA: u8 = 4;
const CANCEL: u8 = 5;
const RESPONSE: u8 = 6;
const RESPONSE_DATA: u8 = 7;
const RESPONSE_END: u8 = 8;
const RESPONSE_ERROR: u8 = 9;
const RESPONSE_NEXT: u8 = 10;
const RESPONSE_HEADERS: &[&str] = &[
    "content-type",
    "content-length",
    "content-range",
    "accept-ranges",
    "cache-control",
    "content-disposition",
];

const RECEIVE_WINDOW: usize = 8;

struct Frames {
    incoming: mpsc::Receiver<Bytes>,
    batch: Bytes,
}

impl Frames {
    async fn next(&mut self) -> Result<Bytes> {
        if self.batch.is_empty() {
            self.batch = self
                .incoming
                .recv()
                .await
                .ok_or_else(|| BridgeError::unavailable("手机加密连接已关闭。"))?;
        }
        if self.batch.len() < 2 {
            return Err(protocol_error("short ciphertext batch"));
        }
        let size = usize::from(self.batch.get_u16());
        if size == 0 || self.batch.len() < size {
            return Err(protocol_error("invalid ciphertext length"));
        }
        Ok(self.batch.split_to(size))
    }
}

pub(super) async fn serve(
    app: Arc<Bridge>,
    access: &RemoteAccess,
    identity: &Identity,
    sender: &DesktopSender,
    session: &str,
    origin: &str,
    incoming: mpsc::Receiver<Bytes>,
) -> Result<()> {
    let mut revoked = access.revoked.subscribe();
    let mut frames = Frames {
        incoming,
        batch: Bytes::new(),
    };
    let (mut cipher, phone_id, allowed) = tokio::time::timeout(CONNECT_TIMEOUT, async {
        let private = hex::decode(&identity.private).map_err(protocol_error)?;
        let mut handshake = snow::Builder::new(NOISE.parse().map_err(protocol_error)?)
            .prologue(PROLOGUE)
            .map_err(protocol_error)?
            .local_private_key(&private)
            .map_err(protocol_error)?
            .build_responder()
            .map_err(protocol_error)?;
        let mut output = [0; 256];
        if handshake
            .read_message(&frames.next().await?, &mut output)
            .map_err(protocol_error)?
            != 0
        {
            return Err(protocol_error("unexpected handshake payload"));
        }
        let phone_id = crate::digest(
            handshake
                .get_remote_static()
                .ok_or_else(|| protocol_error("phone key"))?,
        );
        let size = handshake
            .write_message(&[], &mut output)
            .map_err(protocol_error)?;
        let mut batch = Vec::new();
        append_frame(&mut batch, &output[..size]);
        sender.send(session, batch).await?;

        let mut cipher = handshake.into_transport_mode().map_err(protocol_error)?;
        let encrypted = frames.next().await?;
        let mut plaintext = vec![0; encrypted.len()];
        let size = cipher
            .read_message(&encrypted, &mut plaintext)
            .map_err(protocol_error)?;
        let (kind, id, data) = decode(&plaintext[..size])?;
        if kind != AUTH || id != 0 || data.len() > 1024 {
            return Err(protocol_error("expected authentication"));
        }
        let auth = serde_json::from_slice::<Value>(data)?;
        // 仅已通过 Noise 握手的手机可以递交配对码。
        let allowed = access
            .authorize(
                &phone_id,
                auth["ticket"].as_str().unwrap_or(""),
                auth["name"].as_str().unwrap_or(""),
            )
            .await?;
        Ok::<_, BridgeError>((cipher, phone_id, allowed))
    })
    .await
    .map_err(protocol_error)??;

    let status: &[u8] = if allowed { b"ok" } else { b"revoked" };
    let mut batch = Vec::new();
    encrypt_record(&mut cipher, &record(AUTH_RESULT, 0, status), &mut batch)?;
    sender.send(session, batch).await?;
    if !allowed {
        return Ok(());
    }

    tokio::select! {
        result = requests(app, sender, session, origin, cipher, frames, RelayPhone::new(&phone_id)) => result,
        _ = async {
            loop {
                match revoked.recv().await {
                    Ok(id) if id != phone_id => continue,
                    Ok(_) | Err(broadcast::error::RecvError::Closed | broadcast::error::RecvError::Lagged(_)) => break,
                }
            }
        } => Ok(())
    }
}

struct Active {
    cancel: CancellationToken,
    credit: Arc<Semaphore>,
}

async fn requests(
    app: Arc<Bridge>,
    sender: &DesktopSender,
    session: &str,
    origin: &str,
    mut cipher: snow::TransportState,
    mut frames: Frames,
    phone: RelayPhone,
) -> Result<()> {
    let (responses, mut output) = mpsc::channel::<Vec<u8>>(16);
    let mut incoming = HashMap::<u32, Incoming>::new();
    let mut active = HashMap::<u32, Active>::new();
    let mut tasks = JoinSet::<u32>::new();
    let mut plaintext = vec![0; FRAME_LIMIT];
    loop {
        let ciphertext = tokio::select! {
            frame = frames.next() => frame?,
            Some(message) = output.recv() => {
                let mut batch = Vec::new();
                encrypt_record(&mut cipher, &message, &mut batch)?;
                // 同一会话串行提交，把已经就绪的帧合入一个 POST。
                for _ in 1..BATCH_RECORDS {
                    let Ok(message) = output.try_recv() else { break; };
                    encrypt_record(&mut cipher, &message, &mut batch)?;
                }
                sender.send(session, batch).await?;
                continue;
            }
            Some(task) = tasks.join_next(), if !tasks.is_empty() => {
                active.remove(&task.map_err(protocol_error)?);
                continue;
            }
        };
        let len = cipher
            .read_message(&ciphertext, &mut plaintext)
            .map_err(protocol_error)?;
        let (kind, id, data) = decode(&plaintext[..len])?;
        if id == 0 {
            return Err(protocol_error("invalid request id"));
        }
        match kind {
            REQUEST if !incoming.contains_key(&id) && !active.contains_key(&id) => {
                if incoming.len() + active.len() >= 48 {
                    let mut batch = Vec::new();
                    encrypt_record(&mut cipher, &record(RESPONSE_ERROR, id, &[]), &mut batch)?;
                    sender.send(session, batch).await?;
                    continue;
                }
                incoming.insert(id, Incoming::start(data)?);
            }
            REQUEST_DATA => {
                if let Some(request) = incoming.get_mut(&id) {
                    request.append(data)?;
                }
            }
            CANCEL => {
                incoming.remove(&id);
                if let Some(task) = active.get(&id) {
                    task.cancel.cancel();
                }
                continue;
            }
            RESPONSE_NEXT => {
                let granted = match data {
                    [] => 1,
                    [count] if (1..=RECEIVE_WINDOW).contains(&usize::from(*count)) => {
                        usize::from(*count)
                    }
                    _ => return Err(protocol_error("invalid response window")),
                };
                if let Some(task) = active.get(&id) {
                    task.credit.add_permits(
                        granted.min(RECEIVE_WINDOW.saturating_sub(task.credit.available_permits())),
                    );
                }
                continue;
            }
            _ => return Err(protocol_error("unknown record")),
        }
        if incoming
            .get(&id)
            .is_some_and(|request| request.body.len() == request.head.length)
        {
            let request = incoming.remove(&id).unwrap();
            let credit = Arc::new(Semaphore::new(request.head.response_window));
            let request = request.request(origin);
            let app = app.clone();
            let phone = phone.clone();
            let responses = responses.clone();
            let cancel = CancellationToken::new();
            active.insert(
                id,
                Active {
                    cancel: cancel.clone(),
                    credit: credit.clone(),
                },
            );
            tasks.spawn(async move {
                tokio::select! {
                    _ = cancel.cancelled() => {},
                    _ = respond(app, request, phone, id, responses, credit) => {},
                }
                id
            });
        }
    }
}

fn encrypt_record(
    cipher: &mut snow::TransportState,
    message: &[u8],
    batch: &mut Vec<u8>,
) -> Result<()> {
    let mut bytes = vec![0; message.len() + 16];
    let length = cipher
        .write_message(message, &mut bytes)
        .map_err(protocol_error)?;
    append_frame(batch, &bytes[..length]);
    Ok(())
}

async fn respond(
    app: Arc<Bridge>,
    request: Result<Request<Body>>,
    phone: RelayPhone,
    id: u32,
    responses: mpsc::Sender<Vec<u8>>,
    credit: Arc<Semaphore>,
) -> Result<()> {
    use axum::response::IntoResponse;
    let response = match request {
        Ok(request) => app.relay_http(request, phone).await,
        Err(error) => error.into_response(),
    };
    let (parts, body) = response.into_parts();
    let headers: HashMap<_, _> = RESPONSE_HEADERS
        .iter()
        .filter_map(|name| {
            parts
                .headers
                .get(*name)
                .and_then(|value| value.to_str().ok())
                .map(|value| (*name, value))
        })
        .collect();
    let head = serde_json::to_vec(&json!({"status":parts.status.as_u16(),"headers":headers}))?;
    responses
        .send(record(RESPONSE, id, &head))
        .await
        .map_err(protocol_error)?;
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            responses
                .send(record(RESPONSE_ERROR, id, &[]))
                .await
                .map_err(protocol_error)?;
            return Ok(());
        };
        for part in chunk.chunks(CHUNK) {
            credit.acquire().await.map_err(protocol_error)?.forget();
            responses
                .send(record(RESPONSE_DATA, id, part))
                .await
                .map_err(protocol_error)?;
        }
    }
    responses
        .send(record(RESPONSE_END, id, &[]))
        .await
        .map_err(protocol_error)?;
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestHead {
    method: String,
    path: String,
    length: usize,
    content_type: Option<String>,
    range: Option<String>,
    file_name: Option<String>,
    #[serde(default)]
    response_window: usize,
}

struct Incoming {
    head: RequestHead,
    body: BytesMut,
}

impl Incoming {
    fn start(data: &[u8]) -> Result<Self> {
        let head = serde_json::from_slice::<RequestHead>(data)?;
        let max = if head.path == "/api/images" {
            MAX_IMAGE_BYTES
        } else {
            1024 * 1024
        };
        if head.length > max || head.response_window > RECEIVE_WINDOW {
            return Err(protocol_error("request body size"));
        }
        Ok(Self {
            head,
            body: BytesMut::new(),
        })
    }
    fn append(&mut self, data: &[u8]) -> Result<()> {
        if data.len() > CHUNK || self.body.len() + data.len() > self.head.length {
            return Err(protocol_error("invalid request chunk"));
        }
        self.body.extend_from_slice(data);
        Ok(())
    }
    fn request(self, origin: &str) -> Result<Request<Body>> {
        let uri = self.head.path.parse::<Uri>().map_err(protocol_error)?;
        let method = self.head.method.parse::<Method>().map_err(protocol_error)?;
        if !self.head.path.starts_with("/api/")
            || uri.authority().is_some()
            || !matches!(
                method,
                Method::GET
                    | Method::HEAD
                    | Method::POST
                    | Method::PATCH
                    | Method::DELETE
                    | Method::PUT
            )
        {
            return Err(BridgeError::invalid("云连接只支持本机 Codex Mobile API。"));
        }
        let host = origin.split_once("://").expect("云端地址已校验").1;
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", host)
            .header("origin", origin);
        if let Some(content_type) = self.head.content_type {
            builder = builder.header("content-type", content_type);
        }
        if let Some(file_name) = self.head.file_name {
            builder = builder.header("x-file-name", file_name);
        }
        if let Some(range) = self.head.range {
            builder = builder.header("range", range);
        }
        builder
            .body(Body::from(self.body.freeze()))
            .map_err(protocol_error)
    }
}

fn record(kind: u8, id: u32, data: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(5 + data.len());
    output.push(kind);
    output.extend_from_slice(&id.to_be_bytes());
    output.extend_from_slice(data);
    output
}

fn decode(message: &[u8]) -> Result<(u8, u32, &[u8])> {
    let (head, data) = message
        .split_at_checked(5)
        .ok_or_else(|| protocol_error("short record"))?;
    Ok((
        head[0],
        u32::from_be_bytes(head[1..].try_into().unwrap()),
        data,
    ))
}
