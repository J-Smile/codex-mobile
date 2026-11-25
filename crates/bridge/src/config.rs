use crate::{
    paths::{normalized, read_json_optional, write_private},
    BridgeError, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];
pub fn valid_model(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 150
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:/-".contains(&c))
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MessageModel {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}
impl MessageModel {
    pub fn parse(input: &Value, efforts: &[String]) -> Result<Self> {
        let model = input
            .get("model")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| valid_model(value))
                    .map(str::to_owned)
                    .ok_or_else(|| BridgeError::invalid("请输入有效的模型 ID。").before_send())
            })
            .transpose()?;
        let thinking = input
            .get("thinking")
            .map(|value| {
                value
                    .as_str()
                    .filter(|value| efforts.iter().any(|effort| effort == value))
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        BridgeError::invalid("请选择当前桌面支持的思考强度。").before_send()
                    })
            })
            .transpose()?;
        Ok(Self { model, thinking })
    }
    pub fn is_empty(&self) -> bool {
        self.model.is_none() && self.thinking.is_none()
    }
    pub fn apply(&self, arguments: &mut Value) {
        if let Some(model) = &self.model {
            arguments["model"] = json!(model);
        }
        if let Some(thinking) = &self.thinking {
            arguments["thinking"] = json!(thinking);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ServiceConfig {
    pub port: u16,
    pub bind: String,
    pub public_url: String,
    pub auto_start: bool,
}
impl Default for ServiceConfig {
    fn default() -> Self {
        Self {
            port: 4319,
            bind: "0.0.0.0".into(),
            public_url: String::new(),
            auto_start: true,
        }
    }
}
impl ServiceConfig {
    pub fn load(directory: &Path) -> Result<Self> {
        let mut value = read_json_optional(&directory.join("desktop-settings.json"))?
            .map(serde_json::from_value::<Self>)
            .transpose()?
            .unwrap_or_default();
        if let Ok(port) = std::env::var("CODEX_MOBILE_PORT") {
            value.port = port
                .parse()
                .map_err(|_| BridgeError::invalid("服务端口无效。"))?;
        }
        if let Ok(url) = std::env::var("CODEX_MOBILE_PUBLIC_URL") {
            value.public_url = url;
        }
        if let Ok(bind) = std::env::var("CODEX_MOBILE_HOST") {
            value.bind = bind;
        }
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<()> {
        if self.port == 0 || self.bind.parse::<std::net::IpAddr>().is_err() {
            return Err(BridgeError::invalid("服务地址或端口无效。"));
        }
        if !self.public_url.is_empty() {
            let url = url::Url::parse(&self.public_url)
                .map_err(|_| BridgeError::invalid("请输入完整的网站地址。"))?;
            if !matches!(url.scheme(), "http" | "https")
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(BridgeError::invalid(
                    "网站地址只能包含 HTTP(S) 地址，不要附带连接密钥。",
                ));
            }
        }
        Ok(())
    }
    pub fn save(&self, directory: &Path) -> Result<()> {
        self.validate()?;
        write_private(
            &directory.join("desktop-settings.json"),
            &serde_json::to_vec_pretty(self)?,
        )
    }
}

pub fn read_toml(path: &Path) -> Result<Option<toml::Value>> {
    match fs::read_to_string(path) {
        Ok(text) => toml::from_str(&text)
            .map(Some)
            .map_err(|_| BridgeError::unavailable("Codex 配置格式无效，请在电脑检查配置文件。")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn new_thread_defaults(codex_home: &Path, cwd: Option<&Path>) -> Result<Value> {
    let config = read_toml(&codex_home.join("config.toml"))?
        .unwrap_or(toml::Value::Table(Default::default()));
    let mut configs = vec![config.clone()];
    if let Some(cwd) = cwd {
        let target = normalized(cwd);
        let trust = config
            .get("projects")
            .and_then(toml::Value::as_table)
            .into_iter()
            .flatten()
            .filter_map(|(path, settings)| {
                let directory = normalized(Path::new(path));
                (Path::new(path).is_absolute()
                    && (target == directory
                        || target
                            .starts_with(&(directory.clone() + std::path::MAIN_SEPARATOR_STR))))
                .then(|| {
                    (
                        directory.len(),
                        settings
                            .get("trust_level")
                            .and_then(toml::Value::as_str)
                            .unwrap_or(""),
                    )
                })
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, trust)| trust)
            .unwrap_or("");
        if trust != "untrusted" {
            let root = cwd
                .ancestors()
                .find(|dir| dir.join(".git").exists())
                .unwrap_or(cwd);
            let mut files: Vec<PathBuf> = cwd
                .ancestors()
                .take_while(|dir| dir.starts_with(root))
                .map(|dir| dir.join(".codex/config.toml"))
                .collect();
            files.reverse();
            for file in files {
                if let Some(value) = read_toml(&file)? {
                    if trust != "trusted" {
                        return Err(BridgeError::unavailable(
                            "项目配置的信任状态尚未确认，创建后会显示实际模型。",
                        ));
                    }
                    configs.push(value);
                }
            }
        }
    }
    let mut model = None;
    let mut effort = None;
    for config in configs {
        if config.get("profile").is_some() {
            return Err(BridgeError::unavailable(
                "默认模型使用了配置档案，手机暂时无法确认；创建后会显示实际模型。",
            ));
        }
        if let Some(value) = config.get("model") {
            model = Some(value.clone());
        }
        if let Some(value) = config.get("model_reasoning_effort") {
            effort = Some(value.clone());
        }
    }
    let model = model
        .as_ref()
        .and_then(toml::Value::as_str)
        .filter(|v| valid_model(v))
        .ok_or_else(|| {
            BridgeError::unavailable("桌面未显式配置默认模型，创建后会显示实际模型。")
        })?;
    let effort = match effort {
        None => None,
        Some(value) => Some(
            value
                .as_str()
                .filter(|v| EFFORTS.contains(v))
                .ok_or_else(|| {
                    BridgeError::unavailable("默认思考强度配置无效，请在电脑检查配置文件。")
                })?
                .to_string(),
        ),
    };
    Ok(json!({"model":model,"effort":effort}))
}
