use super::{Job, Restart, cache, installation, read_job};
use anyhow::{Context as _, ensure};
use fs2::FileExt as _;
use serde_json::json;
use std::io::Write as _;
use std::{
    ffi::OsString,
    fs::File,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn helper(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(path
        .parent()
        .context("Invalid update job")?
        .join(if cfg!(windows) {
            "update-helper.exe"
        } else {
            "update-helper"
        }))
}

pub fn spawn(path: &Path, parent: Option<u32>) -> anyhow::Result<()> {
    let _ = read_job(path)?;
    ensure!(
        parent.is_some(),
        "Desktop update requires its parent process"
    );
    let ready = path.with_file_name("ready");
    ensure!(!ready.try_exists()?, "Update job already started");
    let mut command = Command::new(helper(path)?);
    command
        .arg("update")
        .arg("--apply-job")
        .arg(path)
        .arg("--ready-file");
    if let Some(parent) = parent {
        command.arg("--parent").arg(parent.to_string());
    }
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    let mut child = command
        .spawn()
        .context("Could not start the update installer")?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.try_exists()? {
        ensure!(
            child.try_wait()?.is_none(),
            "Update helper exited before it was ready"
        );
        if Instant::now() >= deadline {
            let _ = child.kill();
            anyhow::bail!("Update helper did not become ready");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("{}", json!({"event":"installing"}));
    Ok(())
}

pub fn handoff(path: &Path) -> anyhow::Result<()> {
    let _ = read_job(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        Err(Command::new(helper(path)?)
            .arg("update")
            .arg("--apply-job")
            .arg(path)
            .exec()
            .into())
    }
    #[cfg(windows)]
    {
        // The installed .cmd/PowerShell launcher holds the console open while
        // the old executable exits. Its temporary helper can then replace it.
        let handoff = std::env::var_os("AXIOM_UPDATE_HANDOFF").context(
            "Start the TUI using the installed axiomcli command to update in this terminal",
        )?;
        std::fs::write(handoff, path.to_string_lossy().as_bytes())?;
        std::process::exit(85);
    }
}

fn wait_parent(parent: Option<u32>) -> anyhow::Result<()> {
    let Some(parent) = parent else { return Ok(()) };
    ensure!(
        parent != 0 && parent != std::process::id(),
        "Invalid update parent"
    );
    #[cfg(unix)]
    {
        let pid = nix::unistd::Pid::from_raw(i32::try_from(parent)?);
        let deadline = Instant::now() + Duration::from_secs(180);
        while nix::sys::signal::kill(pid, None).is_ok() {
            ensure!(
                Instant::now() < deadline,
                "Axiom did not close for the update"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    #[cfg(windows)]
    {
        let script = format!(
            "$p = Get-Process -Id {parent} -ErrorAction SilentlyContinue; if ($p) {{ $p | Wait-Process -Timeout 180 -ErrorAction Stop }}"
        );
        ensure!(
            Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .status()?
                .success(),
            "Axiom did not close for the update"
        );
    }
    Ok(())
}

fn exclusive(job: &Job) -> anyhow::Result<File> {
    let file = installation::lock(&job.installation)?;
    eprintln!(
        "Waiting for other AxiomCLI sessions, Desktop, and proxies to close before installation…"
    );
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if file.try_lock_exclusive().is_ok() {
            return Ok(file);
        }
        ensure!(
            Instant::now() < deadline,
            "Other AxiomCLI or proxy sessions are still running. Close them and retry."
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn install(job: &Job, artifact: &Path) -> anyhow::Result<()> {
    let owner = &job.installation;
    match owner.format.as_str() {
        "AppImage" => {
            let destination = owner.app_image.as_ref().context("Missing AppImage path")?;
            let directory = destination.parent().context("Invalid AppImage path")?;
            let temporary = tempfile::NamedTempFile::new_in(directory)?;
            std::fs::copy(artifact, temporary.path())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(0o755))?;
            }
            temporary.as_file().sync_all()?;
            temporary.persist(destination)?;
        }
        "sh" => {
            ensure!(
                owner.product == "cli" && cfg!(target_os = "linux"),
                "Wrong installer kind"
            );
            ensure!(
                Command::new("/bin/sh")
                    .arg(artifact)
                    .arg("--prefix")
                    .arg(&owner.root)
                    .status()?
                    .success(),
                "CLI installer failed"
            );
        }
        "deb" | "pacman" => {
            ensure!(cfg!(target_os = "linux"), "Wrong installer platform");
            let mut command = Command::new("/usr/bin/pkexec");
            if owner.format == "deb" {
                command.args(["/usr/bin/apt-get", "--yes", "install", "--"]);
            } else {
                command.args(["/usr/bin/pacman", "--noconfirm", "-U", "--"]);
            }
            ensure!(
                command.arg(artifact).status()?.success(),
                "Package installation was cancelled or failed"
            );
        }
        #[cfg(windows)]
        "exe" => {
            ensure!(
                Command::new(artifact)
                    .arg("/S")
                    .arg(format!("/D={}", owner.root.display()))
                    .status()?
                    .success(),
                "Windows installer failed"
            );
        }
        #[cfg(target_os = "macos")]
        "pkg" => {
            let path = artifact.to_str().context("Invalid installer path")?;
            ensure!(
                !path.chars().any(char::is_control),
                "Invalid installer path"
            );
            let command = format!("/usr/sbin/installer -pkg {} -target /", shell_quote(path));
            let script = format!(
                "do shell script {} with administrator privileges",
                serde_json::to_string(&command)?
            );
            ensure!(
                Command::new("/usr/bin/osascript")
                    .args(["-e", &script])
                    .status()?
                    .success(),
                "Package installation was cancelled or failed"
            );
        }
        _ => anyhow::bail!("This package cannot be installed automatically"),
    }
    Ok(())
}

fn cli_command(job: &Job) -> Command {
    if let Some(image) = &job.installation.app_image {
        let mut command = Command::new(image);
        command
            .arg("--axiom-cli")
            .env_remove("APPDIR")
            .env_remove("APPIMAGE");
        command
    } else {
        Command::new(&job.installation.cli)
    }
}

fn restart_arguments(restart: &Restart) -> Vec<OsString> {
    match restart {
        Restart::Desktop => vec![],
        Restart::Tui { cwd, resume } => {
            let mut args = vec!["tui".into(), "--cwd".into(), cwd.as_os_str().to_owned()];
            if let Some(id) = resume {
                args.extend(["--resume".into(), id.into()]);
            }
            args
        }
    }
}

fn relaunch(job: &Job) -> anyhow::Result<()> {
    match &job.restart {
        Restart::Desktop => {
            let mut command = if let Some(image) = &job.installation.app_image {
                Command::new(image)
            } else {
                let app = job
                    .installation
                    .desktop
                    .as_ref()
                    .context("Not a Desktop installation")?;
                if cfg!(target_os = "macos") {
                    let mut command = Command::new("/usr/bin/open");
                    command.arg("-n").arg(app);
                    command
                } else {
                    Command::new(app)
                }
            };
            command
                .env_remove("APPDIR")
                .env_remove("APPIMAGE")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            Ok(())
        }
        Restart::Tui { cwd, .. } => {
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt as _;
                Err(cli_command(job)
                    .args(restart_arguments(&job.restart))
                    .current_dir(cwd)
                    .exec()
                    .into())
            }
            // Windows' console launcher restarts after this helper returns.
            #[cfg(windows)]
            {
                let _ = cwd;
                Ok(())
            }
        }
    }
}

pub fn run(path: &Path, parent: Option<u32>, ready_file: bool) -> anyhow::Result<()> {
    let job = read_job(path)?;
    // A bounded acknowledgement is written only after the copied helper has
    // started and independently authenticated its complete staging input.
    if ready_file {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path.with_file_name("ready"))?
            .sync_all()?;
    }
    let result = (|| {
        wait_parent(parent)?;
        install_staged(path, super::TRUSTED_KEYS)?;
        Ok::<(), anyhow::Error>(())
    })();
    record_outcome(&job, &result)?;
    if let Err(error) = &result {
        eprintln!("Axiom update failed: {error}");
    }
    // A failed install also returns the user to their app where possible. The
    // persistent outcome explains failure; it never claims the target ran.
    if let Err(error) = relaunch(&job) {
        let failed = Err(error.context("Axiom could not restart"));
        record_outcome(&job, &failed)?;
        return failed;
    }
    result
}

pub(super) fn install_staged(path: &Path, trusted_keys: &str) -> anyhow::Result<()> {
    let guard = exclusive(&super::read_job_with_keys(path, trusted_keys)?)?;
    // Reverify after waiting: never install stale or changed staging bytes.
    let checked = super::read_job_with_keys(path, trusted_keys)?;
    let artifact = path
        .parent()
        .context("Missing update directory")?
        .join(&checked.artifact);
    let current = cli_command(&checked).arg("--version").output()?;
    let current_version = String::from_utf8_lossy(&current.stdout);
    ensure!(
        current.status.success(),
        "The installed CLI could not start"
    );
    if current_version.trim() != format!("axiomcli {}", checked.release.version) {
        ensure!(
            current_version.trim() == format!("axiomcli {}", env!("CARGO_PKG_VERSION")),
            "The installation changed while waiting; check for updates again"
        );
        install(&checked, &artifact)?;
    }
    let output = cli_command(&checked).arg("--version").output()?;
    ensure!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).trim()
                == format!("axiomcli {}", checked.release.version),
        "Installed version did not pass its startup check"
    );
    drop(guard);
    Ok(())
}

fn record_outcome(job: &Job, result: &anyhow::Result<()>) -> anyhow::Result<()> {
    let outcome = json!({"version":job.release.version, "success":result.is_ok(), "error":result.as_ref().err().map(ToString::to_string), "restartArguments":restart_arguments(&job.restart).iter().map(|s| s.to_string_lossy()).collect::<Vec<_>>()});
    let directory = cache(&job.installation)?;
    let mut pending = tempfile::NamedTempFile::new_in(&directory)?;
    pending.write_all(&serde_json::to_vec(&outcome)?)?;
    pending.as_file().sync_all()?;
    pending.persist(directory.join("last-update.json"))?;
    Ok(())
}
