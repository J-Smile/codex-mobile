use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub fn child(directory: &Path, name: &str) -> PathBuf {
    static BUILD: OnceLock<tempfile::TempDir> = OnceLock::new();
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let build = BUILD.get_or_init(|| {
        let build = tempfile::tempdir().unwrap();
        let compiler = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = std::process::Command::new(compiler)
            .arg("--edition=2021")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process-child.rs"))
            .arg("-o")
            .arg(build.path().join(format!("child{suffix}")))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        build
    });
    std::fs::create_dir_all(directory).unwrap();
    let destination = directory.join(format!("{name}{suffix}"));
    std::fs::copy(build.path().join(format!("child{suffix}")), &destination).unwrap();
    destination
}
