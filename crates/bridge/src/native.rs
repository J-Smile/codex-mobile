//! 官方桌面 App Tools 的原生管道；不是独立 App Server。
use crate::{
    paths::Paths, process_identity::ProcessIdentity, transport::Pipe, BridgeError, Result,
};
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_FRAME: usize = 8 * 1024 * 1024;

pub(crate) fn eligible(paths: &Paths) -> bool {
    cfg!(windows)
        && dirs::home_dir()
            .and_then(|home| std::fs::canonicalize(home.join(".codex")).ok())
            .zip(std::fs::canonicalize(&paths.codex_home).ok())
            .is_some_and(|(a, b)| a == b)
}

#[derive(Clone)]
pub(crate) struct NativeOwner {
    pub pipe: PathBuf,
    pub desktop: ProcessIdentity,
    pub server: ProcessIdentity,
}
impl NativeOwner {
    pub fn instance_id(&self) -> String {
        crate::digest(serde_json::json!([self.pipe, self.desktop, self.server]).to_string())
    }
    pub fn validate(&self) -> Result<()> {
        if !self.desktop.alive() || !self.server.alive() {
            return Err(BridgeError::unavailable("官方桌面连接已换代，请重新同步。"));
        }
        Ok(())
    }
}

pub(crate) async fn discover(paths: &Paths) -> Result<(Pipe, NativeOwner)> {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
        // 当前验收只覆盖 Windows 默认 HOME；隔离测试与其他 HOME 不接入真实桌面。
        if !eligible(paths) {
            return Err(
                BridgeError::unavailable("此 Codex 目录尚未验收原生桌面连接。").before_send(),
            );
        }
        let paths = paths.clone();
        let expected_owner = if crate::runtime::SHARED_RUNTIME_VERIFIED {
            crate::runtime::read_runtime(&paths.runtime_home)
                .ok()
                .flatten()
                .map(|runtime| runtime.desktop)
        } else {
            None
        };
        let (desktop, pipes) = tokio::task::spawn_blocking(move || -> Result<_> {
            let desktop = crate::runtime::discover_desktop(&paths)?
                .filter(|desktop| desktop.running)
                .ok_or_else(|| BridgeError::unavailable("请先打开官方 Codex 桌面。"))?;
            let pipes = std::fs::read_dir(r"\\.\pipe\")?
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("codex-app-tools-")
                })
                .map(|entry| entry.path())
                .collect::<Vec<_>>();
            Ok((desktop.desktop_path, pipes))
        })
        .await
        .map_err(|error| BridgeError::unavailable(error.to_string()))??;
        let mut found = None;
        for path in pipes {
            let Ok(pipe) = tokio::net::windows::named_pipe::ClientOptions::new().open(&path) else {
                continue;
            };
            let mut pid = 0;
            if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle(), &mut pid) } == 0 {
                continue;
            }
            let Some(server) = ProcessIdentity::read(pid) else {
                continue;
            };
            let Some(owner) = crate::runtime::find_desktop_owner(
                &desktop,
                pid,
                ProcessIdentity::read,
                crate::process_identity::parent_pid,
            ) else {
                continue;
            };
            if expected_owner
                .as_ref()
                .is_some_and(|expected| expected != &owner)
            {
                continue;
            }
            if found.is_some() {
                return Err(BridgeError::unavailable(
                    "存在多个官方桌面原生实例，无法确定目标连接。",
                )
                .before_send());
            }
            found = Some((
                Box::new(pipe) as Pipe,
                NativeOwner {
                    pipe: path,
                    desktop: owner,
                    server,
                },
            ));
        }
        found.ok_or_else(|| {
            BridgeError::unavailable("未找到当前官方桌面的 App Tools 通道。").before_send()
        })
    }
    #[cfg(not(windows))]
    {
        let _ = paths;
        Err(BridgeError::unavailable("此平台尚未验收原生桌面控制。").before_send())
    }
}

pub(crate) fn supported_catalog(value: &Value) -> bool {
    [
        "read_thread",
        "send_message_to_thread",
        "create_thread",
        "list_projects",
        "list_threads",
    ]
    .iter()
    .all(|name| {
        value["tools"].as_array().is_some_and(|tools| {
            tools.iter().any(|tool| {
                let required = match *name {
                    "read_thread" => vec!["threadId"],
                    "send_message_to_thread" => vec!["threadId", "prompt"],
                    "create_thread" => vec!["prompt", "target"],
                    _ => vec![],
                };
                let actual = tool["inputSchema"]["required"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                tool["namespace"] == "codex_app"
                    && tool["name"] == *name
                    && tool["inputSchema"]["type"] == "object"
                    && tool["inputSchema"]["additionalProperties"] == false
                    && actual.len() == required.len()
                    && required
                        .iter()
                        .all(|field| actual.iter().any(|value| value == field))
                    && required
                        .iter()
                        .all(|field| tool["inputSchema"]["properties"][*field].is_object())
            })
        })
    })
}

pub(crate) fn model_efforts(catalog: &Value) -> Option<Vec<String>> {
    let tools = catalog["tools"].as_array()?;
    let mut efforts = crate::config::EFFORTS
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    for name in ["create_thread", "send_message_to_thread"] {
        let tool = tools
            .iter()
            .find(|tool| tool["namespace"] == "codex_app" && tool["name"] == name)?;
        let properties = &tool["inputSchema"]["properties"];
        if properties["model"]["type"] != "string" || properties["thinking"]["type"] != "string" {
            return None;
        }
        let supported = properties["thinking"]["enum"].as_array()?;
        efforts.retain(|effort| supported.iter().any(|value| value.as_str() == Some(effort)));
    }
    (!efforts.is_empty()).then_some(efforts)
}

pub(crate) fn navigation_supported(catalog: &Value) -> bool {
    catalog["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            let schema = &tool["inputSchema"];
            tool["namespace"] == "codex_app"
                && tool["name"] == "navigate_to_codex_page"
                && schema["type"] == "object"
                && schema["additionalProperties"] == false
                && schema["required"] == serde_json::json!(["threadId"])
                && schema["properties"]["threadId"]["type"] == "string"
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_requires_its_own_confirmed_tool_contract() {
        let mut catalog = serde_json::json!({"tools":[{
            "namespace":"codex_app","name":"navigate_to_codex_page","inputSchema":{
                "type":"object","additionalProperties":false,"required":["threadId"],
                "properties":{"threadId":{"type":"string"}}
            }
        }]});
        assert!(navigation_supported(&catalog));
        catalog["tools"][0]["inputSchema"]["required"] = serde_json::json!(["threadId", "hostId"]);
        assert!(!navigation_supported(&catalog));
        catalog["tools"][0]["inputSchema"]["required"] = serde_json::json!(["threadId"]);
        catalog["tools"][0]["namespace"] = serde_json::json!("other_app");
        assert!(!navigation_supported(&catalog));
        assert!(!navigation_supported(&serde_json::json!({"tools":[]})));
    }
    #[test]
    fn message_models_require_both_tool_schemas_and_share_supported_efforts() {
        let tool = |name, efforts| {
            serde_json::json!({"namespace":"codex_app","name":name,"inputSchema":{"properties":{
                "model":{"type":"string"},"thinking":{"type":"string","enum":efforts}
            }}})
        };
        let mut catalog = serde_json::json!({"tools":[tool("create_thread", vec!["low","high"]),tool("send_message_to_thread", vec!["high","max"])]});
        assert_eq!(model_efforts(&catalog), Some(vec!["high".into()]));
        catalog["tools"][1]["inputSchema"]["properties"]["thinking"]["enum"] =
            serde_json::json!(["ultra"]);
        assert!(model_efforts(&catalog).is_none());
        catalog["tools"][1]["inputSchema"]["properties"]["model"] = serde_json::Value::Null;
        assert!(model_efforts(&catalog).is_none());
        assert!(model_efforts(&serde_json::json!({"tools":[]})).is_none());
    }
    #[test]
    fn changed_process_identity_and_pipe_generation_invalidate_the_old_owner() {
        let process = ProcessIdentity::current().unwrap();
        let owner = NativeOwner {
            pipe: "pipe-one".into(),
            desktop: process.clone(),
            server: process,
        };
        assert!(owner.validate().is_ok());
        let mut changed = owner.clone();
        changed.server.started.push('0');
        assert!(changed.validate().is_err());
        assert_ne!(owner.instance_id(), changed.instance_id());
        let mut changed = owner.clone();
        changed.pipe = "pipe-two".into();
        assert_ne!(owner.instance_id(), changed.instance_id());
        assert!(!supported_catalog(&serde_json::json!({"tools":[]})));
    }
}
