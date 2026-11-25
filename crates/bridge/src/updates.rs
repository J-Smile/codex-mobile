use crate::runtime::process_alive;
use crate::{
    paths::{normalized, read_json_optional, write_private},
    BridgeError, Result,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::RwLock;

#[cfg(windows)]
mod windows;

const WINDOWS_EXECUTABLE: &str = "Codex Mobile.exe";
const REPOSITORY: &str = "J-Smile/codex-mobile";
const RELEASE_URL: &str = "https://api.github.com/repos/J-Smile/codex-mobile/releases/latest";
pub struct Updates {
    directory: PathBuf,
    pub packaged: bool,
    client: reqwest::Client,
    pub release: RwLock<Option<ReleasePreview>>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleasePreview {
    pub version: String,
    pub notes: String,
    pub published_at: Option<String>,
}
fn release_name(version: &str) -> String {
    let platform = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    format!("codex-mobile-{version}-{platform}-{arch}")
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub root: PathBuf,
    pub version: String,
    pub executable: PathBuf,
    // Windows 换版时从归档复制到原有监听路径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive_executable: Option<PathBuf>,
}

impl Updates {
    pub fn new(directory: PathBuf, packaged: bool) -> Result<Self> {
        Ok(Self {
            directory,
            packaged,
            release: RwLock::new(None),
            client: reqwest::Client::builder()
                .user_agent("Codex-Mobile-Updater")
                .timeout(Duration::from_secs(15))
                .build()?,
        })
    }
    fn token(&self) -> Result<(String, Option<&'static str>)> {
        if let Some(value) = std::env::var("CODEX_MOBILE_GITHUB_TOKEN")
            .ok()
            .filter(|s| !s.trim().is_empty())
        {
            return Ok((value.trim().into(), Some("environment")));
        }
        let file = self.directory.join("github-token");
        if file.exists() {
            let value = std::fs::read_to_string(file)?.trim().to_string();
            if !value.is_empty() {
                return Ok((value, Some("file")));
            }
        }
        Ok((String::new(), None))
    }
    pub fn status(&self) -> Result<Value> {
        let (_, source) = self.token()?;
        Ok(
            json!({"configured":source.is_some(),"source":source,"version":env!("CARGO_PKG_VERSION"),"mode":if !self.packaged{"source"}else if std::env::var("CODEX_MOBILE_AUTO_UPDATE").as_deref()==Ok("0"){"paused"}else{"automatic"}}),
        )
    }
    fn request(&self, url: &str, token: &str, asset: bool) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .bearer_auth(token)
            .header(
                "Accept",
                if asset {
                    "application/octet-stream"
                } else {
                    "application/vnd.github+json"
                },
            )
            .header("X-GitHub-Api-Version", "2022-11-28")
    }
    pub async fn save(&self, value: &str) -> Result<Value> {
        if self.token()?.1 == Some("environment") {
            return Err(BridgeError::new(409,"当前使用电脑环境变量中的 token，请先移除 CODEX_MOBILE_GITHUB_TOKEN 并重启服务，再在网页配置。"));
        }
        let token = value.trim();
        if token.is_empty() || token.chars().any(char::is_whitespace) {
            return Err(BridgeError::invalid(
                "请输入有效的 GitHub token，不能包含空格或换行。",
            ));
        }
        let response = self
            .request(RELEASE_URL, token, false)
            .send()
            .await
            .map_err(|_| {
                BridgeError::new(502, "暂时无法连接 GitHub，未保存 token。请稍后重试。")
            })?;
        if !response.status().is_success() {
            return Err(BridgeError::new(400,format!("无法读取私有仓库的正式版本（HTTP {}）。请确认 token 有 J-Smile/codex-mobile 的 Contents: read 权限。",response.status().as_u16())));
        }
        write_private(
            &self.directory.join("github-token"),
            format!("{token}\n").as_bytes(),
        )?;
        self.status()
    }
    pub fn remove(&self) -> Result<Value> {
        if self.token()?.1 == Some("environment") {
            return Err(BridgeError::new(409,"当前 token 由电脑环境变量配置，请在电脑上移除 CODEX_MOBILE_GITHUB_TOKEN 并重启服务。"));
        }
        let path = self.directory.join("github-token");
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        self.status()
    }
    pub async fn prepare(&self, current: &str) -> Result<Option<Candidate>> {
        if !self.packaged {
            return Err(BridgeError::invalid(
                "源码运行不安装发布更新，请使用桌面安装包。",
            ));
        }
        let (token, source) = self.token()?;
        if source.is_none() {
            return Err(BridgeError::invalid(
                "请先在在线升级中配置私有仓库的 GitHub token。",
            ));
        }
        let response = self.request(RELEASE_URL, &token, false).send().await?;
        if !response.status().is_success() {
            return Err(BridgeError::unavailable(format!(
                "检查私有仓库版本失败（HTTP {}）。",
                response.status().as_u16()
            )));
        }
        let release: Value = response.json().await?;
        let version = release["tag_name"]
            .as_str()
            .unwrap_or("")
            .trim_start_matches('v');
        let candidate = semver::Version::parse(version)
            .map_err(|_| BridgeError::unavailable("正式版本编号无效。"))?;
        let current = semver::Version::parse(current)
            .map_err(|_| BridgeError::unavailable("当前版本编号无效。"))?;
        if release["draft"] == true || release["prerelease"] == true || !candidate.pre.is_empty() {
            return Ok(None);
        }
        *self.release.write().await = Some(ReleasePreview {
            version: version.into(),
            notes: release["body"].as_str().unwrap_or("").into(),
            published_at: release["published_at"].as_str().map(String::from),
        });
        if candidate <= current {
            return Ok(None);
        }
        if installed(&self.directory)?
            .is_some_and(|state| state.failed_version.as_deref() == Some(version))
        {
            return Err(BridgeError::unavailable(format!(
                "版本 {version} 上次启动失败，已保留当前版本；等待后续正式版本。"
            )));
        }
        let name = release_name(version);
        let archive_name = format!("{name}.zip");
        let asset = |name: &str| -> Result<String> {
            let a = release["assets"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|a| a["name"] == name)
                .ok_or_else(|| BridgeError::unavailable(format!("正式版本缺少更新文件：{name}")))?;
            let id = a["id"]
                .as_u64()
                .filter(|id| *id > 0)
                .ok_or_else(|| BridgeError::unavailable("更新文件编号无效。"))?;
            let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/assets/{id}");
            if a["url"] != url {
                return Err(BridgeError::unavailable("更新文件地址无效。"));
            }
            Ok(url)
        };
        let archive_url = asset(&archive_name)?;
        let checksum_url = asset(&format!("{archive_name}.sha256"))?;
        let checksum = self
            .request(&checksum_url, &token, true)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let pieces = checksum.split_whitespace().collect::<Vec<_>>();
        if pieces.len() != 2
            || pieces[1] != archive_name
            || pieces[0].len() != 64
            || !pieces[0].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(BridgeError::unavailable("更新校验文件格式不正确。"));
        }
        let versions = self.directory.join("updates/native");
        std::fs::create_dir_all(&versions)?;
        let target = versions.join(&name);
        if target.exists() {
            return read_candidate(&target, version).map(Some);
        }
        let temporary = versions.join(format!(".download-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&temporary)?;
        let archive = temporary.join(&archive_name);
        let response = self
            .request(&archive_url, &token, true)
            .timeout(Duration::from_secs(300))
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut file = std::fs::File::create(&archive)?;
        let mut hash = Sha256::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hash.update(&chunk);
            file.write_all(&chunk)?;
        }
        file.sync_all()?;
        drop(file);
        if hex::encode(hash.finalize()) != pieces[0].to_lowercase() {
            return Err(BridgeError::unavailable("更新包 SHA256 校验失败。"));
        }
        let unpacked = temporary.join("unpacked");
        std::fs::create_dir(&unpacked)?;
        let archive_file = std::fs::File::open(&archive)?;
        let mut zip = zip::ZipArchive::new(archive_file)
            .map_err(|_| BridgeError::unavailable("更新压缩包格式无效。"))?;
        for index in 0..zip.len() {
            let mut entry = zip
                .by_index(index)
                .map_err(|_| BridgeError::unavailable("无法读取更新压缩包。"))?;
            let relative = entry
                .enclosed_name()
                .ok_or_else(|| BridgeError::unavailable("更新包包含越界路径。"))?;
            if entry
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 == 0o120000)
            {
                return Err(BridgeError::unavailable("更新包不能包含符号链接。"));
            }
            let path = unpacked.join(relative);
            if entry.is_dir() {
                std::fs::create_dir_all(&path)?;
            } else {
                std::fs::create_dir_all(path.parent().unwrap())?;
                let mut output = std::fs::File::create(&path)?;
                std::io::copy(&mut entry, &mut output)?;
                #[cfg(unix)]
                if let Some(mode) = entry.unix_mode() {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode))?;
                }
            }
        }
        let candidate = unpacked.join(&name);
        read_candidate(&candidate, version)?;
        std::fs::rename(candidate, &target)?;
        // The temporary path was created directly under this updater's own directory.
        std::fs::remove_dir_all(&temporary)?;
        read_candidate(&target, version).map(Some)
    }
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Installation {
    pub current: Candidate,
    pub previous: Option<Candidate>,
    pub failed_version: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub parent_pid: u32,
    pub current: Candidate,
    pub candidate: Candidate,
    pub data_home: PathBuf,
    #[serde(default)]
    pub launch: LaunchOptions,
}
#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LaunchOptions {
    pub background: bool,
    pub resume_service: bool,
}
pub fn installed(data: &Path) -> Result<Option<Installation>> {
    read_json_optional(&data.join("updates/native/installed.json"))?
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}
fn save_installation(data: &Path, state: &Installation) -> Result<()> {
    write_private(
        &data.join("updates/native/installed.json"),
        &serde_json::to_vec_pretty(state)?,
    )
}
fn current_candidate(data: &Path) -> Result<Candidate> {
    let executable = std::env::current_exe()?;
    let current = Candidate {
        root: executable.parent().unwrap().to_path_buf(),
        version: env!("CARGO_PKG_VERSION").into(),
        executable,
        archive_executable: None,
    };
    #[cfg(windows)]
    let current = windows::archive_current(data, current)?;
    #[cfg(not(windows))]
    let _ = data;
    Ok(current)
}

#[cfg(windows)]
pub use windows::resource_root as windows_resource_root;

pub fn spawn_handoff(
    data: &Path,
    binaries: &Path,
    candidate: Candidate,
    launch: LaunchOptions,
) -> Result<()> {
    let handoff = Handoff {
        parent_pid: std::process::id(),
        current: current_candidate(data)?,
        candidate,
        data_home: data.to_path_buf(),
        launch,
    };
    let plan = data.join("updates/native/handoff.json");
    write_private(&plan, &serde_json::to_vec(&handoff)?)?;
    let helper = binaries.join(if cfg!(windows) {
        "codex-mobile-updater.exe"
    } else {
        "codex-mobile-updater"
    });
    let mut command = std::process::Command::new(helper);
    command
        .arg(&plan)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    command.spawn()?;
    Ok(())
}
pub fn acknowledge(args: &[String]) -> Result<()> {
    let Some(index) = args.iter().position(|arg| arg == "--update-ready") else {
        return Ok(());
    };
    let path = args
        .get(index + 1)
        .ok_or_else(|| BridgeError::invalid("更新就绪参数不完整。"))?;
    let run = args
        .get(index + 2)
        .ok_or_else(|| BridgeError::invalid("更新就绪参数不完整。"))?;
    write_private(
        Path::new(path),
        &serde_json::to_vec(
            &json!({"run":run,"version":env!("CARGO_PKG_VERSION"),"pid":std::process::id()}),
        )?,
    )
}
async fn launch_ready(
    candidate: &Candidate,
    data: &Path,
    launch: LaunchOptions,
    updating: bool,
) -> Result<()> {
    require_boundary(&candidate.root)?;
    let run = uuid::Uuid::new_v4().to_string();
    let marker = data
        .join("updates/native")
        .join(format!("ready-{run}.json"));
    let mut command = tokio::process::Command::new(&candidate.executable);
    command
        .current_dir(&candidate.root)
        .arg("--update-ready")
        .arg(&marker)
        .arg(&run)
        .env("CODEX_MOBILE_HOME", data)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if launch.background {
        command.arg("--autostart");
    }
    if launch.resume_service {
        command.arg("--resume-service");
    }
    if updating {
        command.arg("--update-handoff");
    }
    // 后续关闭启动终端时，避免把新版桌面窗口随原进程组一起退出。
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command.spawn()?;
    for _ in 0..300 {
        let status = child.try_wait()?;
        match read_json_optional(&marker) {
            Ok(Some(value)) if value["run"] == run && value["version"] == candidate.version => {
                let _ = std::fs::remove_file(&marker);
                return Ok(());
            }
            Err(error) => {
                child.kill().await?;
                return Err(error);
            }
            _ => {}
        }
        if let Some(status) = status {
            return Err(BridgeError::unavailable(format!(
                "新版桌面程序在就绪前退出（{status}）。"
            )));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    child.kill().await?;
    Err(BridgeError::unavailable("桌面程序启动超时。"))
}
pub async fn apply_handoff(plan: &Path) -> Result<()> {
    let handoff: Handoff = serde_json::from_value(
        read_json_optional(plan)?.ok_or_else(|| BridgeError::invalid("找不到更新切换记录。"))?,
    )?;
    for _ in 0..300 {
        if !process_alive(handoff.parent_pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if process_alive(handoff.parent_pid) {
        return Err(BridgeError::unavailable("原连接服务尚未关闭，未切换版本。"));
    }
    let previous = installed(&handoff.data_home)?;
    #[cfg(windows)]
    let candidate = windows::with_listener(handoff.candidate, &handoff.current);
    #[cfg(not(windows))]
    let candidate = handoff.candidate;
    require_boundary(&candidate.root)?;
    #[cfg(windows)]
    let activation = windows::activate(&candidate);
    #[cfg(not(windows))]
    let activation: Result<()> = Ok(());
    #[cfg(windows)]
    let replaced = activation.is_ok() && candidate.archive_executable.is_some();
    let ready = match activation {
        Ok(()) => launch_ready(&candidate, &handoff.data_home, handoff.launch, true).await,
        Err(error) => Err(error),
    };
    match ready {
        Ok(()) => {
            #[cfg(windows)]
            let old = windows::as_archive(handoff.current);
            #[cfg(not(windows))]
            let old = handoff.current;
            save_installation(
                &handoff.data_home,
                &Installation {
                    current: candidate,
                    previous: Some(old),
                    failed_version: None,
                },
            )?;
            let error_file = handoff.data_home.join("updates/native/last-error.json");
            if error_file.exists() {
                std::fs::remove_file(error_file)?;
            }
        }
        Err(error) => {
            require_boundary(&handoff.current.root)?;
            let mut restored = previous.unwrap_or(Installation {
                current: handoff.current.clone(),
                previous: None,
                failed_version: None,
            });
            restored.current = handoff.current.clone();
            restored.failed_version = Some(candidate.version);
            #[cfg(windows)]
            if replaced {
                windows::activate(&handoff.current)?;
            }
            save_installation(&handoff.data_home, &restored)?;
            write_private(
                &handoff.data_home.join("updates/native/last-error.json"),
                &serde_json::to_vec(&json!({"message":error.message}))?,
            )?;
            launch_ready(&handoff.current, &handoff.data_home, handoff.launch, true)
                .await
                .map_err(|e| {
                    BridgeError::unavailable(format!("新版启动失败，旧版也未能恢复：{}", e.message))
                })?;
        }
    }
    std::fs::remove_file(plan)?;
    Ok(())
}
pub async fn redirect_installed(data: &Path, args: &[String]) -> Result<bool> {
    if args.iter().any(|arg| arg == "--update-ready") {
        return Ok(false);
    }
    let Some(mut state) = installed(data)? else {
        return Ok(false);
    };
    // 通过安装包手动安装的新版可以直接启动，避免被旧的在线升级记录拉回旧版。
    if semver::Version::parse(&state.current.version)
        .is_ok_and(|version| version < semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
    {
        return Ok(false);
    }
    let current = std::env::current_exe()?;
    if normalized(&current) == normalized(&state.current.executable) {
        return Ok(false);
    }
    let launch = LaunchOptions {
        background: args.iter().any(|arg| arg == "--autostart"),
        ..Default::default()
    };
    match launch_ready(&state.current, data, launch, false).await {
        Ok(()) => Ok(true),
        Err(error) => {
            let Some(previous) = state.previous.take() else {
                return Err(error);
            };
            require_boundary(&previous.root)?;
            #[cfg(windows)]
            let previous = windows::with_listener(previous, &state.current);
            #[cfg(windows)]
            windows::activate(&previous)?;
            state.failed_version = Some(state.current.version);
            state.current = previous;
            save_installation(data, &state)?;
            if normalized(&current) == normalized(&state.current.executable) {
                Ok(false)
            } else {
                launch_ready(&state.current, data, launch, false).await?;
                Ok(true)
            }
        }
    }
}
pub fn read_candidate(root: &Path, version: &str) -> Result<Candidate> {
    require_boundary(root)?;
    let value = read_json_optional(&root.join("release.json"))?
        .ok_or_else(|| BridgeError::unavailable("更新包缺少版本信息。"))?;
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    if value["backend"] != "rust"
        || value["version"] != version
        || value["platform"] != platform
        || value["arch"] != arch
    {
        return Err(BridgeError::unavailable("更新包的版本或平台不匹配。"));
    }
    let executable = if cfg!(windows) {
        root.join(WINDOWS_EXECUTABLE)
    } else if cfg!(target_os = "macos") {
        root.join("Codex Mobile.app/Contents/MacOS/codex-mobile-desktop")
    } else {
        root.join("codex-mobile-desktop")
    };
    if !executable.is_file() {
        return Err(BridgeError::unavailable("更新包缺少桌面程序。"));
    }
    let resources = if cfg!(target_os = "macos") {
        root.join("Codex Mobile.app/Contents/Resources")
    } else {
        root.to_path_buf()
    };
    if !resources.join("mobile/index.html").is_file() {
        return Err(BridgeError::unavailable("更新包缺少手机网页。"));
    }
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    for name in ["codex-mobile-proxy", "codex-mobile-updater"] {
        if !resources
            .join("bin")
            .join(format!("{name}{suffix}"))
            .is_file()
        {
            return Err(BridgeError::unavailable(format!(
                "更新包缺少运行组件：{name}"
            )));
        }
    }
    Ok(Candidate {
        root: root.into(),
        version: version.into(),
        executable,
        archive_executable: None,
    })
}

fn require_boundary(root: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let root = if root.file_name().is_some_and(|name| name == "MacOS") {
        root.parent().unwrap().join("Resources")
    } else {
        root.to_path_buf()
    };
    if read_json_optional(&root.join("release.json"))?
        .is_none_or(|value| value["runtimeBoundary"] != 2)
    {
        return Err(BridgeError::unavailable(
            "该运行包未声明新的连接边界，禁止自动启动或回退。请安装经过验收的新版本。",
        ));
    }
    Ok(())
}
