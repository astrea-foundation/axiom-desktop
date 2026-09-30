use anyhow::{Context as _, ensure};
use axiom_installation::{Installation, discover_at, lock};
use axiom_setup::{PUBLISHER, TRUSTED_KEYS, needs_install, windows_installer};
use axiom_update_client::{accept_sequence, download_verified, latest_release, verify_file};
use fs2::FileExt as _;
use serde::Deserialize;
use std::os::windows::process::CommandExt as _;
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

pub enum Event {
    Status(String),
    Progress(u32),
    Applying,
    Complete(PathBuf),
    Error(String),
    Cancelled,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Registered {
    root: PathBuf,
    machine: bool,
}

struct Windows {
    script: tempfile::TempPath,
    powershell: PathBuf,
}
impl Windows {
    fn new() -> anyhow::Result<Self> {
        use std::io::Write as _;
        let mut script = tempfile::Builder::new()
            .prefix("axiom-setup-")
            .suffix(".ps1")
            .tempfile()?;
        script.write_all(include_bytes!("windows.ps1"))?;
        script.as_file().sync_all()?;
        // PowerShell opens scripts without sharing existing write handles on
        // Windows. Close ours while retaining automatic cleanup of the path.
        let script = script.into_temp_path();
        let powershell =
            PathBuf::from(std::env::var_os("WINDIR").context("Windows directory is unavailable")?)
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        ensure!(powershell.is_file(), "Windows PowerShell is unavailable");
        Ok(Self { script, powershell })
    }
    fn command(&self, action: &str) -> Command {
        let mut command = Command::new(&self.powershell);
        command
            .creation_flags(0x0800_0000)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&self.script)
            .args(["-Action", action]);
        command
    }
    fn output(command: &mut Command) -> anyhow::Result<Vec<u8>> {
        let output = command.output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
        Ok(output.stdout)
    }
    fn registered(&self) -> anyhow::Result<Option<Registered>> {
        Ok(serde_json::from_slice(&Self::output(
            &mut self.command("Inspect"),
        )?)?)
    }
    fn verify(&self, file: &Path) -> anyhow::Result<()> {
        Self::output(
            self.command("Verify")
                .arg("-FilePath")
                .arg(file)
                .arg("-Publisher")
                .arg(PUBLISHER),
        )?;
        Ok(())
    }
    fn running(&self, file: &Path) -> anyhow::Result<bool> {
        Ok(String::from_utf8(Self::output(
            self.command("Running").arg("-FilePath").arg(file),
        )?)?
        .trim()
            == "true")
    }
    fn install(&self, file: &Path, root: &Path, machine: bool) -> anyhow::Result<()> {
        let mut command = self.command("Install");
        command
            .arg("-FilePath")
            .arg(file)
            .arg("-Destination")
            .arg(axiom_installation::nsis_directory(root)?);
        if machine {
            command.arg("-Machine");
        }
        Self::output(&mut command)?;
        Ok(())
    }
}

pub fn self_check() -> anyhow::Result<()> {
    let windows = Windows::new()?;
    ensure!(
        Windows::output(&mut windows.command("Check"))? == b"ready\r\n",
        "Setup PowerShell helper did not respond"
    );
    Ok(())
}

fn installed(windows: &Windows, registered: &Registered) -> anyhow::Result<(Installation, String)> {
    let installation = discover_at(&registered.root.join("resources/bin/axiomcli.exe"), None)?;
    ensure!(
        installation.product == "desktop"
            && installation.format == "exe"
            && installation.root == registered.root.canonicalize()?,
        "Axiom installation ownership changed"
    );
    windows.verify(&installation.cli)?;
    let output = Command::new(&installation.cli)
        .creation_flags(0x0800_0000)
        .arg("--version")
        .output()?;
    ensure!(
        output.status.success(),
        "Couldn’t read the installed Axiom version"
    );
    let reported = String::from_utf8(output.stdout)?;
    let version = reported
        .trim()
        .strip_prefix("axiomcli ")
        .context("Unknown installed Axiom version")?;
    axiom_update_client::manifest::version_parts(version)?;
    Ok((installation, version.to_owned()))
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

pub async fn install_latest(
    cancel: Arc<AtomicBool>,
    applying: Arc<AtomicBool>,
    notify: impl Fn(Event),
) -> anyhow::Result<()> {
    let windows = Windows::new()?;
    let base = directories::BaseDirs::new().context("No user cache directory")?;
    let cache = base.cache_dir().join("axiom/setup");
    std::fs::create_dir_all(&cache)?;
    // Only one setup process may fetch/install at once for this user.
    let session = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache.join("setup.lock"))?;
    session
        .try_lock_exclusive()
        .context("Another Axiom setup is already running")?;
    notify(Event::Status("Checking the latest signed release…".into()));
    let release = tokio::select! {
        result = latest_release(TRUSTED_KEYS) => result?,
        () = cancelled(&cancel) => { notify(Event::Cancelled); return Ok(()); }
    };
    accept_sequence(&cache, &release, TRUSTED_KEYS)?;
    let file = windows_installer(&release)?;
    let registered = windows.registered()?;
    let existing = registered
        .as_ref()
        .map(|registered| installed(&windows, registered))
        .transpose()?;
    if !needs_install(
        &release,
        existing.as_ref().map(|(_, version)| version.as_str()),
    )? {
        notify(Event::Complete(
            existing
                .context("Missing existing installation")?
                .0
                .desktop
                .context("Missing Desktop")?,
        ));
        return Ok(());
    }
    let root = if let Some(registered) = &registered {
        registered.root.clone()
    } else {
        PathBuf::from(std::env::var_os("LOCALAPPDATA").context("No local application directory")?)
            // NSIS /D requires native Windows separators; a forward slash is
            // treated as an option delimiter rather than a path separator.
            .join("Programs")
            .join("Axiom")
    };
    if registered.is_none() && root.exists() {
        ensure!(
            std::fs::read_dir(&root)?.next().is_none(),
            "An unregistered installation already exists. Repair it with the offline installer."
        );
    }
    let directory = tempfile::Builder::new()
        .prefix("download-")
        .tempdir_in(&cache)?;
    let destination = directory.path().join(&file.name);
    notify(Event::Status(format!(
        "Downloading Axiom {}…",
        release.version
    )));
    tokio::select! {
        result = download_verified(file, &destination, |received,total| notify(Event::Progress(u32::try_from(received * 100 / total).unwrap_or(100)))) => result?,
        () = cancelled(&cancel) => { notify(Event::Cancelled); return Ok(()); }
    }
    notify(Event::Status("Verifying the installer’s publisher…".into()));
    windows.verify(&destination)?;
    let lease = existing
        .as_ref()
        .map(|(installation, _)| lock(installation))
        .transpose()?;
    if let Some(lease) = &lease {
        notify(Event::Status("Close Axiom and its CLI/proxy sessions to continue. Setup will wait without stopping your work.".into()));
        loop {
            if cancel.load(Ordering::Acquire) {
                notify(Event::Cancelled);
                return Ok(());
            }
            if !windows.running(&root.join("Axiom.exe"))? && lease.try_lock_exclusive().is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // A normal updater may have finished while we were waiting.
        let (_, version) = installed(
            &windows,
            registered
                .as_ref()
                .context("Missing registered installation")?,
        )?;
        if !needs_install(&release, Some(&version))? {
            notify(Event::Complete(root.join("Axiom.exe")));
            return Ok(());
        }
    }
    if cancel.load(Ordering::Acquire) {
        notify(Event::Cancelled);
        return Ok(());
    }
    // Recheck bytes and OS trust immediately before handing off to NSIS.
    verify_file(&destination, file)?;
    windows.verify(&destination)?;
    applying.store(true, Ordering::Release);
    notify(Event::Applying);
    windows.install(
        &destination,
        &root,
        registered.as_ref().is_some_and(|r| r.machine),
    )?;
    let registered = windows
        .registered()?
        .context("Installer did not register Axiom")?;
    ensure!(
        registered.root.canonicalize()? == root.canonicalize()?,
        "Installer changed destination"
    );
    let (installation, version) = installed(&windows, &registered)?;
    ensure!(
        version == release.version,
        "The installed version does not match the signed release"
    );
    notify(Event::Complete(
        installation.desktop.context("Missing installed Desktop")?,
    ));
    Ok(())
}

pub fn open(file: &Path) -> anyhow::Result<()> {
    Windows::new()?.verify(file)?;
    Command::new(file).spawn()?;
    Ok(())
}

pub fn show_licenses() -> anyhow::Result<()> {
    let base = directories::BaseDirs::new().context("No user cache directory")?;
    let directory = base.cache_dir().join("axiom/setup");
    std::fs::create_dir_all(&directory)?;
    let file = directory.join("licenses.txt");
    std::fs::write(&file, axiom_setup::LICENSES)?;
    let notepad =
        PathBuf::from(std::env::var_os("WINDIR").context("Windows directory is unavailable")?)
            .join("System32/notepad.exe");
    Command::new(notepad).arg(file).spawn()?;
    Ok(())
}
