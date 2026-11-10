//! Bounded retention of installer downloads; active native work holds a lease.
use anyhow::{Context as _, ensure};
use fs2::FileExt as _;
use std::{
    fs::{self, File, OpenOptions},
    path::Path,
    time::{Duration, SystemTime},
};

const LOCK: &str = "staging.lock";
const COMPLETE: &str = "completed";
const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub(super) fn lock(directory: &Path) -> anyhow::Result<File> {
    ensure!(
        directory
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("pending-"))
            && fs::symlink_metadata(directory)?.file_type().is_dir(),
        "Invalid update staging directory"
    );
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(LOCK))?;
    file.try_lock_exclusive()
        .context("Update staging is still in use")?;
    Ok(file)
}

// Keep the completion marker until all payloads are gone. Windows cannot remove
// the running helper or its loaded DLLs; the next update check retries them.
fn remove(directory: &Path) -> std::io::Result<()> {
    let mut failure = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_name() == LOCK || entry.file_name() == COMPLETE {
            continue;
        }
        let result = if entry.file_type()?.is_dir() {
            fs::remove_dir_all(entry.path())
        } else {
            fs::remove_file(entry.path())
        };
        if let Err(error) = result {
            failure = Some(error);
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    if directory.join(COMPLETE).try_exists()? {
        fs::remove_file(directory.join(COMPLETE))?;
    }
    fs::remove_file(directory.join(LOCK))?;
    fs::remove_dir(directory)
}

pub(super) fn finish(directory: &Path, _lease: File) {
    let _ = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(COMPLETE));
    let _ = remove(directory);
}

pub(super) fn prune(root: &Path) {
    prune_at(root, SystemTime::now());
}

fn prune_at(root: &Path, now: SystemTime) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("pending-")
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let directory = entry.path();
        let expired = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= MAX_AGE);
        if !expired && !directory.join(COMPLETE).is_file() {
            continue;
        }
        if let Ok(lease) = lock(&directory) {
            finish(&directory, lease);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(root: &Path, name: &str) -> std::path::PathBuf {
        let directory = root.join(name);
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("installer"), b"large payload").unwrap();
        directory
    }

    #[test]
    fn completed_update_removes_payload_and_preserves_feed_and_outcome() {
        let root = tempfile::tempdir().unwrap();
        fs::write(root.path().join("accepted.json"), b"signed feed").unwrap();
        fs::write(root.path().join("last-update.json"), b"outcome").unwrap();
        let directory = stage(root.path(), "pending-complete");
        let lease = lock(&directory).unwrap();
        finish(&directory, lease);
        assert!(!directory.exists());
        assert!(root.path().join("accepted.json").is_file());
        assert!(root.path().join("last-update.json").is_file());
    }

    #[test]
    fn pruning_keeps_fresh_and_active_jobs_but_removes_completed_and_abandoned_jobs() {
        let root = tempfile::tempdir().unwrap();
        let fresh = stage(root.path(), "pending-fresh");
        let active = stage(root.path(), "pending-active");
        let completed = stage(root.path(), "pending-complete");
        let unrelated = stage(root.path(), "other-data");
        fs::write(completed.join(COMPLETE), b"").unwrap();
        let lease = lock(&active).unwrap();
        let now = SystemTime::now();
        prune_at(root.path(), now);
        assert!(fresh.exists() && active.exists() && unrelated.exists());
        assert!(!completed.exists());
        prune_at(root.path(), now + MAX_AGE + Duration::from_secs(1));
        assert!(!fresh.exists());
        assert!(active.exists() && unrelated.exists());
        drop(lease);
        prune_at(root.path(), now + MAX_AGE + Duration::from_secs(1));
        assert!(!active.exists());
    }

    #[test]
    #[cfg(windows)]
    fn windows_keeps_a_completion_marker_until_the_helper_is_released() {
        use std::os::windows::fs::OpenOptionsExt as _;
        let root = tempfile::tempdir().unwrap();
        let directory = stage(root.path(), "pending-windows");
        let helper = directory.join("update-helper.exe");
        fs::write(&helper, b"helper").unwrap();
        let running = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(&helper)
            .unwrap();
        let lease = lock(&directory).unwrap();
        finish(&directory, lease);
        assert!(!directory.join("installer").exists());
        assert!(helper.exists() && directory.join(COMPLETE).is_file());
        drop(running);
        prune(root.path());
        assert!(!directory.exists());
    }

    #[test]
    #[cfg(unix)]
    fn pruning_never_follows_staging_or_payload_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("keep"), b"keep").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("pending-link")).unwrap();
        let directory = stage(root.path(), "pending-complete");
        std::os::unix::fs::symlink(outside.path(), directory.join("linked-payload")).unwrap();
        fs::write(directory.join(COMPLETE), b"").unwrap();
        prune_at(
            root.path(),
            SystemTime::now() + MAX_AGE + Duration::from_secs(1),
        );
        assert!(outside.path().join("keep").is_file());
        assert!(root.path().join("pending-link").is_symlink());
        assert!(!directory.exists());
    }
}
