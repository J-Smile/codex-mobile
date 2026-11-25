use crate::{BridgeError, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Paths {
    pub codex_home: PathBuf,
    pub data_home: PathBuf,
    pub web_root: PathBuf,
    pub runtime_home: PathBuf,
}

impl Paths {
    pub fn from_env(web_root: PathBuf) -> Result<Self> {
        let home =
            dirs::home_dir().ok_or_else(|| BridgeError::unavailable("无法找到用户目录。"))?;
        let data_home = std::env::var_os("CODEX_MOBILE_HOME")
            .map(PathBuf::from)
            .unwrap_or(home.join(".codex-mobile"));
        let codex_home = std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or(home.join(".codex"));
        let runtime_home = std::env::var_os("CODEX_MOBILE_RUNTIME_DIRECTORY")
            .map(PathBuf::from)
            .unwrap_or(data_home.join("shared-runtime"));
        fs::create_dir_all(&data_home)?;
        Ok(Self {
            codex_home,
            data_home,
            web_root,
            runtime_home,
        })
    }
}

pub fn read_json_optional(path: &Path) -> Result<Option<serde_json::Value>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp",
        path.extension().unwrap_or_default().to_string_lossy()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, path)?;
    Ok(())
}

pub fn normalized(path: &Path) -> String {
    let value = path
        .to_string_lossy()
        .replace(r"\\?\UNC\", r"\\")
        .replace(r"\\?\", "");
    if cfg!(windows) {
        value.replace('/', r"\").to_lowercase()
    } else {
        value
    }
}
