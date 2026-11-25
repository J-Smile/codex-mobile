use std::path::PathBuf;
use tokio::sync::watch;

/// 监听已通过原生桌面校验的聊天文件，只通知读取方重新取快照。
pub(crate) struct RolloutWatch {
    path: PathBuf,
    changes: watch::Receiver<bool>,
    #[cfg(windows)]
    _watcher: notify::RecommendedWatcher,
}

impl RolloutWatch {
    #[cfg(windows)]
    pub(crate) fn open(path: PathBuf) -> Option<Self> {
        use notify::{EventKind, RecursiveMode, Watcher};

        if !path.is_file() {
            return None;
        }
        let file_name = path.file_name()?.to_owned();
        let (updates, changes) = watch::channel(true);
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Ok(event) = &event {
                    if !event.need_rescan()
                        && (matches!(event.kind, EventKind::Access(_))
                            || !event
                                .paths
                                .iter()
                                .any(|path| path.file_name() == Some(&file_name)))
                    {
                        return;
                    }
                }
                updates.send_modify(|valid| *valid &= event.is_ok());
            })
            .ok()?;
        watcher
            .watch(path.parent()?, RecursiveMode::NonRecursive)
            .ok()?;
        Some(Self {
            path,
            changes,
            _watcher: watcher,
        })
    }

    #[cfg(not(windows))]
    pub(crate) fn open(_path: PathBuf) -> Option<Self> {
        None
    }

    pub(crate) fn file_exists(&self) -> bool {
        self.path.is_file()
    }

    pub(crate) async fn changed(&mut self) -> bool {
        self.changes.changed().await.is_ok() && *self.changes.borrow_and_update()
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::{fs, time::Duration};
    use tokio::time::timeout;

    #[tokio::test]
    async fn wakes_for_the_selected_rollout_only() {
        let directory = tempfile::tempdir().unwrap();
        let selected = directory.path().join("selected.jsonl");
        let unrelated = directory.path().join("unrelated.jsonl");
        assert!(RolloutWatch::open(selected.clone()).is_none());

        fs::write(&selected, b"first turn\n").unwrap();
        let mut subscription = RolloutWatch::open(selected.clone()).unwrap();
        fs::write(unrelated, b"different chat\n").unwrap();
        assert!(timeout(Duration::from_millis(200), subscription.changed())
            .await
            .is_err());

        fs::write(&selected, b"first turn\nassistant reply\n").unwrap();
        assert!(timeout(Duration::from_secs(2), subscription.changed())
            .await
            .unwrap());
    }
}
