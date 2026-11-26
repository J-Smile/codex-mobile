use crate::{
    paths::{normalized, read_json_optional, write_private, Paths},
    process_identity::{parent_pid, ProcessIdentity},
    BridgeError, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
#[cfg(windows)]
#[path = "windows_start.rs"]
mod windows_start;

pub const CONTRACT: &str = "local-desktop-0.171.0-alpha.13-stdio-v2";
// Windows 已验收同实例官方工具、stdio、包身份和沙箱；其他平台仍只透传。
// 没有配置或环境变量绕过平台验收。
pub const SHARED_RUNTIME_VERIFIED: bool = cfg!(windows);
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeInfo {
    pub format: u32,
    pub instance_id: String,
    pub host: String,
    pub contract: String,
    pub state: String,
    pub url: String,
    pub auth_token: String,
    pub desktop: ProcessIdentity,
    pub server: ProcessIdentity,
    pub monitor: ProcessIdentity,
    pub codex_home: PathBuf,
    pub cli_path: PathBuf,
}
impl RuntimeInfo {
    pub fn path(&self, directory: &Path) -> PathBuf {
        directory
            .join("instances")
            .join(format!("{}.json", self.instance_id))
    }
    pub fn publish(&self, directory: &Path) -> Result<()> {
        write_private(&self.path(directory), &serde_json::to_vec(self)?)
    }
    pub fn validate(&self, directory: &Path) -> Result<()> {
        let current = read_runtime(directory)?
            .ok_or_else(|| BridgeError::unavailable("桌面共享连接已退役。"))?;
        if current != *self {
            return Err(BridgeError::unavailable("桌面运行时已变化，请重新同步。"));
        }
        Ok(())
    }
}
pub fn process_alive(pid: u32) -> bool {
    ProcessIdentity::read(pid).is_some()
}

pub fn read_runtime(directory: &Path) -> Result<Option<RuntimeInfo>> {
    // 旧记录不参与接入，也不以新格式改写旧运行时的身份。
    if let Some(legacy) = read_json_optional(&directory.join("runtime.json"))? {
        if legacy["pid"]
            .as_u64()
            .is_some_and(|pid| pid <= u32::MAX as u64 && process_alive(pid as u32))
        {
            return Err(BridgeError::unavailable(
                "旧共享运行时仍在运行，请完成旧任务并退出旧版连接后再打开 Codex。",
            ));
        }
    }
    let instances = directory.join("instances");
    if !instances.exists() {
        return Ok(None);
    }
    let mut current = None;
    for entry in std::fs::read_dir(instances)? {
        let path = entry?.path();
        if path.extension().is_none_or(|ext| ext != "json") {
            continue;
        }
        let record: RuntimeInfo = serde_json::from_slice(&std::fs::read(&path)?).map_err(|_| {
            BridgeError::unavailable("共享记录契约不受支持，请使用同一版本重新打开桌面。")
        })?;
        if record.format != 2
            || record.host != "local"
            || record.contract != CONTRACT
            || uuid::Uuid::parse_str(&record.instance_id).is_err()
            || record.path(directory) != path
            || !record.codex_home.is_absolute()
            || !record.cli_path.is_absolute()
            || record.auth_token.len() != 43
        {
            return Err(BridgeError::unavailable("共享记录身份无效。"));
        }
        if !record.server.alive() {
            continue;
        }
        let endpoint =
            url::Url::parse(&record.url).map_err(|_| BridgeError::unavailable("共享地址无效。"))?;
        if endpoint.scheme() != "ws"
            || endpoint.host_str() != Some("127.0.0.1")
            || endpoint.port().is_none()
            || endpoint.path() != "/"
            || !endpoint.username().is_empty()
            || endpoint.password().is_some()
            || endpoint.query().is_some()
            || endpoint.fragment().is_some()
        {
            return Err(BridgeError::unavailable("共享地址无效。"));
        }
        if record.state != "ready" || !record.desktop.alive() || !record.monitor.alive() {
            return Err(BridgeError::unavailable(
                "旧桌面已退出，运行时任务状态待确认；共享控制已停止。",
            ));
        }
        if current.is_some() {
            return Err(BridgeError::unavailable(
                "存在多个本机共享实例，已停止手机控制。",
            ));
        }
        current = Some(record);
    }
    Ok(current)
}

fn usable_cli(path: &Path, current: &Path) -> bool {
    path.is_absolute()
        && path.is_file()
        && std::fs::canonicalize(path)
            .ok()
            .zip(std::fs::canonicalize(current).ok())
            .is_some_and(|(a, b)| normalized(&a) != normalized(&b))
}
fn original_cli() -> Result<PathBuf> {
    let current = std::env::current_exe()?;
    let binding = current.parent().and_then(|directory| {
        read_json_optional(&directory.join("proxy-cli.json"))
            .ok()
            .flatten()
    });
    let cli = binding
        .as_ref()
        .and_then(|v| v["cliPath"].as_str())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_MOBILE_ORIGINAL_CLI").map(PathBuf::from));
    cli.filter(|cli| usable_cli(cli, &current))
        .ok_or_else(|| BridgeError::unavailable("代理没有本轮桌面 CLI 绑定。"))
}
pub fn main_app_server(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "app-server")
        && !args.iter().any(|arg| {
            [
                "--help",
                "-h",
                "--version",
                "-V",
                "--print-startup-config",
                "--listen",
                "daemon",
                "proxy",
                "generate-ts",
                "generate-json-schema",
                "help",
            ]
            .contains(&arg.as_str())
                || arg.starts_with("--listen=")
        })
}
fn desktop_owner() -> Option<ProcessIdentity> {
    let expected = std::env::var_os("CODEX_MOBILE_DESKTOP_PATH").map(PathBuf::from)?;
    find_desktop_owner(
        &expected,
        parent_pid(std::process::id())?,
        ProcessIdentity::read,
        parent_pid,
    )
}
pub(crate) fn find_desktop_owner(
    expected: &Path,
    mut pid: u32,
    read: impl Fn(u32) -> Option<ProcessIdentity>,
    parent: impl Fn(u32) -> Option<u32>,
) -> Option<ProcessIdentity> {
    let mut owner = None;
    for _ in 0..8 {
        let Some(process) = read(pid) else {
            // 启动器可能已经退出，不能因此丢弃已确认的桌面祖先进程。
            break;
        };
        if normalized(&process.executable) != normalized(expected) {
            break;
        }
        owner = Some(process);
        let Some(parent) = parent(pid).filter(|parent| *parent > 0 && *parent != pid) else {
            break;
        };
        pid = parent;
    }
    owner
}
pub fn local_initialize(message: &Value) -> bool {
    message["method"] == "initialize"
        && (message["id"].is_string() || message["id"].is_i64())
        && message["params"]["capabilities"]["explicitGatewayOauth"] == true
}
pub fn supported_initialize(message: &Value) -> bool {
    message["userAgent"].as_str().is_some_and(|agent| {
        [
            "Codex Desktop/0.171.0-alpha.13 ",
            "Codex Desktop/0.171.0-alpha.13.2 ",
        ]
        .iter()
        .any(|version| agent.starts_with(version))
    })
}

pub fn proxy_main(args: Vec<String>) -> Result<i32> {
    #[cfg(unix)]
    {
        // 此平台尚未完成共享控制实机验收；保持官方签名 CLI 的父子关系和原始传输。
        use std::os::unix::process::CommandExt;
        let cli = original_cli()?;
        Err(std::process::Command::new(&cli)
            .args(args)
            .env("CODEX_CLI_PATH", &cli)
            .env("CODEX_APP_RUNTIME_CLI_PATH", &cli)
            .exec()
            .into())
    }
    #[cfg(windows)]
    {
        tokio::runtime::Runtime::new()?.block_on(proxy(args))
    }
}
pub async fn proxy(args: Vec<String>) -> Result<i32> {
    let cli = original_cli()?;
    let mut command = tokio::process::Command::new(&cli);
    command
        .env("CODEX_CLI_PATH", &cli)
        .env("CODEX_APP_RUNTIME_CLI_PATH", &cli)
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let owner = desktop_owner();
    #[cfg(windows)]
    let owner = owner.filter(|desktop| {
        windows_start::package(desktop.pid).is_some_and(|package| {
            windows_start::package(std::process::id()).as_ref() == Some(&package)
        })
    });
    if !SHARED_RUNTIME_VERIFIED || !main_app_server(&args) || owner.is_none() {
        return Ok(command
            .args(args)
            .stdin(std::process::Stdio::inherit())
            .spawn()?
            .wait()
            .await?
            .code()
            .unwrap_or(1));
    }
    let mut input = BufReader::new(tokio::io::stdin()).lines();
    let Some(first) = input.next_line().await? else {
        return Ok(0);
    };
    let message: Value = serde_json::from_str(&first).unwrap_or(Value::Null);
    if !local_initialize(&message) {
        // 首条消息已经读取，原样投递一次；未知来源不发布共享端点。
        let mut child = command
            .args(args)
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        let mut writer = child.stdin.take().unwrap();
        writer.write_all(format!("{first}\n").as_bytes()).await?;
        loop {
            tokio::select! {
                status = child.wait() => return Ok(status?.code().unwrap_or(1)),
                line = input.next_line() => match line? { Some(line) => writer.write_all(format!("{line}\n").as_bytes()).await?, None => break }
            }
        }
        drop(writer);
        return Ok(child.wait().await?.code().unwrap_or(1));
    }
    let paths = Paths::from_env(PathBuf::new())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://127.0.0.1:{}", listener.local_addr()?.port());
    let codex_home = std::fs::canonicalize(&paths.codex_home)?;
    let monitor = ProcessIdentity::current()?;
    let mut child = command
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    let Some(server) = ProcessIdentity::read(child.id().unwrap()) else {
        child.kill().await?;
        return Err(BridgeError::unavailable("无法核对本轮 App Server。"));
    };
    #[cfg(windows)]
    if windows_start::package(server.pid) != windows_start::package(owner.as_ref().unwrap().pid) {
        child.kill().await?;
        return Err(BridgeError::unavailable(
            "官方核心未继承桌面的 Windows 包身份，初始化尚未发送。",
        ));
    }
    let mut record = RuntimeInfo {
        format: 2,
        instance_id: uuid::Uuid::new_v4().to_string(),
        host: "local".into(),
        contract: CONTRACT.into(),
        state: "starting".into(),
        url: url.clone(),
        auth_token: crate::auth::random_token(),
        desktop: owner.unwrap(),
        server,
        monitor,
        codex_home,
        cli_path: cli,
    };
    if let Err(error) = record.publish(&paths.runtime_home) {
        child.kill().await?;
        return Err(error);
    }
    let upstream = crate::transport::StdioPipe {
        reader: child.stdout.take().unwrap(),
        writer: child.stdin.take().unwrap(),
    };
    // 保留原始 CLI 参数及 stdio；共享发生在代理内部，而不是改变官方后端传输。
    let result = crate::broker::run(
        input.into_inner(),
        tokio::io::stdout(),
        Box::new(upstream),
        listener,
        &mut record,
        paths.runtime_home.clone(),
        &first,
    )
    .await;
    // 所有 I/O 和握手错误都走退役路径；旧监护只写自己的记录。

    record.state = "retired".into();
    let retired = record.publish(&paths.runtime_home);
    // 已可能投递请求，不重启、不强杀；stdin EOF 沿用官方生命周期。
    let status = child.wait().await?;
    std::fs::remove_file(record.path(&paths.runtime_home))?;
    retired?;
    result?;
    Ok(status.code().unwrap_or(1))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRuntime {
    pub desktop_path: PathBuf,
    pub cli_path: Option<PathBuf>,
    pub running: bool,
}

pub fn discover_desktop(paths: &Paths) -> Result<Option<DesktopRuntime>> {
    #[cfg(windows)]
    let records: Vec<Value> = {
        let script="ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_Process -Filter \"Name = 'ChatGPT.exe' OR Name = 'Codex.exe'\" | Select-Object @{n='pid';e={$_.ProcessId}},@{n='parentPid';e={$_.ParentProcessId}},@{n='path';e={$_.ExecutablePath}})";
        let output = hidden_powershell(script)?;
        serde_json::from_slice(&output)?
    };
    #[cfg(unix)]
    let records: Vec<Value> = {
        let output = std::process::Command::new("ps")
            .args(["-axo", "pid=,ppid=,comm="])
            .output()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| {
                let mut split = line.split_whitespace();
                let pid = split.next()?.parse::<u32>().ok()?;
                let parent = split.next()?.parse::<u32>().ok()?;
                let path = split.collect::<Vec<_>>().join(" ");
                Some(json!({"pid":pid,"parentPid":parent,"path":path}))
            })
            .collect()
    };
    discover_desktop_from_processes(paths, &records)
}

fn discover_desktop_from_processes(
    paths: &Paths,
    records: &[Value],
) -> Result<Option<DesktopRuntime>> {
    let cached = read_json_optional(&paths.runtime_home.join("desktop.json"))?;
    let apps: Vec<&Value> = records
        .iter()
        .filter(|r| {
            r["path"].as_str().is_some_and(|p| {
                let path = Path::new(p);
                // app.asar 是 Electron 的通用文件，必须先核对 Codex 主程序名称。
                path.file_stem().is_some_and(|name| {
                    name.eq_ignore_ascii_case("Codex") || name.eq_ignore_ascii_case("ChatGPT")
                }) && path.parent().is_some_and(|dir| {
                    dir.join("resources/app.asar").exists()
                        // macOS 主程序位于 MacOS 目录；Resources 内的 CLI 不能当作桌面。
                        || (dir.file_name().is_some_and(|name| name == "MacOS")
                            && dir.join("../Resources/app.asar").exists())
                })
            })
        })
        .collect();
    let desktop = apps
        .first()
        .and_then(|r| r["path"].as_str())
        .map(PathBuf::from)
        .or_else(|| {
            cached
                .as_ref()
                .and_then(|c| c["desktopPath"].as_str())
                .map(PathBuf::from)
                .filter(|p| p.is_file())
        });
    let desktop = match desktop {
        Some(desktop) => Some(desktop),
        None => {
            #[cfg(windows)]
            {
                let bytes=hidden_powershell("ConvertTo-Json -Compress -InputObject @(Get-AppxPackage -Name OpenAI.Codex | Select-Object -ExpandProperty InstallLocation)")?;
                let locations: Vec<String> = serde_json::from_slice(&bytes)?;
                locations
                    .iter()
                    .flat_map(|location| {
                        ["ChatGPT.exe", "Codex.exe"]
                            .map(|name| Path::new(location).join("app").join(name))
                    })
                    .find(|p| p.is_file())
            }
            #[cfg(unix)]
            {
                [
                    "/Applications/Codex.app/Contents/MacOS/Codex",
                    "/Applications/ChatGPT.app/Contents/MacOS/ChatGPT",
                ]
                .iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
            }
        }
    };
    let Some(desktop_path) = desktop else {
        return Ok(None);
    };
    let running = apps.iter().any(|r| {
        r["path"]
            .as_str()
            .is_some_and(|p| Path::new(p) == desktop_path)
    });
    let bundle = desktop_path
        .ancestors()
        .find(|path| path.extension().is_some_and(|e| e == "app"));
    let app_ids = records
        .iter()
        .filter(|record| {
            record["path"].as_str().is_some_and(|p| {
                Path::new(p) == desktop_path
                    || (running && bundle.is_some_and(|bundle| Path::new(p).starts_with(bundle)))
            })
        })
        .filter_map(|r| r["pid"].as_u64())
        .collect::<Vec<_>>();
    let cli = records
        .iter()
        .find(|r| {
            r["parentPid"]
                .as_u64()
                .is_some_and(|id| app_ids.contains(&id))
                && r["path"].as_str().is_some_and(|p| {
                    Path::new(p) != desktop_path
                        && Path::new(p)
                            .file_stem()
                            .is_some_and(|s| s.eq_ignore_ascii_case("codex"))
                })
        })
        .and_then(|r| r["path"].as_str())
        .map(PathBuf::from);
    let stamp = std::fs::metadata(&desktop_path)?
        .modified()?
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let cli_path = cli
        .or_else(|| {
            cached
                .as_ref()
                .filter(|c| {
                    c["desktopPath"]
                        .as_str()
                        .is_some_and(|p| Path::new(p) == desktop_path)
                        && c["desktopStamp"]
                            .as_f64()
                            .is_some_and(|t| t.floor() as u64 == stamp)
                })
                .and_then(|c| c["cliPath"].as_str())
                .map(PathBuf::from)
                .filter(|p| p.is_file())
        })
        .or_else(|| bundled_cli_path(&desktop_path));
    if let Some(cli) = &cli_path {
        write_private(
            &paths.runtime_home.join("desktop.json"),
            &serde_json::to_vec(
                &json!({"desktopPath":desktop_path,"desktopStamp":stamp,"cliPath":cli}),
            )?,
        )?;
    }
    Ok(Some(DesktopRuntime {
        desktop_path,
        cli_path,
        running,
    }))
}

fn bundled_cli_path(desktop: &Path) -> Option<PathBuf> {
    let directory = desktop.parent()?;
    #[cfg(windows)]
    let path = directory.join("resources/codex.exe");
    #[cfg(unix)]
    let path = directory.parent()?.join("Resources/codex");
    path.is_file().then_some(path)
}

#[cfg(windows)]
fn hidden_powershell(script: &str) -> Result<Vec<u8>> {
    use std::os::windows::process::CommandExt;
    let output = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(0x08000000)
        .output()?;
    if !output.status.success() {
        return Err(BridgeError::unavailable("无法识别本机 Codex 桌面程序。"));
    }
    Ok(output.stdout)
}

pub fn prepare_proxy(directory: &Path, source: &Path, cli: &Path) -> Result<PathBuf> {
    if !usable_cli(cli, source) {
        return Err(BridgeError::unavailable("找不到桌面配套的 Codex CLI。"));
    }
    std::fs::create_dir_all(directory)?;
    let name = if cfg!(windows) {
        "codex-mobile-proxy.exe"
    } else {
        "codex-mobile-proxy"
    };
    let binding = directory
        .join("bindings")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&binding)?;
    let proxy = binding.join(name);
    // A physical copy keeps current_exe beside this runtime's binding.
    std::fs::copy(source, &proxy)?;
    write_private(
        &binding.join("proxy-cli.json"),
        &serde_json::to_vec(&json!({"cliPath":cli}))?,
    )?;
    Ok(proxy)
}

pub async fn prepare_desktop(paths: &Paths, binaries: &Path) -> Result<Value> {
    let desktop = discover_desktop(paths)?
        .ok_or_else(|| BridgeError::unavailable("请先安装官方 Codex 桌面程序。"))?;
    if desktop.running {
        // 调用前已实测工具连接失败，旧运行时存活不能证明当前桌面已接入。
        return Ok(
            json!({"status":if SHARED_RUNTIME_VERIFIED { "restart-required" } else { "history-only" },"desktop":desktop}),
        );
    }
    let cli = desktop.cli_path.as_ref().ok_or_else(|| {
        BridgeError::unavailable("找不到桌面配套的 Codex CLI，请更新或重新安装官方 Codex。")
    })?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let proxy_source = binaries.join(format!("codex-mobile-proxy{suffix}"));
    if !proxy_source.is_file() {
        return Err(BridgeError::unavailable(
            "桌面运行包缺少 Rust 运行时组件，请重新安装完整版本。",
        ));
    }
    read_runtime(&paths.runtime_home)?;
    let mut command = std::process::Command::new(&desktop.desktop_path);
    command
        .current_dir(desktop.desktop_path.parent().unwrap())
        .env_remove("CODEX_APP_SERVER_FORCE_CLI")
        .env_remove("CODEX_CLI_PATH")
        .env_remove("CODEX_APP_RUNTIME_CLI_PATH")
        .env_remove("CODEX_MOBILE_ORIGINAL_CLI")
        .env_remove("CODEX_MOBILE_DESKTOP_PATH")
        .env_remove("CODEX_MOBILE_RUNTIME_DIRECTORY")
        .env_remove("CODEX_MOBILE_PROXY_NODE")
        .env_remove("CODEX_MOBILE_PROXY_SCRIPT")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if SHARED_RUNTIME_VERIFIED {
        let proxy = prepare_proxy(&paths.runtime_home, &proxy_source, cli)?;
        command
            .env("CODEX_CLI_PATH", proxy)
            .env("CODEX_MOBILE_ORIGINAL_CLI", cli)
            .env("CODEX_MOBILE_DESKTOP_PATH", &desktop.desktop_path)
            .env("CODEX_MOBILE_RUNTIME_DIRECTORY", &paths.runtime_home);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000 | 0x00000200);
    }
    #[cfg(windows)]
    if SHARED_RUNTIME_VERIFIED {
        let alias = dirs::data_local_dir()
            .map(|directory| directory.join("Microsoft/WindowsApps/chatgpt-desktop.exe"))
            .filter(|path| path.exists())
            .ok_or_else(|| {
                BridgeError::unavailable("未找到官方 Windows 启动别名，请修复官方桌面安装。")
            })?;
        let installed = hidden_powershell(
            "Get-AppxPackage -Name OpenAI.Codex | Select-Object -ExpandProperty InstallLocation",
        )?;
        let matches = String::from_utf8_lossy(&installed).lines().any(|location| {
            let location = Path::new(location.trim());
            desktop
                .desktop_path
                .strip_prefix(location)
                .is_ok_and(|path| path == Path::new("app/ChatGPT.exe"))
        });
        if !matches {
            return Err(BridgeError::unavailable("启动别名与当前官方安装包不匹配。"));
        }
        windows_start::launch(&command, &alias)?;
    } else {
        command.spawn()?;
    }
    #[cfg(not(windows))]
    command.spawn()?;
    Ok(
        json!({"status":if SHARED_RUNTIME_VERIFIED { "starting" } else { "history-only" },"desktop":desktop}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_owner_survives_exited_launcher_and_stops_at_other_apps() {
        let executable = PathBuf::from("official/ChatGPT.exe");
        let identity = |pid, executable: PathBuf| ProcessIdentity {
            pid,
            started: pid.to_string(),
            executable,
        };
        let desktop = identity(20, executable.clone());
        let helper = identity(30, executable.clone());
        let launcher = identity(10, PathBuf::from("launcher/node.exe"));
        for exited in [false, true] {
            let owner = find_desktop_owner(
                &executable,
                30,
                |pid| match pid {
                    30 => Some(helper.clone()),
                    20 => Some(desktop.clone()),
                    10 if !exited => Some(launcher.clone()),
                    _ => None,
                },
                |pid| Some(pid - 10),
            );
            assert_eq!(owner, Some(desktop.clone()));
        }
        assert!(
            find_desktop_owner(&executable, 10, |_| Some(launcher.clone()), |_| Some(20)).is_none()
        );
        assert!(find_desktop_owner(&executable, 30, |_| None, |_| Some(20)).is_none());
    }

    #[test]
    fn desktop_discovery_ignores_a_leftover_cli_before_and_after_desktop_exit() {
        for (desktop, cli, archive) in [
            (
                "app/ChatGPT.exe",
                "app/resources/codex.exe",
                "app/resources/app.asar",
            ),
            (
                "Codex.app/Contents/MacOS/Codex",
                "Codex.app/Contents/Resources/codex",
                "Codex.app/Contents/Resources/app.asar",
            ),
        ] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path();
            let paths = Paths {
                codex_home: root.join("codex"),
                data_home: root.join("mobile"),
                web_root: root.join("web"),
                runtime_home: root.join("runtime"),
            };
            let desktop = root.join(desktop);
            let cli = root.join(cli);
            for file in [&desktop, &cli, &root.join(archive)] {
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, "fixture").unwrap();
            }
            let records = vec![
                json!({"pid":10,"parentPid":1,"path":cli}),
                json!({"pid":20,"parentPid":1,"path":desktop}),
                json!({"pid":21,"parentPid":20,"path":cli}),
            ];
            let running = discover_desktop_from_processes(&paths, &records)
                .unwrap()
                .unwrap();
            assert_eq!(running.desktop_path, desktop);
            assert_eq!(running.cli_path, Some(cli.clone()));
            assert!(running.running);

            let stopped = discover_desktop_from_processes(&paths, &records[..1])
                .unwrap()
                .unwrap();
            assert_eq!(stopped.desktop_path, desktop);
            assert_eq!(stopped.cli_path, Some(cli));
            assert!(!stopped.running);
        }
    }

    #[test]
    fn desktop_discovery_uses_bundled_cli_without_a_cached_cli() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        let paths = Paths {
            codex_home: root.join("codex"),
            data_home: root.join("mobile"),
            web_root: root.join("web"),
            runtime_home: root.join("runtime"),
        };
        #[cfg(windows)]
        let (desktop, cli) = ("app/ChatGPT.exe", "app/resources/codex.exe");
        #[cfg(unix)]
        let (desktop, cli) = ("Contents/MacOS/Codex", "Contents/Resources/codex");
        let desktop = root.join(desktop);
        let cli = root.join(cli);
        for executable in [&desktop, &cli] {
            std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
            std::fs::write(executable, "fixture").unwrap();
        }
        write_private(
            &paths.runtime_home.join("desktop.json"),
            &serde_json::to_vec(&json!({"desktopPath":desktop})).unwrap(),
        )
        .unwrap();

        let found = discover_desktop_from_processes(&paths, &[])
            .unwrap()
            .unwrap();
        assert_eq!(found.desktop_path, desktop);
        assert_eq!(found.cli_path, Some(cli));
        assert!(!found.running);
    }

    #[test]
    fn desktop_discovery_ignores_other_electron_apps_before_codex() {
        for executable in ["ChatGPT", "Codex", "ChatGPT.exe", "Codex.exe"] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path();
            let paths = Paths {
                codex_home: root.join("codex"),
                data_home: root.join("mobile"),
                web_root: root.join("web"),
                runtime_home: root.join("runtime"),
            };
            let bundle = root.join("Official.app");
            let desktop = bundle.join("Contents/MacOS").join(executable);
            let terminal = root.join("XTerminal.app/Contents/MacOS/XTerminal");
            let docker = root.join("Docker Desktop.app/Contents/MacOS/Docker Desktop");
            for application in [&terminal, &docker, &desktop] {
                let parent = application.parent().unwrap();
                std::fs::create_dir_all(parent).unwrap();
                std::fs::create_dir_all(parent.join("../Resources")).unwrap();
                std::fs::write(application, "fixture").unwrap();
                std::fs::write(parent.join("../Resources/app.asar"), "fixture").unwrap();
            }
            let helper =
                bundle.join("Contents/Frameworks/Codex Helper.app/Contents/MacOS/Codex Helper");
            let cli = bundle.join("Contents/Resources/codex");
            std::fs::write(&cli, "fixture").unwrap();
            let records = vec![
                json!({"pid":10,"parentPid":1,"path":terminal}),
                json!({"pid":11,"parentPid":1,"path":docker}),
                json!({"pid":20,"parentPid":1,"path":desktop}),
                json!({"pid":21,"parentPid":20,"path":helper}),
                json!({"pid":22,"parentPid":21,"path":cli}),
            ];
            let found = discover_desktop_from_processes(&paths, &records)
                .unwrap()
                .unwrap();
            assert_eq!(found.desktop_path, desktop);
            assert_eq!(found.cli_path, Some(cli.clone()));
            assert!(found.running);

            // Codex 退出后，其他 Electron 应用不能覆盖已识别的配套路径。
            let stopped = discover_desktop_from_processes(&paths, &records[..2])
                .unwrap()
                .unwrap();
            assert_eq!(stopped.desktop_path, desktop);
            assert_eq!(stopped.cli_path, Some(cli));
            assert!(!stopped.running);
        }
    }
}
