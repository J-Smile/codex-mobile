mod support;
use codex_mobile_bridge::{
    paths::write_private,
    updates::{self, Candidate, Handoff, LaunchOptions, Updates},
};
use serde_json::json;
use std::path::Path;

fn candidate(root: &Path, name: &str, version: &str) -> Candidate {
    write_private(&root.join("release.json"), br#"{"runtimeBoundary":2}"#).unwrap();
    Candidate {
        root: root.into(),
        version: version.into(),
        executable: support::process::child(root, name),
        archive_executable: None,
    }
}

#[tokio::test]
async fn unverified_previous_release_is_never_started_as_fallback() {
    let f = tempfile::tempdir().unwrap();
    let data = f.path().join("data");
    let old = candidate(&f.path().join("old"), "ready-0.3.0", "0.3.0");
    std::fs::remove_file(old.root.join("release.json")).unwrap();
    let missing = candidate(&f.path().join("failed"), "fail", "0.5.0");
    write_private(
        &data.join("updates/native/installed.json"),
        &serde_json::to_vec(&updates::Installation {
            current: missing,
            previous: Some(old.clone()),
            failed_version: None,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(updates::redirect_installed(&data, &[])
        .await
        .unwrap_err()
        .message
        .contains("连接边界"));
    assert!(!old.executable.with_extension("launched").exists());
}

#[tokio::test]
async fn update_handoff_waits_for_ready_and_restores_failed_version() {
    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let data = root.join("data");
    let old = candidate(&root.join("old"), "ready-0.3.0", "0.3.0");
    let new = candidate(&root.join("new"), "ready-0.4.0", "0.4.0");
    let plan = data.join("updates/native/handoff.json");
    let launch = LaunchOptions {
        background: true,
        resume_service: true,
    };
    let handoff = Handoff {
        parent_pid: 0,
        current: old.clone(),
        candidate: new.clone(),
        data_home: data.clone(),
        launch,
    };
    write_private(&plan, &serde_json::to_vec(&handoff).unwrap()).unwrap();
    // Use the shipped updater binary so argument parsing and process boundaries are covered.
    let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_codex-mobile-updater"))
        .arg(&plan)
        .status()
        .await
        .unwrap();
    assert!(status.success());
    assert!(!plan.exists());
    let installed = updates::installed(&data).unwrap().unwrap();
    assert_eq!(installed.current.version, "0.4.0");
    assert_eq!(installed.previous.unwrap().version, "0.3.0");
    let launched = new.executable.with_extension("launched");
    let arguments = std::fs::read_to_string(&launched).unwrap();
    assert_eq!(
        arguments.lines().skip(3).collect::<Vec<_>>(),
        ["--autostart", "--resume-service", "--update-handoff"]
    );
    #[cfg(unix)]
    {
        let process = std::fs::read_to_string(new.executable.with_extension("group")).unwrap();
        let (pid, group) = process.split_once('\n').unwrap();
        assert_eq!(pid, group, "更新后的应用应使用独立进程组");
    }
    std::fs::remove_file(&launched).unwrap();

    let failed = candidate(&root.join("failed"), "fail", "0.5.0");
    write_private(
        &plan,
        &serde_json::to_vec(&Handoff {
            parent_pid: 0,
            current: new.clone(),
            candidate: failed,
            data_home: data.clone(),
            launch,
        })
        .unwrap(),
    )
    .unwrap();
    updates::apply_handoff(&plan).await.unwrap();
    let installed = updates::installed(&data).unwrap().unwrap();
    assert_eq!(installed.current.version, "0.4.0");
    assert_eq!(installed.failed_version.as_deref(), Some("0.5.0"));
    assert_eq!(installed.previous.unwrap().version, "0.3.0");
    assert!(data.join("updates/native/last-error.json").is_file());
    let fallback_arguments = std::fs::read_to_string(&launched).unwrap();
    assert_eq!(
        fallback_arguments.lines().skip(3).collect::<Vec<_>>(),
        ["--autostart", "--resume-service", "--update-handoff"]
    );

    // A later successful release clears the failure and keeps the current working version.
    let next = candidate(&root.join("next"), "ready-0.6.0", "0.6.0");
    let mut previous_format = serde_json::to_value(Handoff {
        parent_pid: 0,
        current: new,
        candidate: next.clone(),
        data_home: data.clone(),
        launch: Default::default(),
    })
    .unwrap();
    previous_format.as_object_mut().unwrap().remove("launch");
    write_private(&plan, &serde_json::to_vec(&previous_format).unwrap()).unwrap();
    updates::apply_handoff(&plan).await.unwrap();
    assert_eq!(
        updates::installed(&data).unwrap().unwrap().current.version,
        "0.6.0"
    );
    assert!(!data.join("updates/native/last-error.json").exists());
    let original_arguments =
        std::fs::read_to_string(next.executable.with_extension("launched")).unwrap();
    assert_eq!(
        original_arguments.lines().skip(3).collect::<Vec<_>>(),
        ["--update-handoff"]
    );
}

#[tokio::test]
async fn launcher_follows_installation_and_falls_back_when_current_is_missing() {
    let f = tempfile::tempdir().unwrap();
    let data = f.path().join("data");
    let old = candidate(&f.path().join("old"), "ready-0.3.0", "0.3.0");
    let missing = Candidate {
        root: f.path().join("missing"),
        version: "0.4.0".into(),
        executable: f.path().join("missing/app"),
        archive_executable: None,
    };
    write_private(
        &data.join("updates/native/installed.json"),
        &serde_json::to_vec(&updates::Installation {
            current: missing,
            previous: Some(old.clone()),
            failed_version: None,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(updates::redirect_installed(&data, &[]).await.unwrap());
    assert!(
        !std::fs::read_to_string(old.executable.with_extension("launched"))
            .unwrap()
            .contains("--update-handoff")
    );
    let installed = updates::installed(&data).unwrap().unwrap();
    assert_eq!(installed.current.version, old.version);
    assert_eq!(installed.failed_version.as_deref(), Some("0.4.0"));
    assert!(
        !updates::redirect_installed(&data, &["--update-ready".into()])
            .await
            .unwrap()
    );
    let source = Updates::new(data, false).unwrap();
    assert!(source
        .prepare("0.3.0", |_| async {})
        .await
        .unwrap_err()
        .message
        .contains("源码运行"));
}

#[cfg(windows)]
#[tokio::test]
async fn windows_upgrades_keep_the_listener_path_and_restore_it_on_failure() {
    use std::{fs, io::Write};

    let fixture = tempfile::tempdir().unwrap();
    let root = fixture.path();
    let data = root.join("data");
    let plan = data.join("updates/native/handoff.json");
    let listener = support::process::child(&root.join("installed"), "codex-mobile-desktop");
    let packaged = |version: &str, ready: bool| {
        let archive = candidate(&root.join(version), "Codex Mobile", version);
        // 为每个版本生成不同的可执行文件内容，便于核对实际替换和恢复的版本。
        fs::OpenOptions::new()
            .append(true)
            .open(&archive.executable)
            .unwrap()
            .write_all(version.as_bytes())
            .unwrap();
        fs::write(
            archive.root.join("fixture-version.txt"),
            if ready { version } else { "fail" },
        )
        .unwrap();
        archive
    };
    let old = packaged("0.3.9", true);
    fs::copy(&old.executable, &listener).unwrap();
    let mut current = Candidate {
        executable: listener.clone(),
        archive_executable: Some(old.executable),
        ..old
    };
    for (version, ready) in [("0.3.10", true), ("0.3.11", true), ("0.3.12", false)] {
        let next = packaged(version, ready);
        let expected = if ready {
            &next.executable
        } else {
            current.archive_executable.as_ref().unwrap()
        };
        let old_bytes = fs::read(expected).unwrap();
        write_private(
            &plan,
            &serde_json::to_vec(&Handoff {
                parent_pid: 0,
                current: current.clone(),
                candidate: next,
                data_home: data.clone(),
                launch: LaunchOptions {
                    background: true,
                    resume_service: true,
                },
            })
            .unwrap(),
        )
        .unwrap();
        let status = tokio::process::Command::new(env!("CARGO_BIN_EXE_codex-mobile-updater"))
            .arg(&plan)
            .status()
            .await
            .unwrap();
        assert!(status.success());
        let state = updates::installed(&data).unwrap().unwrap();
        assert_eq!(state.current.executable, listener);
        assert_eq!(
            state.current.version,
            if ready { version } else { &current.version }
        );
        assert_eq!(state.failed_version.as_deref(), (!ready).then_some(version));
        assert_eq!(fs::read(&listener).unwrap(), old_bytes);
        let arguments = fs::read_to_string(listener.with_extension("launched")).unwrap();
        assert_eq!(
            arguments.lines().skip(3).collect::<Vec<_>>(),
            ["--autostart", "--resume-service", "--update-handoff"]
        );
        current = state.current;
    }
}

#[cfg(windows)]
#[tokio::test]
async fn windows_manual_install_starts_a_newer_build_than_the_saved_update() {
    let fixture = tempfile::tempdir().unwrap();
    let old = candidate(fixture.path(), "ready-0.0.1", "0.0.1");
    write_private(
        &fixture.path().join("updates/native/installed.json"),
        &serde_json::to_vec(&updates::Installation {
            current: old.clone(),
            previous: None,
            failed_version: None,
        })
        .unwrap(),
    )
    .unwrap();
    assert!(!updates::redirect_installed(fixture.path(), &[])
        .await
        .unwrap());
    assert!(!old.executable.with_extension("launched").exists());
}

#[test]
fn release_requires_matching_platform_and_complete_runtime_resources() {
    let f = tempfile::tempdir().unwrap();
    let root = f.path();
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
    std::fs::write(
        root.join("release.json"),
        json!({"runtimeBoundary":2,"backend":"rust","version":"0.3.0","platform":platform,"arch":arch}).to_string(),
    )
    .unwrap();
    let executable = if cfg!(windows) {
        root.join("Codex Mobile.exe")
    } else if cfg!(target_os = "macos") {
        root.join("Codex Mobile.app/Contents/MacOS/codex-mobile-desktop")
    } else {
        root.join("codex-mobile-desktop")
    };
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(executable, b"fixture").unwrap();
    assert!(updates::read_candidate(root, "0.3.0")
        .unwrap_err()
        .message
        .contains("手机网页"));
    let resources = if cfg!(target_os = "macos") {
        root.join("Codex Mobile.app/Contents/Resources")
    } else {
        root.into()
    };
    std::fs::create_dir_all(resources.join("mobile")).unwrap();
    std::fs::write(resources.join("mobile/index.html"), b"fixture").unwrap();
    assert!(updates::read_candidate(root, "0.3.0")
        .unwrap_err()
        .message
        .contains("运行组件"));
    for name in ["codex-mobile-proxy", "codex-mobile-updater"] {
        let file = resources
            .join("bin")
            .join(format!("{name}{}", if cfg!(windows) { ".exe" } else { "" }));
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, b"fixture").unwrap();
    }
    assert!(updates::read_candidate(root, "0.3.0").is_ok());
    assert!(updates::read_candidate(root, "0.4.0")
        .unwrap_err()
        .message
        .contains("不匹配"));
}
