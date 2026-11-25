// A std-only, disposable child process for updater and runtime integration tests.
use std::{
    env, fs,
    io::{Read, Write},
    net::TcpListener,
};

#[cfg(unix)]
fn record_process(executable: &std::path::Path) {
    let parent = std::process::Command::new("ps")
        .args(["-p", &std::process::id().to_string(), "-o", "ppid="])
        .output()
        .unwrap();
    fs::write(
        executable.with_extension("process"),
        format!(
            "{}\n{}\n{}\n{}\n",
            std::process::id(),
            String::from_utf8_lossy(&parent.stdout).trim(),
            env::var("CODEX_CLI_PATH").unwrap_or_default(),
            env::var("CODEX_APP_RUNTIME_CLI_PATH").unwrap_or_default(),
        ),
    )
    .unwrap();
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let executable = env::current_exe().unwrap();
    let file_name = executable.file_name().unwrap().to_string_lossy();
    let name = file_name.strip_suffix(".exe").unwrap_or(&file_name);
    fs::write(executable.with_extension("launched"), args.join("\n")).unwrap();
    #[cfg(unix)]
    if name == "desktop-fixture" || name == "desktop-helper" {
        record_process(&executable);
        let status = std::process::Command::new(&args[0]).args(&args[1..]).status().unwrap();
        std::process::exit(status.code().unwrap_or(1));
    }
    if name == "fail" { std::process::exit(7); }
    if args.first().is_some_and(|arg| arg == "--update-ready") {
        let resource_version = fs::read_to_string("fixture-version.txt").ok();
        let version = name.strip_prefix("ready-").or(resource_version.as_deref()).unwrap();
        if version == "fail" { std::process::exit(7); }
        #[cfg(unix)]
        {
            let pid = std::process::id().to_string();
            let group = std::process::Command::new("ps").args(["-p", &pid, "-o", "pgid="]).output().unwrap();
            assert!(group.status.success());
            fs::write(executable.with_extension("group"), format!("{pid}\n{}", String::from_utf8_lossy(&group.stdout).trim())).unwrap();
        }
        let temporary = format!("{}.tmp", args[1]);
        fs::write(&temporary, format!(r#"{{"run":"{}","version":"{}","pid":{}}}"#, args[2], version, std::process::id())).unwrap();
        fs::rename(temporary, &args[1]).unwrap();
        return;
    }
    if args.iter().any(|s| s == "--version") {
        println!("fixture-cli 1.0");
        std::process::exit(42);
    }
    if name == "helper-cli" {
        #[cfg(unix)]
        record_process(&executable);
        println!("fixture-helper");
        std::process::exit(43);
    }
    #[cfg(unix)]
    if name == "exec-cli" {
        record_process(&executable);
        // This fixture only records the process chain. WebSocket forwarding is
        // exercised against a separate real local listener in runtime tests.
        loop {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    let index = args.iter().position(|s| s == "--listen").unwrap();
    let listener = TcpListener::bind(args[index + 1].strip_prefix("ws://").unwrap()).unwrap();
    let marker = env::var("RUNTIME_FIXTURE_MARKER").unwrap_or_default();
    fs::write(
        executable.with_extension("context"),
        format!("{}\n{}", env::current_dir().unwrap().display(), marker),
    )
    .unwrap();
    for socket in listener.incoming() {
        let mut socket = socket.unwrap();
        let mut bytes = [0; 4096];
        let _ = socket.read(&mut bytes);
        let _ = socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    }
}
