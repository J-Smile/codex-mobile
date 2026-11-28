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
mod submissions;
#[cfg(test)]
extern crate self as codex_mobile_bridge;
#[cfg(test)]
#[path = "../tests/support/controls.rs"]
mod control_regression;
#[cfg(test)]
#[path = "../tests/support/native_pipe.rs"]
pub(crate) mod native_test_support;
#[cfg(test)]
#[path = "../tests/support/official_controls.rs"]
mod official_controls;
#[cfg(test)]
#[path = "../tests/support/queue_send.rs"]
mod queue_send_regression;
#[cfg(test)]
#[path = "../tests/support/restoration_lifecycle.rs"]
mod restoration_lifecycle;
#[cfg(test)]
#[path = "../tests/support/mod.rs"]
mod test_support;
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
