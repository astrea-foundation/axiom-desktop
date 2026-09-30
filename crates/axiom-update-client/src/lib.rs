//! Signed release inventory and verified transport shared by updates and setup.
pub mod manifest;

use anyhow::ensure;
use fs2::FileExt as _;
use manifest::{Artifact, MAX_FEED_BYTES, Release, parse_release, verify_release, version_parts};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read as _, Write as _},
    path::Path,
    time::Duration,
};

pub const RELEASE_API: &str = "https://axiom.stream/api/releases/latest";

pub fn release_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .build()?)
}

pub async fn read_release(mut response: reqwest::Response) -> anyhow::Result<Release> {
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

pub async fn latest_release(trusted_keys: &str) -> anyhow::Result<Release> {
    let release = read_release(
        release_client()?
            .get(RELEASE_API)
            .timeout(Duration::from_secs(20))
            .header("Accept", "application/json")
            .send()
            .await?,
    )
    .await?;
    verify_release(&release, trusted_keys)?;
    Ok(release)
}

/// Persist only verified feeds and reject signed rollbacks or changed releases.
pub fn accept_sequence(
    directory: &Path,
    release: &Release,
    trusted_keys: &str,
) -> anyhow::Result<()> {
    verify_release(release, trusted_keys)?;
    std::fs::create_dir_all(directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("feed.lock"))?;
    lock.lock_exclusive()?;
    let path = directory.join("accepted.json");
    if path.exists() {
        let previous = parse_release(&std::fs::read(&path)?)?;
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

pub fn verify_file(path: &Path, file: &Artifact) -> anyhow::Result<()> {
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

pub async fn download_verified(
    file: &Artifact,
    destination: &Path,
    mut progress: impl FnMut(u64, u64),
) -> anyhow::Result<()> {
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
            progress(received, file.bytes);
            last_percent = percent;
        }
    }
    output.sync_all()?;
    drop(output);
    verify_file(destination, file)
}
