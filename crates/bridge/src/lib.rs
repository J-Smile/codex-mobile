pub mod app_server;
pub mod auth;
mod broker;
pub mod catalog;
pub mod config;
pub mod context;
pub mod desktop;
pub mod error;
pub mod goals;
pub mod history;
pub mod host;
pub mod http;
pub mod images;
pub mod interactions;
pub mod live;
mod native;
mod native_events;
pub mod notifications;
pub mod operations;
pub mod paths;
mod preview;
pub mod process_identity;
pub mod relay;
pub mod runtime;
pub mod session;
pub mod state;
pub mod transport;
pub mod updates;
pub mod usage;

pub use error::{BridgeError, Result};

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
pub fn digest(bytes: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes.as_ref()))
}
