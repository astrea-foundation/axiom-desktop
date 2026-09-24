//! One native update path for Desktop and terminal installations.
mod apply;
mod installation;
mod manifest;

use anyhow::{Context as _, ensure};
use fs2::FileExt as _;
use installation::{Installation, cache, discover};
use manifest::{Artifact, MAX_FEED_BYTES, Release, parse_release, verify_release, version_parts};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

pub use installation::lease;
const RELEASE_API: &str = "https://axiom.stream/api/releases/latest";
const TRUSTED_KEYS: &str = match option_env!("AXIOM_UPDATE_PUBLIC_KEYS") {
    Some(keys) => keys,
    None => "",
};

#[derive(Debug, Default, clap::Args)]
#[allow(clippy::struct_excessive_bools)] // Independent Clap switches; incompatible operations are rejected.
pub struct Arguments {
    /// Check for a newer version without installing it.
    #[arg(long)]
    pub check: bool,
    /// Emit newline-delimited JSON for the Desktop update controller.
    #[arg(long)]
    pub json: bool,
    #[arg(long, hide = true)]
    pub prepare: bool,
    #[arg(long, hide = true)]
    pub desktop: bool,
    #[arg(long, hide = true)]
    pub start_job: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub apply_job: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub parent: Option<u32>,
    #[arg(long, hide = true)]
    pub trust_keys: bool,
    #[arg(long, hide = true, requires = "apply_job")]
    pub ready_file: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "frontend", rename_all = "camelCase", deny_unknown_fields)]
pub enum Restart {
    Desktop,
    Tui {
        cwd: PathBuf,
        resume: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Job {
    installation: Installation,
    release: Release,
    artifact: String,
    restart: Restart,
}

fn emit(event: &serde_json::Value, machine: bool) {
    if machine {
        println!("{event}");
    } else if let Some(message) = event["message"].as_str() {
        eprintln!("{message}");
    }
}

fn release_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()?)
}

async fn read_release(mut response: reqwest::Response) -> anyhow::Result<Release> {
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "Release feed is unavailable"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|n| n <= MAX_FEED_BYTES as u64),
        "Release feed is too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len() + chunk.len() <= MAX_FEED_BYTES,
            "Release feed is too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    parse_release(&bytes)
}

async fn latest_release() -> anyhow::Result<Release> {
    let release = read_release(
        release_client()?
            .get(RELEASE_API)
            .timeout(Duration::from_secs(20))
            .header("Accept", "application/json")
            .send()
            .await?,
    )
    .await?;
    verify_release(&release, TRUSTED_KEYS)?;
    Ok(release)
}

fn accept_sequence(installation: &Installation, release: &Release) -> anyhow::Result<()> {
    accept_sequence_with_keys(installation, release, TRUSTED_KEYS)
}
fn accept_sequence_with_keys(
    installation: &Installation,
    release: &Release,
    trusted_keys: &str,
) -> anyhow::Result<()> {
    verify_release(release, trusted_keys)?;
    let directory = cache(installation)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("feed.lock"))?;
    lock.lock_exclusive()?;
    let path = directory.join("accepted.json");
    if path.exists() {
        let previous: Release = parse_release(&std::fs::read(&path)?)?;
        verify_release(&previous, trusted_keys)?;
        ensure!(
            release.sequence >= previous.sequence
                && version_parts(&release.version)? >= version_parts(&previous.version)?,
            "Refusing an older update feed"
        );
        if release.sequence == previous.sequence || release.version == previous.version {
            ensure!(
                serde_json::to_value(release)? == serde_json::to_value(&previous)?,
                "An immutable release changed"
            );
        }
    }
    let mut pending = tempfile::NamedTempFile::new_in(directory)?;
    pending.write_all(&serde_json::to_vec(release)?)?;
    pending.as_file().sync_all()?;
    pending.persist(&path)?;
    Ok(())
}

fn host_platform() -> &'static str {
    if cfg!(windows) {
        "win"
    } else if cfg!(target_os = "macos") {
        "mac"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else {
        "unsupported"
    }
}
fn host_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x64"
    } else {
        "unsupported"
    }
}

fn matches_target(file: &Artifact, platform: &str, arch: &str) -> bool {
    file.platform == platform
        && matches!(arch, "x64" | "arm64")
        && (file.arch == arch || (matches!(platform, "mac" | "win") && file.arch == "universal"))
}

fn select<'a>(release: &'a Release, installation: &Installation) -> anyhow::Result<&'a Artifact> {
    let files = if installation.product == "desktop" {
        &release.downloads
    } else {
        &release.cli_downloads
    };
    files
        .iter()
        .find(|f| {
            matches_target(f, host_platform(), host_arch()) && f.format == installation.format
        })
        .context("No update has been published for this installation")
}

fn verify_file(path: &Path, file: &Artifact) -> anyhow::Result<()> {
    let mut input = File::open(path)?;
    ensure!(
        input.metadata()?.is_file() && input.metadata()?.len() == file.bytes,
        "Update size mismatch"
    );
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    ensure!(
        hex::encode(hash.finalize()) == file.sha256,
        "Update checksum mismatch"
    );
    Ok(())
}

async fn download(file: &Artifact, destination: &Path, machine: bool) -> anyhow::Result<()> {
    let mut response = release_client()?
        .get(&file.url)
        .timeout(Duration::from_secs(1800))
        .send()
        .await?;
    ensure!(
        response.status() == reqwest::StatusCode::OK
            && response.content_length().is_none_or(|n| n == file.bytes),
        "Update download is unavailable or changed"
    );
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut received = 0u64;
    let mut last_percent = 101;
    while let Some(chunk) = response.chunk().await? {
        received += chunk.len() as u64;
        ensure!(received <= file.bytes, "Update exceeds expected size");
        output.write_all(&chunk)?;
        let percent = received * 100 / file.bytes;
        if percent != last_percent {
            emit(
                &json!({"event":"progress", "name":file.name, "received":received, "total":file.bytes,
                "message":format!("Downloading update: {percent}%")}),
                machine,
            );
            last_percent = percent;
        }
    }
    output.sync_all()?;
    drop(output);
    verify_file(destination, file)?;
    Ok(())
}

async fn prepare(
    installation: Installation,
    release: Release,
    restart: Restart,
    machine: bool,
) -> anyhow::Result<PathBuf> {
    ensure!(
        version_parts(&release.version)? > version_parts(env!("CARGO_PKG_VERSION"))?,
        "Already up to date"
    );
    let file = select(&release, &installation)?;
    let directory = tempfile::Builder::new()
        .prefix("pending-")
        .tempdir_in(cache(&installation)?)?;
    download(file, &directory.path().join(&file.name), machine).await?;
    let helper = directory.path().join(if cfg!(windows) {
        "update-helper.exe"
    } else {
        "update-helper"
    });
    std::fs::copy(std::env::current_exe()?, &helper)?;
    #[cfg(windows)]
    {
        let current = std::env::current_exe()?;
        for entry in std::fs::read_dir(current.parent().context("Missing runtime directory")?)? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
            {
                std::fs::copy(entry.path(), directory.path().join(entry.file_name()))?;
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700))?;
    }
    let job = Job {
        artifact: file.name.clone(),
        installation,
        release,
        restart,
    };
    let path = directory.path().join("job.json");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    output.write_all(&serde_json::to_vec(&job)?)?;
    output.sync_all()?;
    let _ = directory.keep();
    emit(
        &json!({"event":"ready", "job":path, "message":"Update verified. Ready to install."}),
        machine,
    );
    Ok(path)
}

fn read_job(path: &Path) -> anyhow::Result<Job> {
    read_job_with_keys(path, TRUSTED_KEYS)
}
fn read_job_with_keys(path: &Path, trusted_keys: &str) -> anyhow::Result<Job> {
    ensure!(
        std::fs::metadata(path)?.len() <= 256 * 1024,
        "Invalid update job"
    );
    let job: Job = serde_json::from_slice(&std::fs::read(path)?)?;
    parse_release(&serde_json::to_vec(&job.release)?)?;
    ensure!(
        path.canonicalize()?.starts_with(cache(&job.installation)?),
        "Update job is outside its private cache"
    );
    if let Some(image) = &job.installation.app_image {
        ensure!(
            image.is_absolute()
                && image.canonicalize()? == *image
                && *image == job.installation.root
                && job.installation.product == "desktop"
                && job.installation.format == "AppImage",
            "Installation changed"
        );
    } else {
        ensure!(
            installation::discover_at(&job.installation.cli, None)? == job.installation,
            "Installation changed"
        );
    }
    ensure!(
        !matches!(job.restart, Restart::Desktop) || job.installation.product == "desktop",
        "Wrong restart frontend"
    );
    verify_release(&job.release, trusted_keys)?;
    accept_sequence_with_keys(&job.installation, &job.release, trusted_keys)?;
    ensure!(
        version_parts(&job.release.version)? > version_parts(env!("CARGO_PKG_VERSION"))?,
        "Refusing update downgrade"
    );
    let artifact = select(&job.release, &job.installation)?;
    ensure!(artifact.name == job.artifact, "Update target changed");
    verify_file(
        &path
            .parent()
            .context("Invalid update job path")?
            .join(&job.artifact),
        artifact,
    )?;
    Ok(job)
}

pub async fn command(arguments: Arguments) -> anyhow::Result<()> {
    let modes = [
        arguments.check,
        arguments.prepare,
        arguments.start_job.is_some(),
        arguments.apply_job.is_some(),
        arguments.trust_keys,
    ];
    ensure!(
        modes.into_iter().filter(|x| *x).count() <= 1,
        "Choose one update operation"
    );
    ensure!(
        arguments.parent.is_none()
            || arguments.start_job.is_some()
            || arguments.apply_job.is_some(),
        "Invalid parent option"
    );
    ensure!(
        !arguments.desktop || arguments.prepare,
        "Desktop requires prepare mode"
    );
    if arguments.trust_keys {
        println!("{TRUSTED_KEYS}");
        return Ok(());
    }
    if let Some(path) = arguments.apply_job {
        return apply::run(&path, arguments.parent, arguments.ready_file);
    }
    if let Some(path) = arguments.start_job {
        return apply::spawn(&path, arguments.parent);
    }
    let installation = discover()
        .context("Use an installed Axiom package to update; source builds must be rebuilt")?;
    let release = latest_release().await?;
    accept_sequence(&installation, &release)?;
    let newer = version_parts(&release.version)? > version_parts(env!("CARGO_PKG_VERSION"))?;
    emit(
        &json!({"event":"checked", "release":release, "installation":installation, "available":newer, "lastError":last_error(&installation),
        "message":if newer { format!("Axiom {} is available.", release.version) } else { "Axiom is up to date.".into() }}),
        arguments.json,
    );
    if arguments.check || !newer {
        return Ok(());
    }
    let restart = if arguments.desktop {
        Restart::Desktop
    } else {
        Restart::Tui {
            cwd: std::env::current_dir()?,
            resume: None,
        }
    };
    let path = prepare(installation, release, restart, arguments.json).await?;
    if arguments.prepare {
        return Ok(());
    }
    apply::handoff(&path)
}

pub async fn run_with_resume(cwd: PathBuf, resume: String) -> anyhow::Result<()> {
    let installation = discover()?;
    let release = latest_release().await?;
    accept_sequence(&installation, &release)?;
    if version_parts(&release.version)? <= version_parts(env!("CARGO_PKG_VERSION"))? {
        eprintln!("Axiom is up to date.");
        return Ok(());
    }
    let path = prepare(
        installation,
        release,
        Restart::Tui {
            cwd,
            resume: Some(resume),
        },
        false,
    )
    .await?;
    apply::handoff(&path)
}

pub async fn startup_notice() -> anyhow::Result<Option<String>> {
    let Ok(installation) = discover() else {
        return Ok(None);
    };
    if let Some(error) = last_error(&installation) {
        return Ok(Some(format!(
            "Previous update failed: {error}. Use /update to retry."
        )));
    }
    let release = latest_release().await?;
    accept_sequence(&installation, &release)?;
    Ok(
        (version_parts(&release.version)? > version_parts(env!("CARGO_PKG_VERSION"))?).then(|| {
            format!(
                "Axiom {} is available. Use /update to install and restart.",
                release.version
            )
        }),
    )
}

fn last_error(installation: &Installation) -> Option<String> {
    let bytes = std::fs::read(cache(installation).ok()?.join("last-update.json")).ok()?;
    if bytes.len() > 16 * 1024 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if value["success"] == false {
        value["error"].as_str().map(str::to_owned)
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
