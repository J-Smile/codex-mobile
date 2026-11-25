use crate::{BridgeError, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use bytes::{Bytes, BytesMut};
use std::{sync::Arc, time::Duration};

pub(super) const FRAME_LIMIT: usize = 65_535;
pub(super) const BATCH_RECORDS: usize = 8;
pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);
const SESSION_HEADER: &str = "x-codex-relay-session";

pub(super) fn protocol_error(_: impl std::fmt::Display) -> BridgeError {
    BridgeError::invalid("云连接协议校验失败。")
}

pub(super) fn append_frame(batch: &mut Vec<u8>, frame: &[u8]) {
    let len = u16::try_from(frame.len()).expect("Noise 帧长度由协议库限制");
    batch.extend_from_slice(&len.to_be_bytes());
    batch.extend_from_slice(frame);
}

#[derive(Clone)]
pub(super) struct DesktopSender {
    client: reqwest::Client,
    endpoint: Arc<str>,
    token: Arc<str>,
}

impl DesktopSender {
    pub(super) fn new(endpoint: String, token: &str) -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(Duration::from_secs(45))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            endpoint: endpoint.into(),
            token: token.into(),
        })
    }

    pub(super) async fn approval(
        &self,
        path: &str,
        approver: &str,
        body: serde_json::Value,
    ) -> Result<reqwest::Response> {
        let request = self
            .client
            .post(format!("{}/approval{path}", self.endpoint))
            .bearer_auth(self.token.as_ref())
            .timeout(CONNECT_TIMEOUT)
            .json(&body);
        let response = if approver.is_empty() {
            request
        } else {
            request.header("x-codex-desktop-approver", approver)
        }
        .send()
        .await?;
        match response.status().as_u16() {
            200 | 204 => Ok(response),
            403 => Err(BridgeError::new(
                403,
                "验证失败，请使用此账号的网页登录密钥重新验证电脑。",
            )),
            404 => Err(BridgeError::unavailable("请更新云中转并确认电脑已连接。")),
            410 => Err(BridgeError::new(
                410,
                "手机已离开或授权码已失效，请重新授权。",
            )),
            _ => Err(BridgeError::unavailable("云端授权暂不可用，请重试。")),
        }
    }

    pub(super) async fn send(&self, session: &str, batch: Vec<u8>) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/send", self.endpoint))
            .bearer_auth(self.token.as_ref())
            .header(SESSION_HEADER, session)
            .header("content-type", "application/octet-stream")
            .timeout(CONNECT_TIMEOUT)
            .body(batch)
            .send()
            .await?;
        if response.status().is_success() {
            Ok(())
        } else {
            Err(BridgeError::unavailable(format!(
                "云中转已关闭此会话（HTTP {}）。",
                response.status()
            )))
        }
    }

    pub(super) async fn close(&self, session: &str) {
        let _ = self
            .client
            .post(format!("{}/close", self.endpoint))
            .bearer_auth(self.token.as_ref())
            .header(SESSION_HEADER, session)
            .timeout(CONNECT_TIMEOUT)
            .send()
            .await;
    }
}

pub(super) enum DesktopEvent {
    Ready,
    Open(String),
    Batch { session: String, bytes: Bytes },
    Close(String),
    Approval { request: String, proof: Vec<u8> },
}

pub(super) struct DesktopLink {
    response: reqwest::Response,
    pending: BytesMut,
}

impl DesktopLink {
    pub(super) async fn connect(
        endpoint: String,
        token: &str,
        name: &str,
    ) -> Result<(Self, DesktopSender)> {
        let sender = DesktopSender::new(endpoint, token)?;
        let response = sender
            .client
            .get(sender.endpoint.as_ref())
            .bearer_auth(sender.token.as_ref())
            .header("x-codex-desktop-name", URL_SAFE_NO_PAD.encode(name))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            return Err(BridgeError::new(403, "电脑尚未加入云端账号。"));
        }
        if !response.status().is_success()
            || !crate::auth::header(response.headers(), "content-type")
                .starts_with("text/event-stream")
        {
            return Err(BridgeError::unavailable(format!(
                "云中转登记失败（HTTP {}）。",
                response.status()
            )));
        }
        Ok((
            Self {
                response,
                pending: BytesMut::new(),
            },
            sender,
        ))
    }

    pub(super) async fn next(&mut self) -> Result<DesktopEvent> {
        loop {
            // Axum 生成的 SSE 使用 LF 和空行分隔，保留跨 HTTP 分块的数据。
            if let Some(end) = self.pending.windows(2).position(|part| part == b"\n\n") {
                let block = self.pending.split_to(end + 2);
                let text = std::str::from_utf8(&block).map_err(protocol_error)?;
                let mut event = "";
                let mut id = "";
                let mut data = "";
                for line in text.lines() {
                    if let Some(value) = line.strip_prefix("event: ") {
                        event = value;
                    }
                    if let Some(value) = line.strip_prefix("id: ") {
                        id = value;
                    }
                    if let Some(value) = line.strip_prefix("data: ") {
                        data = value;
                    }
                }
                match event {
                    "ready" => return Ok(DesktopEvent::Ready),
                    "open" | "close" if valid_session(data) => {
                        return Ok(if event == "open" {
                            DesktopEvent::Open(data.into())
                        } else {
                            DesktopEvent::Close(data.into())
                        })
                    }
                    "frame" if valid_session(id) => {
                        return Ok(DesktopEvent::Batch {
                            session: id.into(),
                            bytes: URL_SAFE_NO_PAD.decode(data).map_err(protocol_error)?.into(),
                        })
                    }
                    "approval" if valid_session(id) && data.len() <= 512 => {
                        return Ok(DesktopEvent::Approval {
                            request: id.into(),
                            proof: URL_SAFE_NO_PAD.decode(data).map_err(protocol_error)?,
                        });
                    }
                    "" | "heartbeat" => continue,
                    _ => return Err(protocol_error("desktop event")),
                }
            }
            let Some(chunk) = self.response.chunk().await? else {
                return Err(BridgeError::unavailable("云中转连接已关闭。"));
            };
            self.pending.extend_from_slice(&chunk);
            if self.pending.len() > 2 * BATCH_RECORDS * (FRAME_LIMIT + 2) {
                return Err(protocol_error("SSE event too large"));
            }
        }
    }
}

fn valid_session(session: &str) -> bool {
    session.len() == 32 && uuid::Uuid::parse_str(session).is_ok()
}
