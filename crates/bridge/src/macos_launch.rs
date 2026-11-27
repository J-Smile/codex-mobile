//! 通过官方启动时读取的 shell 环境安装共享入口，不改变官方应用或登录项。
use super::*;
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

const BEGIN: &str = "# >>> Codex Mobile shared connection >>>";
const END: &str = "# <<< Codex Mobile shared connection <<<";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Installation {
    enabled: bool,
    profile: PathBuf,
    block: String,
    proxy: PathBuf,
    version: String,
}

fn record_path(paths: &Paths) -> PathBuf {
    paths.runtime_home.join("desktop-launch.json")
}

fn read(paths: &Paths) -> Result<Option<Installation>> {
    read_json_optional(&record_path(paths))?
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

fn contents(path: &Path) -> Result<String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(value),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(error.into()),
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn block(launcher: &Path) -> String {
    let launcher = quote(launcher);
    format!("{BEGIN}\nif [ \"${{CODEX_SHELL:-}}\" = \"1\" ] && [ -z \"${{CODEX_CLI_PATH:-}}\" ] && [ -x {launcher} ]; then\n  export CODEX_CLI_PATH={launcher}\nfi\n{END}\n")
}

fn replace_block(text: &str, previous: Option<&str>, next: &str) -> Result<String> {
    if !next.is_empty() && text.contains(next) {
        return Ok(text.into());
    }
    if let Some(previous) = previous.filter(|previous| text.contains(*previous)) {
        return Ok(text.replacen(previous, next, 1));
    }
    if text.contains(BEGIN) || text.contains(END) {
        return Err(BridgeError::unavailable(
            "Codex 连接配置已被手动修改，请先恢复对应的 Codex Mobile 配置段后重试。",
        ));
    }
    let separator = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    Ok(format!("{text}{separator}{next}"))
}

fn write_profile(path: &Path, text: &str) -> Result<()> {
    // 保留用户已有的符号链接、文件权限及其余配置。
    let path = if path.is_symlink() {
        fs::canonicalize(path)?
    } else {
        path.to_path_buf()
    };
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let permissions = fs::metadata(&path)
        .map(|value| value.permissions())
        .unwrap_or_else(|_| fs::Permissions::from_mode(0o600));
    let temporary = path.with_extension("codex-mobile.tmp");
    write_private(&temporary, text.as_bytes())?;
    fs::set_permissions(&temporary, permissions)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn login_shell() -> Result<PathBuf> {
    let mut user = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let mut buffer = vec![0; 16384];
    // 与官方桌面一样读取系统账号的登录 shell，不受父进程的 SHELL 覆盖影响。
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            user.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Err(BridgeError::unavailable("无法读取当前账号的登录 shell。"));
    }
    Ok(PathBuf::from(
        unsafe { std::ffi::CStr::from_ptr(user.assume_init().pw_shell) }
            .to_string_lossy()
            .as_ref(),
    ))
}

fn profile(shell: &Path) -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| BridgeError::unavailable("无法找到用户目录。"))?;
    match shell.file_name().and_then(|name| name.to_str()) {
        Some("zsh") => {
            let output = Command::new(shell)
                .args(["-lc", "printf '\\0%s\\0' \"${ZDOTDIR:-$HOME}\""])
                .env_remove("CODEX_SHELL")
                .output()?;
            let directory = output
                .stdout
                .split(|byte| *byte == 0)
                .nth(1)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .filter(|_| output.status.success())
                .ok_or_else(|| {
                    BridgeError::unavailable("无法读取 zsh 启动目录，请检查 shell 配置后重试。")
                })?;
            Ok(directory.join(".zshenv"))
        }
        Some("bash") => Ok([".bash_profile", ".bash_login", ".profile"]
            .into_iter()
            .map(|name| home.join(name))
            .find(|path| path.exists())
            .unwrap_or(home.join(".bash_profile"))),
        _ => Err(BridgeError::unavailable(
            "自动接入暂支持 macOS 的 zsh 和 bash，请使用这两种登录 shell 后重新配置。",
        )),
    }
}

pub(super) fn status(paths: &Paths) -> Value {
    let result = (|| -> Result<bool> {
        let Some(record) = read(paths)? else {
            return Ok(false);
        };
        Ok(record.enabled
            && contents(&record.profile)?.contains(&record.block)
            && paths.runtime_home.join("desktop-launch/codex").is_file()
            && record.proxy.is_file())
    })();
    let enabled = result.as_ref().copied().unwrap_or(false);
    let message = match result {
        Err(error) => error.message,
        Ok(true) => "已配置。以后可直接打开 Codex，连接不依赖两个应用的启动顺序。".into(),
        Ok(false) => {
            "首次启用后重开一次 Codex，以后可按任意顺序打开。关闭仅撤销后续启动配置。".into()
        }
    };
    json!({"supported":true,"enabled":enabled,"message":message})
}

pub(super) fn ensure(paths: &Paths, binaries: &Path) -> Result<()> {
    if read(paths)?.is_some_and(|record| !record.enabled) {
        return Ok(());
    }
    configure(paths, binaries, true)
}

pub(super) fn configure(paths: &Paths, binaries: &Path, enabled: bool) -> Result<()> {
    let previous = read(paths)?;
    if !enabled {
        if let Some(mut record) = previous {
            let text = contents(&record.profile)?;
            let text = replace_block(&text, Some(&record.block), "")?;
            write_profile(&record.profile, &text)?;
            record.enabled = false;
            write_private(&record_path(paths), &serde_json::to_vec(&record)?)?;
        }
        return Ok(());
    }
    let desktop = discover_desktop(paths)?
        .ok_or_else(|| BridgeError::unavailable("请先安装官方 Codex 桌面程序。"))?;
    let cli = desktop
        .cli_path
        .as_ref()
        .ok_or_else(|| BridgeError::unavailable("未找到官方 Codex CLI，请修复官方安装后重试。"))?;
    let shell = login_shell()?;
    let launcher = paths.runtime_home.join("desktop-launch/codex");
    let output = Command::new(&shell)
        .args(["-ilc", "printf '\\0%s\\0' \"${CODEX_CLI_PATH:-}\""])
        .env("CODEX_SHELL", "1")
        .env_remove("CODEX_CLI_PATH")
        .output()?;
    let existing = output
        .stdout
        .split(|byte| *byte == 0)
        .nth(1)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .filter(|_| output.status.success())
        .ok_or_else(|| {
            BridgeError::unavailable("无法读取 Codex 启动环境，请检查 shell 配置后重试。")
        })?;
    if !existing.is_empty() && Path::new(existing) != launcher {
        return Err(BridgeError::unavailable(
            "已有自定义 CODEX_CLI_PATH，未覆盖原配置。请清除该自定义启动路径后重新启用自动接入。",
        ));
    }
    let profile = previous
        .as_ref()
        .map(|record| record.profile.clone())
        .map(Ok)
        .unwrap_or_else(|| profile(&shell))?;
    install(
        paths,
        binaries,
        &desktop.desktop_path,
        cli,
        profile,
        previous,
    )
}

fn install(
    paths: &Paths,
    binaries: &Path,
    desktop: &Path,
    cli: &Path,
    profile: PathBuf,
    previous: Option<Installation>,
) -> Result<()> {
    let launcher = paths.runtime_home.join("desktop-launch/codex");
    let next_block = block(&launcher);
    let original = contents(&profile)?;
    let updated = replace_block(
        &original,
        previous.as_ref().map(|record| record.block.as_str()),
        &next_block,
    )?;
    let proxy = match previous
        .as_ref()
        .filter(|record| record.version == env!("CARGO_PKG_VERSION") && record.proxy.is_file())
    {
        Some(record) => record.proxy.clone(),
        None => prepare_proxy(
            &paths.runtime_home,
            &binaries.join("codex-mobile-proxy"),
            cli,
        )?,
    };
    write_private(
        &proxy.parent().unwrap().join("proxy-cli.json"),
        &serde_json::to_vec(&json!({"cliPath":cli}))?,
    )?;
    // 固定脚本只选择当前版本的代理；官方更新后仍从当前应用包解析 CLI。
    let script = format!("#!/bin/sh\nif [ \"${{CODEX_HOME:-$HOME/.codex}}\" != {} ] || [ ! -x {} ]; then\n  exec {} \"$@\"\nfi\nexport CODEX_MOBILE_DESKTOP_PATH={}\nexport CODEX_MOBILE_RUNTIME_DIRECTORY={}\nexport CODEX_MOBILE_HOME={}\nexec {} \"$@\"\n", quote(&paths.codex_home), quote(&proxy), quote(cli), quote(desktop), quote(&paths.runtime_home), quote(&paths.data_home), quote(&proxy));
    write_private(&launcher, script.as_bytes())?;
    fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700))?;
    write_profile(&profile, &updated)?;
    let record = Installation {
        enabled: true,
        profile,
        block: next_block,
        proxy,
        version: env!("CARGO_PKG_VERSION").into(),
    };
    write_private(&record_path(paths), &serde_json::to_vec(&record)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;

    #[test]
    fn configuration_preserves_user_edits_and_rejects_changed_owned_blocks() {
        let old = block(Path::new("/tmp/old/codex"));
        let new = block(Path::new("/tmp/new/codex"));
        let original = "export USER_SETTING=kept\n";
        let installed = replace_block(original, None, &old).unwrap();
        let edited = format!("{installed}export LATER_SETTING=kept\n");
        let updated = replace_block(&edited, Some(&old), &new).unwrap();
        assert_eq!(updated.matches(BEGIN).count(), 1);
        assert_eq!(
            replace_block(&updated, Some(&new), "").unwrap(),
            format!("{original}export LATER_SETTING=kept\n")
        );
        assert!(replace_block(&installed.replace("-x", "-f"), Some(&old), &new).is_err());
    }

    #[test]
    fn installed_entry_survives_restart_and_disable_preserves_the_profile() {
        let fixture = Fixture::new();
        let root = fixture.paths.data_home.join("quoted ' space");
        fs::create_dir_all(&root).unwrap();
        let profile = root.join(".zshenv");
        let original = "export USER_SETTING=kept\n";
        fs::write(&profile, original).unwrap();
        let binaries = root.join("bin");
        fs::create_dir_all(&binaries).unwrap();
        fs::write(binaries.join("codex-mobile-proxy"), "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(
            binaries.join("codex-mobile-proxy"),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        install(
            &fixture.paths,
            &binaries,
            Path::new("/tmp/Codex.app/Contents/MacOS/Codex"),
            Path::new("/bin/echo"),
            profile.clone(),
            None,
        )
        .unwrap();
        let first = read(&fixture.paths).unwrap().unwrap();
        let first_proxy = first.proxy.clone();
        install(
            &fixture.paths,
            &binaries,
            Path::new("/tmp/Codex.app/Contents/MacOS/Codex"),
            Path::new("/bin/echo"),
            profile.clone(),
            Some(first),
        )
        .unwrap();
        assert_eq!(contents(&profile).unwrap().matches(BEGIN).count(), 1);
        assert_eq!(status(&fixture.paths)["enabled"], true);
        assert_eq!(read(&fixture.paths).unwrap().unwrap().proxy, first_proxy);
        for active in [false, true] {
            let mut shell = Command::new("/bin/zsh");
            shell
                .env("ZDOTDIR", &root)
                .env_remove("CODEX_CLI_PATH")
                .env_remove("CODEX_SHELL");
            if active {
                shell.env("CODEX_SHELL", "1");
            }
            let output = shell
                .args(["-c", "printf '%s' \"${CODEX_CLI_PATH:-unset}\""])
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                if active {
                    fixture
                        .paths
                        .runtime_home
                        .join("desktop-launch/codex")
                        .display()
                        .to_string()
                } else {
                    "unset".into()
                }
            );
        }
        let mut old = read(&fixture.paths).unwrap().unwrap();
        old.version = "previous-release".into();
        install(
            &fixture.paths,
            &binaries,
            Path::new("/tmp/Codex.app/Contents/MacOS/Codex"),
            Path::new("/bin/echo"),
            profile.clone(),
            Some(old),
        )
        .unwrap();
        assert_ne!(read(&fixture.paths).unwrap().unwrap().proxy, first_proxy);
        assert!(first_proxy.is_file(), "升级保留已运行实例使用的代理");
        configure(&fixture.paths, &binaries, false).unwrap();
        assert_eq!(contents(&profile).unwrap(), original);
        assert_eq!(status(&fixture.paths)["enabled"], false);
        ensure(&fixture.paths, &binaries).unwrap();
        assert_eq!(contents(&profile).unwrap(), original);
    }

    #[test]
    fn bash_entry_preserves_explicit_cli_and_symlinked_profile_permissions() {
        let fixture = Fixture::new();
        let launcher = fixture.paths.data_home.join("a ' quoted launcher");
        fs::write(&launcher, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o700)).unwrap();
        let target = fixture.paths.data_home.join("real-profile");
        fs::write(&target, "export EXISTING=kept\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o640)).unwrap();
        let profile = fixture.paths.data_home.join(".bash_profile");
        std::os::unix::fs::symlink(&target, &profile).unwrap();
        write_profile(
            &profile,
            &format!("export EXISTING=kept\n{}", block(&launcher)),
        )
        .unwrap();
        assert!(profile.is_symlink());
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o640
        );
        for existing in ["", "/custom/cli"] {
            let output = Command::new("/bin/bash")
                .args(["-c", ". \"$1\"; printf '%s' \"$CODEX_CLI_PATH\"", "fixture"])
                .arg(&profile)
                .env("CODEX_SHELL", "1")
                .env("CODEX_CLI_PATH", existing)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(
                String::from_utf8(output.stdout).unwrap(),
                if existing.is_empty() {
                    launcher.display().to_string()
                } else {
                    existing.into()
                }
            );
        }
    }
}
