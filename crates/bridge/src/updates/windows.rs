use super::{read_candidate, read_json_optional, release_name, Candidate, WINDOWS_EXECUTABLE};
use crate::{paths::normalized, BridgeError, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn version_root(data: &Path, version: &str) -> PathBuf {
    data.join("updates/native").join(release_name(version))
}

pub fn resource_root(data: &Path, executable_root: &Path) -> Result<PathBuf> {
    let version = env!("CARGO_PKG_VERSION");
    let local = read_json_optional(&executable_root.join("release.json"))?;
    if local.is_some_and(|release| release["version"] == version) {
        return Ok(executable_root.into());
    }
    // 固定路径只替换桌面程序。手机网页和子进程使用各版本自己的资源目录。
    Ok(read_candidate(&version_root(data, version), version)?.root)
}

pub(super) fn archive_current(data: &Path, mut current: Candidate) -> Result<Candidate> {
    let root = version_root(data, &current.version);
    if !root.exists() {
        let staging = root.with_file_name(format!(".archive-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&staging)?;
        let result = (|| -> Result<()> {
            fs::copy(&current.executable, staging.join(WINDOWS_EXECUTABLE))?;
            fs::copy(
                current.root.join("release.json"),
                staging.join("release.json"),
            )?;
            for directory in ["bin", "mobile"] {
                let source = current.root.join(directory);
                for item in walkdir::WalkDir::new(&source) {
                    let item = item.map_err(|error| BridgeError::unavailable(error.to_string()))?;
                    let target = staging
                        .join(directory)
                        .join(item.path().strip_prefix(&source).unwrap());
                    if item.file_type().is_dir() {
                        fs::create_dir_all(target)?;
                    } else {
                        fs::copy(item.path(), target)?;
                    }
                }
            }
            read_candidate(&staging, &current.version)?;
            fs::rename(&staging, &root)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        result?;
    }
    let archive = read_candidate(&root, &current.version)?;
    // 从旧更新器迁入时，正在监听的路径可能正好位于版本目录里。
    let original = root.join("Codex Mobile.original.exe");
    if normalized(&archive.executable) == normalized(&current.executable) && !original.is_file() {
        replace_executable(&current.executable, &original)?;
    }
    let original = if original.is_file() {
        original
    } else {
        archive.executable
    };
    current.root = archive.root;
    current.archive_executable = Some(original);
    Ok(current)
}

pub(super) fn with_listener(candidate: Candidate, current: &Candidate) -> Candidate {
    if current.archive_executable.is_none() {
        return candidate;
    }
    let source = candidate.archive_executable.unwrap_or(candidate.executable);
    Candidate {
        executable: current.executable.clone(),
        archive_executable: Some(source),
        ..candidate
    }
}

pub(super) fn as_archive(mut candidate: Candidate) -> Candidate {
    if let Some(source) = candidate.archive_executable.take() {
        candidate.executable = source;
    }
    candidate
}

pub(super) fn activate(candidate: &Candidate) -> Result<()> {
    if let Some(source) = &candidate.archive_executable {
        replace_executable(source, &candidate.executable)?;
    }
    Ok(())
}

fn replace_executable(source: &Path, destination: &Path) -> Result<()> {
    let staged = destination.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = fs::copy(source, &staged).and_then(|_| fs::rename(&staged, destination));
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result.map_err(Into::into)
}
