//! Native child-process conventions shared by tools and MCP transports.

use std::{
    env,
    ffi::{OsStr, OsString},
    path::PathBuf,
    sync::Arc,
};

#[cfg(unix)]
use std::{collections::HashSet, sync::Mutex, time::Duration};

#[cfg(any(windows, test))]
use std::path::Path;

use tokio::process::Command;

/// Tracks POSIX process groups whose leaders were spawned by `AxiomCLI`.
///
/// Tool and MCP children deliberately lead their own process groups so one
/// cancellation can terminate their complete process trees. That also moves
/// them outside a supervising Desktop sidecar's process group, so ownership
/// must remain inside `AxiomCLI` as an RAII lease. Dropping either the final
/// registry owner or an armed guard synchronously sends SIGKILL before the
/// process-group ID can be forgotten and later reused.
#[derive(Clone, Default)]
pub(crate) struct ProcessGroupRegistry {
    inner: Arc<ProcessGroupRegistryInner>,
}

#[derive(Default)]
struct ProcessGroupRegistryInner {
    #[cfg(unix)]
    state: Mutex<ProcessGroupRegistryState>,
}

#[cfg(unix)]
#[derive(Default)]
struct ProcessGroupRegistryState {
    active: HashSet<i32>,
    closed: bool,
}

impl Drop for ProcessGroupRegistryInner {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let state = self
                .state
                .get_mut()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            for &pgid in &state.active {
                signal_process_group(pgid, nix::sys::signal::Signal::SIGKILL);
            }
            state.active.clear();
        }
    }
}

impl ProcessGroupRegistry {
    pub(crate) fn register(&self, process_id: Option<u32>) -> ProcessGroupGuard {
        #[cfg(unix)]
        let group_id = process_id
            .and_then(|value| i32::try_from(value).ok())
            .filter(|value| *value > 1 && Some(*value) != i32::try_from(std::process::id()).ok());
        #[cfg(not(unix))]
        let group_id = {
            let _ = process_id;
            None
        };

        #[cfg(unix)]
        let group_id = group_id.and_then(|group_id| {
            let mut state = self.state();
            if state.closed {
                // The owner may have shut down after scheduling a task but
                // before that task reached spawn. Keep registration and the
                // closed check under one lock so no process group can appear
                // after the final drain.
                signal_process_group(group_id, nix::sys::signal::Signal::SIGKILL);
                None
            } else {
                state.active.insert(group_id);
                Some(group_id)
            }
        });

        ProcessGroupGuard {
            registry: self.clone(),
            pgid: group_id,
        }
    }

    /// Immediately terminates and forgets every group still owned by this
    /// registry. This is synchronous so it remains usable from Drop and from a
    /// runtime-shutdown path that can no longer poll async cleanup futures.
    pub(crate) fn terminate_all(&self) {
        #[cfg(unix)]
        {
            let mut state = self.state();
            state.closed = true;
            for &pgid in &state.active {
                signal_process_group(pgid, nix::sys::signal::Signal::SIGKILL);
            }
            state.active.clear();
        }
    }

    #[cfg(unix)]
    fn state(&self) -> std::sync::MutexGuard<'_, ProcessGroupRegistryState> {
        self.inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    #[cfg(unix)]
    fn signal_if_registered(&self, pgid: i32, signal: nix::sys::signal::Signal) -> bool {
        let state = self.state();
        if !state.active.contains(&pgid) {
            return false;
        }
        signal_process_group(pgid, signal)
    }

    #[cfg(unix)]
    fn kill_and_unregister(&self, pgid: i32) {
        let mut state = self.state();
        if state.active.contains(&pgid) {
            signal_process_group(pgid, nix::sys::signal::Signal::SIGKILL);
            state.active.remove(&pgid);
        }
    }

    #[cfg(all(test, unix))]
    pub(crate) fn active_count(&self) -> usize {
        self.state().active.len()
    }

    #[cfg(all(test, unix))]
    pub(crate) fn is_closed(&self) -> bool {
        self.state().closed
    }
}

pub(crate) struct ProcessGroupGuard {
    registry: ProcessGroupRegistry,
    pgid: Option<i32>,
}

impl ProcessGroupGuard {
    /// Gracefully asks the group to stop, then guarantees a synchronous kill
    /// before releasing its registration. If SIGTERM reports no group, the
    /// leader and every descendant are already gone and no grace delay is
    /// needed.
    pub(crate) async fn terminate(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            if self
                .registry
                .signal_if_registered(pgid, nix::sys::signal::Signal::SIGTERM)
            {
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
            self.registry.kill_and_unregister(pgid);
        }
        self.pgid = None;
    }

    pub(crate) fn terminate_now(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            self.registry.kill_and_unregister(pgid);
        }
        self.pgid = None;
    }
}

impl Drop for ProcessGroupGuard {
    fn drop(&mut self) {
        self.terminate_now();
    }
}

#[cfg(unix)]
fn signal_process_group(pgid: i32, signal: nix::sys::signal::Signal) -> bool {
    nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pgid), signal).is_ok()
}

pub(crate) fn apply_sanitized_environment(command: &mut Command) {
    command.env_clear().env("PATH", sanitized_path());
    #[cfg(windows)]
    command.env("PATHEXT", sanitized_pathext());
    for key in inherited_environment_keys() {
        if let Some(value) = env::var_os(key).filter(|value| !value.is_empty()) {
            command.env(key, value);
        }
    }
}

pub(crate) fn native_shell(script: String) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        let program = env::var_os("SystemRoot")
            .map(PathBuf::from)
            .map(|root| {
                root.join("System32")
                    .join("WindowsPowerShell")
                    .join("v1.0")
                    .join("powershell.exe")
            })
            .filter(|path| path.is_file())
            .unwrap_or_else(|| PathBuf::from("powershell.exe"));
        return (
            program.to_string_lossy().into_owned(),
            vec![
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-NonInteractive".into(),
                "-Command".into(),
                script,
            ],
        );
    }

    #[cfg(not(windows))]
    {
        ("/bin/sh".into(), vec!["-c".into(), script])
    }
}

pub(crate) fn runtime_platform_context() -> String {
    format!(
        "Operating system: {} ({}). When a shell tool is available, it uses {}.",
        env::consts::OS,
        env::consts::ARCH,
        if cfg!(windows) {
            "Windows PowerShell"
        } else {
            "/bin/sh"
        }
    )
}

fn sanitized_path() -> OsString {
    let inherited = env::var_os("PATH");
    let mut paths = sanitized_path_directories(inherited.as_deref());

    if let Some(home) = home_directory() {
        push_unique_directory(&mut paths, home.join(".cargo").join("bin"));
    }

    #[cfg(target_os = "macos")]
    for path in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin"] {
        push_unique_directory(&mut paths, PathBuf::from(path));
    }

    #[cfg(windows)]
    if let Some(root) = env::var_os("SystemRoot").map(PathBuf::from) {
        push_unique_directory(&mut paths, root.join("System32"));
        push_unique_directory(&mut paths, root.clone());
        push_unique_directory(&mut paths, root.join("System32").join("Wbem"));
        push_unique_directory(
            &mut paths,
            root.join("System32").join("WindowsPowerShell").join("v1.0"),
        );
    }

    if paths.is_empty() {
        #[cfg(windows)]
        push_unique_directory(&mut paths, PathBuf::from(r"C:\Windows\System32"));
        #[cfg(not(windows))]
        for path in ["/usr/bin", "/bin"] {
            push_unique_directory(&mut paths, PathBuf::from(path));
        }
    }

    env::join_paths(paths).unwrap_or_default()
}

fn sanitized_path_directories(value: Option<&OsStr>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(value) = value {
        for candidate in env::split_paths(value) {
            push_unique_directory(&mut paths, candidate);
        }
    }
    paths
}

fn push_unique_directory(paths: &mut Vec<PathBuf>, candidate: PathBuf) {
    if candidate.is_absolute()
        && candidate.is_dir()
        && env::join_paths([candidate.as_path()]).is_ok()
        && !paths.iter().any(|path| path == &candidate)
    {
        paths.push(candidate);
    }
}

#[cfg(windows)]
fn sanitized_pathext() -> OsString {
    let inherited = env::var_os("PATHEXT");
    sanitized_pathext_from(inherited.as_deref())
}

#[cfg(any(windows, test))]
fn sanitized_pathext_from(value: Option<&OsStr>) -> OsString {
    const ALLOWED: [&str; 5] = [".COM", ".EXE", ".BAT", ".CMD", ".PS1"];
    let mut extensions = Vec::new();
    if let Some(value) = value.and_then(OsStr::to_str) {
        for extension in value.split(';') {
            let extension = extension.trim().to_ascii_uppercase();
            if ALLOWED.contains(&extension.as_str()) && !extensions.contains(&extension) {
                extensions.push(extension);
            }
        }
    }
    for required in ALLOWED {
        if !extensions.iter().any(|extension| extension == required) {
            extensions.push(required.into());
        }
    }
    OsString::from(extensions.join(";"))
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program(program: &str, cwd: &Path) -> Option<PathBuf> {
    let path = sanitized_path();
    let pathext = sanitized_pathext();
    resolve_windows_program_with(program, cwd, &path, &pathext)
}

#[cfg(any(windows, test))]
fn resolve_windows_program_with(
    program: &str,
    cwd: &Path,
    path: &OsStr,
    pathext: &OsStr,
) -> Option<PathBuf> {
    let requested = Path::new(program);
    if requested.is_absolute() || program.contains(['/', '\\']) {
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            cwd.join(requested)
        };
        return candidate.is_file().then_some(candidate);
    }

    let extensions = pathext.to_string_lossy();
    for directory in sanitized_path_directories(Some(path)) {
        if requested.extension().is_some() {
            let exact = directory.join(requested);
            if exact.is_file() {
                return Some(exact);
            }
        } else {
            for extension in extensions.split(';').filter(|value| !value.is_empty()) {
                let candidate = directory.join(format!("{program}{extension}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

#[cfg(any(windows, test))]
pub(crate) fn windows_program_requires_shell(program: &Path) -> bool {
    program
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "cmd" | "bat" | "ps1"
            )
        })
}

fn home_directory() -> Option<PathBuf> {
    env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}

#[cfg(windows)]
fn inherited_environment_keys() -> &'static [&'static str] {
    &[
        "SystemRoot",
        "WINDIR",
        "ComSpec",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PROGRAMDATA",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "PSModulePath",
        "RUSTUP_HOME",
        "CARGO_HOME",
    ]
}

#[cfg(not(windows))]
fn inherited_environment_keys() -> &'static [&'static str] {
    &[
        "HOME",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "RUSTUP_HOME",
        "CARGO_HOME",
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn sanitized_path_drops_relative_empty_missing_and_duplicate_entries() {
        let root = tempfile::tempdir().expect("temporary root");
        let existing = root.path().join("existing");
        std::fs::create_dir(&existing).expect("existing PATH directory");
        let missing = root.path().join("missing");
        let joined = std::env::join_paths([
            std::path::PathBuf::new(),
            std::path::PathBuf::from("relative-tools"),
            existing.clone(),
            missing,
            existing.clone(),
        ])
        .expect("synthetic PATH");

        assert_eq!(
            super::sanitized_path_directories(Some(&joined)),
            vec![existing]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_shell_does_not_source_home_profile_or_replace_the_requested_cwd() {
        let root = tempfile::tempdir().expect("temporary root");
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home).expect("synthetic home");
        std::fs::create_dir_all(&workspace).expect("workspace");
        std::fs::write(
            home.join(".profile"),
            "export AXIOM_PROFILE_MARKER=profile-loaded\ncd /\n",
        )
        .expect("synthetic profile");

        let (program, arguments) = super::native_shell(
            "printf '%s\\n%s\\n' \"${AXIOM_PROFILE_MARKER-unset}\" \"$PWD\"".into(),
        );
        let mut command = tokio::process::Command::new(program);
        super::apply_sanitized_environment(&mut command);
        let output = command
            .args(arguments)
            .env("HOME", &home)
            .current_dir(&workspace)
            .output()
            .await
            .expect("run non-login shell");

        assert!(output.status.success());
        let stdout = String::from_utf8(output.stdout).expect("UTF-8 shell output");
        let mut lines = stdout.lines();
        assert_eq!(lines.next(), Some("unset"));
        let reported = std::path::Path::new(lines.next().expect("shell working directory"));
        assert_eq!(
            reported.canonicalize().expect("reported working directory"),
            workspace
                .canonicalize()
                .expect("requested working directory")
        );
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn windows_bare_program_resolution_finds_cmd_shims_but_marks_them_as_shells() {
        let pathext =
            super::sanitized_pathext_from(Some(std::ffi::OsStr::new(".PY;.cmd;.exe;.cmd")))
                .to_string_lossy()
                .into_owned();
        assert_eq!(pathext, ".CMD;.EXE;.COM;.BAT;.PS1");

        let root = tempfile::tempdir().expect("temporary root");
        let bin = root.path().join("bin");
        std::fs::create_dir(&bin).expect("synthetic bin");
        let npx = bin.join("npx.cmd");
        std::fs::write(&npx, "@echo off\r\n").expect("npx shim");
        let path = std::env::join_paths([bin]).expect("synthetic PATH");

        let resolved = super::resolve_windows_program_with(
            "npx",
            root.path(),
            &path,
            std::ffi::OsStr::new(".cmd;.exe"),
        )
        .expect("resolve npx.cmd");
        assert_eq!(resolved, npx);
        assert!(super::windows_program_requires_shell(&resolved));
    }
}
