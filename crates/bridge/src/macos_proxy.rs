//! 保留官方签名 CLI 的 PID、父链和 stdio；旁路进程复用两端共有的 broker。
use super::*;
use std::{
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{net::UnixStream, process::CommandExt},
    },
    process::{Command, Stdio},
    time::Duration,
};

const BROKER_FD: i32 = 3;

pub(super) fn run(args: Vec<String>) -> Result<i32> {
    if args.first().is_some_and(|arg| arg == "--stdio-broker") {
        return tokio::runtime::Runtime::new()?.block_on(serve());
    }
    let cli = original_cli()?;
    let mut command = Command::new(&cli);
    command
        .args(&args)
        .env("CODEX_CLI_PATH", &cli)
        .env("CODEX_APP_RUNTIME_CLI_PATH", &cli);
    let mut monitor = None;
    if main_app_server(&args) && desktop_owner().is_some() {
        let (core, broker) = UnixStream::pair()?;
        let fd = broker.as_raw_fd();
        let mut child = Command::new(std::env::current_exe()?);
        child.arg("--stdio-broker");
        // 只把专用 IPC 描述符交给旁路进程；官方 stdin/stdout 由其原样承接。
        unsafe {
            child.pre_exec(move || {
                if libc::dup2(fd, BROKER_FD) == -1 || libc::fcntl(BROKER_FD, libc::F_SETFD, 0) == -1
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        monitor = Some(child.spawn()?);
        command
            .stdin(Stdio::from(OwnedFd::from(core.try_clone()?)))
            .stdout(Stdio::from(OwnedFd::from(core)));
    }
    // exec 不增加中间父进程，官方 MCP/沙箱仍看到原始桌面 -> 签名 CLI 链。
    let error = command.exec();
    if let Some(mut child) = monitor {
        let _ = child.kill();
        let _ = child.wait();
    }
    Err(error.into())
}

async fn serve() -> Result<i32> {
    let cli = original_cli()?;
    let pid = parent_pid(std::process::id())
        .ok_or_else(|| BridgeError::unavailable("无法确认官方 CLI 父进程。"))?;
    let server = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let process = ProcessIdentity::read(pid)
                .ok_or_else(|| BridgeError::unavailable("官方 CLI 已退出。"))?;
            if normalized(&process.executable) == normalized(&cli) {
                return Ok::<_, BridgeError>(process);
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .map_err(|_| BridgeError::unavailable("官方 CLI 启动超时。"))??;
    let expected = std::env::var_os("CODEX_MOBILE_DESKTOP_PATH")
        .map(PathBuf::from)
        .ok_or_else(|| BridgeError::unavailable("缺少本轮官方桌面绑定。"))?;
    let desktop = find_desktop_owner(
        &expected,
        parent_pid(server.pid).unwrap_or_default(),
        ProcessIdentity::read,
        parent_pid,
    )
    .ok_or_else(|| BridgeError::unavailable("官方 CLI 不属于本轮桌面。"))?;
    // 此描述符只由上面的启动分支传入，并在这里接管关闭责任。
    let upstream = unsafe { UnixStream::from_raw_fd(BROKER_FD) };
    upstream.set_nonblocking(true)?;
    let mut upstream = tokio::net::UnixStream::from_std(upstream)?;
    let mut input = BufReader::new(tokio::io::stdin());
    let mut first = String::new();
    if input.read_line(&mut first).await? == 0 {
        return Ok(0);
    }
    if !local_initialize(&serde_json::from_str(&first).unwrap_or(Value::Null)) {
        upstream.write_all(first.as_bytes()).await?;
        let (mut reader, mut writer) = upstream.split();
        let mut output = tokio::io::stdout();
        tokio::select! {
            result = tokio::io::copy(&mut input, &mut writer) => { result?; },
            result = tokio::io::copy(&mut reader, &mut output) => { result?; },
        }
        return Ok(0);
    }
    let paths = Paths::from_env(PathBuf::new())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let mut record = RuntimeInfo {
        format: 2,
        instance_id: uuid::Uuid::new_v4().to_string(),
        host: "local".into(),
        contract: CONTRACT.into(),
        state: "starting".into(),
        url: format!("ws://127.0.0.1:{}", listener.local_addr()?.port()),
        auth_token: crate::auth::random_token(),
        desktop,
        server,
        monitor: ProcessIdentity::current()?,
        codex_home: std::fs::canonicalize(&paths.codex_home)?,
        cli_path: cli,
    };
    record.publish(&paths.runtime_home)?;
    let result = crate::broker::run(
        input,
        tokio::io::stdout(),
        Box::new(upstream),
        listener,
        &mut record,
        paths.runtime_home.clone(),
        first.trim_end_matches(['\r', '\n']),
    )
    .await;
    record.state = "retired".into();
    let retired = record.publish(&paths.runtime_home);
    std::fs::remove_file(record.path(&paths.runtime_home))?;
    retired?;
    result?;
    Ok(0)
}
