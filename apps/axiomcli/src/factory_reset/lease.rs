//! Cross-process fencing shared by current runtimes and older factory-reset
//! leases. Migration upgrades the existing process's handle, not a second lock.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

use anyhow::{Context as _, ensure};

use crate::paths::AxiomPaths;

const CLOSE_PROCESSES: &str = "Close all other Axiom Desktop, CLI and proxy processes, then retry local storage migration or maintenance. Existing data was preserved.";

type Shared = Arc<Mutex<State>>;
type Registry = Mutex<HashMap<PathBuf, Weak<Mutex<State>>>>;
static REGISTRY: OnceLock<Registry> = OnceLock::new();

struct State {
    file: File,
    restore_shared: bool,
    maintenance_holders: usize,
    upgrade_gate: Option<UpgradeGate>,
    failed: bool,
}

impl Drop for State {
    fn drop(&mut self) {
        // A forked child can retain the same Unix open-file description until
        // exec even when the descriptor is close-on-exec. Closing our File is
        // therefore insufficient to release its flock immediately. Only unlock
        // here, after the final Arc is gone: a successful Weak::upgrade must
        // continue to see the locked zero-holder state used by maintenance().
        let _ = fs2::FileExt::unlock(&self.file);
    }
}

struct UpgradeGate(File);

impl Drop for UpgradeGate {
    fn drop(&mut self) {
        // The gate has the same fork/dup lifetime issue as the runtime lock.
        let _ = fs2::FileExt::unlock(&self.0);
    }
}

/// Keeps the process's shared storage lease alive until normal shutdown.
pub struct RuntimeLease {
    _state: Shared,
}

/// Keeps exclusive access through database replacement, schema migration and
/// the final asset purge/checkpoint. The last guard restores the runtime lease.
pub(crate) struct MaintenanceGuard {
    state: Shared,
}

fn registry() -> &'static Registry {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn runtime(paths: &AxiomPaths) -> anyhow::Result<RuntimeLease> {
    let path = super::lease_path(paths)?;
    let mut registry = registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("Axiom storage lease is unavailable"))?;
    registry.retain(|_, value| value.strong_count() != 0);
    if let Some(state) = registry.get(&path).and_then(Weak::upgrade) {
        {
            let mut inner = state
                .lock()
                .map_err(|_| anyhow::anyhow!("Axiom storage lease is unavailable"))?;
            ensure!(
                !inner.failed && inner.maintenance_holders == 0,
                "Axiom local storage maintenance is in progress"
            );
            if !inner.restore_shared {
                downgrade(&inner.file).context(CLOSE_PROCESSES)?;
                inner.restore_shared = true;
            }
        }
        return Ok(RuntimeLease { _state: state });
    }
    let file = super::open_lease(paths)?;
    fs2::FileExt::try_lock_shared(&file)
        .context("Axiom local storage maintenance is in progress. Try again after it finishes.")?;
    let state = Arc::new(Mutex::new(State {
        file,
        restore_shared: true,
        maintenance_holders: 0,
        upgrade_gate: None,
        failed: false,
    }));
    registry.insert(path, Arc::downgrade(&state));
    Ok(RuntimeLease { _state: state })
}

/// Serialize upgrades before flock's non-atomic shared-to-exclusive conversion.
/// Competing new runtimes therefore keep their shared lease while one upgrader
/// tries exclusive access. Older runtimes keep their original shared lease.
fn upgrade_gate(path: &Path) -> anyhow::Result<UpgradeGate> {
    let path = path.with_file_name(".axiom-storage-maintenance.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(
            metadata.is_file() && !metadata.is_symlink(),
            "Invalid Axiom maintenance lock"
        );
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    fs2::FileExt::try_lock_exclusive(&file).context(CLOSE_PROCESSES)?;
    Ok(UpgradeGate(file))
}

fn try_upgrade(file: &File) -> std::io::Result<()> {
    // LockFileEx layers locks instead of converting them. On Windows release
    // our shared region first; the upgrade gate still serializes conversions.
    #[cfg(windows)]
    fs2::FileExt::unlock(file)?;
    fs2::FileExt::try_lock_exclusive(file)
}

fn downgrade(file: &File) -> std::io::Result<()> {
    #[cfg(windows)]
    fs2::FileExt::unlock(file)?;
    fs2::FileExt::lock_shared(file)
}

fn maintenance(
    paths: &AxiomPaths,
    permit_runtime_upgrade: bool,
) -> anyhow::Result<MaintenanceGuard> {
    let path = super::lease_path(paths)?;
    let mut registry = registry()
        .lock()
        .map_err(|_| anyhow::anyhow!("Axiom storage lease is unavailable"))?;
    registry.retain(|_, value| value.strong_count() != 0);
    if let Some(state) = registry.get(&path).and_then(Weak::upgrade) {
        {
            let mut inner = state
                .lock()
                .map_err(|_| anyhow::anyhow!("Axiom storage lease is unavailable"))?;
            ensure!(!inner.failed, "Axiom storage lease is unavailable");
            ensure!(
                permit_runtime_upgrade || (!inner.restore_shared && inner.maintenance_holders == 0),
                "{CLOSE_PROCESSES}"
            );
            if inner.maintenance_holders == 0 {
                let gate = upgrade_gate(&path)?;
                let upgraded = if inner.restore_shared {
                    try_upgrade(&inner.file)
                } else {
                    Ok(())
                };
                if let Err(error) = upgraded {
                    // flock can drop the shared lock when conversion fails.
                    // Restore it while retaining the upgrade gate so another
                    // current process cannot migrate through this interval.
                    if inner.restore_shared {
                        fs2::FileExt::lock_shared(&inner.file)
                            .context("Could not restore the Axiom runtime lease")?;
                    }
                    return Err(error).context(CLOSE_PROCESSES);
                }
                inner.upgrade_gate = Some(gate);
            }
            inner.maintenance_holders += 1;
        }
        return Ok(MaintenanceGuard { state });
    }
    let gate = upgrade_gate(&path)?;
    let file = super::open_lease(paths)?;
    fs2::FileExt::try_lock_exclusive(&file).context(CLOSE_PROCESSES)?;
    let state = Arc::new(Mutex::new(State {
        file,
        restore_shared: false,
        maintenance_holders: 1,
        upgrade_gate: Some(gate),
        failed: false,
    }));
    registry.insert(path, Arc::downgrade(&state));
    Ok(MaintenanceGuard { state })
}

pub(super) fn offline(paths: &AxiomPaths) -> anyhow::Result<MaintenanceGuard> {
    maintenance(paths, false)
}

pub(crate) fn maintenance_for_database(database: &Path) -> anyhow::Result<MaintenanceGuard> {
    let parent = database
        .parent()
        .context("Local database directory is missing")?;
    let root = parent
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "accounts"))
        .and_then(Path::parent)
        // Explicit standalone fixtures have their own directory-local fence.
        .map_or_else(
            || parent.join(".axiom-standalone-storage"),
            Path::to_path_buf,
        );
    maintenance(&AxiomPaths::from_roots(root.join("config"), root), true)
}

impl Drop for MaintenanceGuard {
    fn drop(&mut self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.maintenance_holders -= 1;
        if state.maintenance_holders == 0 {
            if state.restore_shared && downgrade(&state.file).is_err() {
                // Retaining exclusive access is safe; proceeding without any
                // fence is not. This is an unrecoverable OS locking failure.
                state.failed = true;
                if fs2::FileExt::lock_exclusive(&state.file).is_err() {
                    std::process::abort();
                }
            }
            state.upgrade_gate = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, Read, Write},
        process::{Child, Command, Stdio},
    };

    struct FixtureProcess(Child);
    impl Drop for FixtureProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[test]
    fn legacy_runtime_child() {
        let Some(path) = std::env::var_os("AXIOM_SYNTHETIC_LEASE_PATH") else {
            return;
        };
        let path = PathBuf::from(path).canonicalize().unwrap();
        assert!(path.starts_with(std::env::temp_dir().canonicalize().unwrap()));
        assert_eq!(path.file_name().unwrap(), ".axiom-factory-reset.lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        fs2::FileExt::try_lock_shared(&file).unwrap();
        println!("fixture-lease-ready");
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_exact(&mut [0; 1]).unwrap();
    }

    #[test]
    fn old_process_blocks_upgrade_and_nested_guards_restore_shared_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AxiomPaths::from_roots(
            directory.path().join("config/axiom"),
            directory.path().join("data/axiom"),
        );
        let _runtime = runtime(&paths).unwrap();
        let lease_path = super::super::lease_path(&paths).unwrap();
        let mut child = FixtureProcess(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "factory_reset::lease::tests::legacy_runtime_child",
                    "--nocapture",
                ])
                .env("AXIOM_SYNTHETIC_LEASE_PATH", &lease_path)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut ready = false;
        for line in std::io::BufReader::new(child.0.stdout.as_mut().unwrap()).lines() {
            if line.unwrap() == "fixture-lease-ready" {
                ready = true;
                break;
            }
        }
        assert!(ready, "legacy child did not obtain its shared lease");
        let database = paths.data_root().join("accounts/one/desktop/state.sqlite3");
        assert!(maintenance_for_database(&database).is_err());
        // Failed upgrade restored our shared lock, not an unprotected runtime.
        let observer = super::super::open_lease(&paths).unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&observer).is_err());
        child.0.stdin.take().unwrap().write_all(b"x").unwrap();
        assert!(child.0.wait().unwrap().success());
        let first = maintenance_for_database(&database).unwrap();
        let nested = maintenance_for_database(&database).unwrap();
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        drop(first);
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        drop(nested);
        fs2::FileExt::try_lock_shared(&observer).unwrap();
        fs2::FileExt::unlock(&observer).unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&observer).is_err());
        assert!(offline(&paths).is_err());
    }

    #[test]
    fn nested_opener_guard_cannot_downgrade_offline_maintenance() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AxiomPaths::from_roots(
            directory.path().join("config/axiom"),
            directory.path().join("data/axiom"),
        );
        let outer = offline(&paths).unwrap();
        let observer = super::super::open_lease(&paths).unwrap();
        let database = paths.data_root().join("accounts/one/desktop/state.sqlite3");
        let inner = maintenance_for_database(&database).unwrap();
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        drop(inner);
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        assert!(runtime(&paths).is_err());
        drop(outer);
        fs2::FileExt::try_lock_shared(&observer).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn last_owner_releases_lock_even_when_file_description_is_duplicated() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AxiomPaths::from_roots(
            directory.path().join("config/axiom"),
            directory.path().join("data/axiom"),
        );
        let outer = offline(&paths).unwrap();
        // dup and fork both retain the same flock open-file description.
        // Keeping a raw descriptor does not retain a logical lease owner.
        let (inherited, inherited_gate) = {
            let state = outer.state.lock().unwrap();
            (
                state.file.try_clone().unwrap(),
                state.upgrade_gate.as_ref().unwrap().0.try_clone().unwrap(),
            )
        };
        let observer = super::super::open_lease(&paths).unwrap();
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        drop(outer);
        fs2::FileExt::try_lock_shared(&observer).unwrap();
        let next_gate = upgrade_gate(&super::super::lease_path(&paths).unwrap()).unwrap();
        drop(next_gate);
        drop(inherited_gate);
        drop(inherited);
    }

    #[test]
    fn upgraded_weak_reference_keeps_zero_holder_state_exclusive_until_reused() {
        let directory = tempfile::tempdir().unwrap();
        let paths = AxiomPaths::from_roots(
            directory.path().join("config/axiom"),
            directory.path().join("data/axiom"),
        );
        let outer = offline(&paths).unwrap();
        let observer = super::super::open_lease(&paths).unwrap();
        // Model a registry lookup retaining State while the final guard exits.
        let retained = Arc::downgrade(&outer.state).upgrade().unwrap();
        drop(outer);
        assert_eq!(retained.lock().unwrap().maintenance_holders, 0);
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        let database = paths.data_root().join("accounts/one/desktop/state.sqlite3");
        let next = maintenance_for_database(&database).unwrap();
        assert!(Arc::ptr_eq(&retained, &next.state));
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        drop(next);
        assert!(fs2::FileExt::try_lock_shared(&observer).is_err());
        drop(retained);
        fs2::FileExt::try_lock_shared(&observer).unwrap();
    }
}
