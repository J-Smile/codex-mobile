use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, BridgeError>;

#[derive(Debug, Clone, thiserror::Error, Serialize, Deserialize)]
#[error("{message}")]
pub struct BridgeError {
    pub message: String,
    #[serde(skip_serializing)]
    pub status: u16,
    #[serde(skip)]
    pub before_send: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rpc_error: Option<Box<serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
}

impl BridgeError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status,
            before_send: false,
            rpc_error: None,
            outcome: None,
        }
    }
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::new(503, message)
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, message)
    }
    pub fn before_send(mut self) -> Self {
        self.before_send = true;
        self.outcome = Some("not-dispatched".into());
        self
    }
}

impl IntoResponse for BridgeError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE),
            Json(self),
        )
            .into_response()
    }
}

impl From<std::io::Error> for BridgeError {
    fn from(error: std::io::Error) -> Self {
        Self::unavailable(error.to_string())
    }
}
impl From<serde_json::Error> for BridgeError {
    fn from(error: serde_json::Error) -> Self {
        Self::unavailable(format!("数据格式无效：{error}"))
    }
}
impl From<rusqlite::Error> for BridgeError {
    fn from(error: rusqlite::Error) -> Self {
        Self::unavailable(format!("读取 Codex 数据失败：{error}"))
    }
}
impl From<reqwest::Error> for BridgeError {
    fn from(error: reqwest::Error) -> Self {
        Self::unavailable(format!("网络请求失败：{}", error.without_url()))
    }
}
