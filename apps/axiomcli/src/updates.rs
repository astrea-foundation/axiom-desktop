//! Anonymous release discovery shared with Desktop's public release contract.
//! Bundled CLI installations are upgraded only by their Desktop installer.

use std::{collections::HashSet, path::Path, time::Duration};

use anyhow::{Context as _, ensure};
use serde::Deserialize;

const RELEASE_API: &str = "https://axiom.stream/api/releases/latest";
const MAX_FEED_BYTES: usize = 128 * 1024;
const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct Artifact {
    name: String,
    platform: String,
    arch: String,
    bytes: u64,
    sha256: String,
    url: String,
}

#[derive(Debug)]
struct Release {
    version: String,
    cli_downloads: Vec<Artifact>,
}

fn version_parts(version: &str) -> anyhow::Result<[u32; 3]> {
    let parts = version.split('.').collect::<Vec<_>>();
    ensure!(parts.len() == 3, "Invalid release version");
    let mut result = [0; 3];
    for (index, part) in parts.iter().enumerate() {
        ensure!(
            !part.is_empty()
                && part.len() <= 9
                && part.bytes().all(|c| c.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0')),
            "Invalid release version"
        );
        result[index] = part.parse()?;
    }
    Ok(result)
}

fn compare_versions(left: &str, right: &str) -> anyhow::Result<std::cmp::Ordering> {
    Ok(version_parts(left)?.cmp(&version_parts(right)?))
}

fn desktop_format(name: &str) -> Option<&'static str> {
    if name.ends_with(".pkg.tar.xz") || name.ends_with(".pkg.tar.zst") {
        return Some("pacman");
    }
    ["AppImage", "deb", "dmg", "pkg", "zip", "exe"]
        .into_iter()
        .find(|format| name.ends_with(&format!(".{format}")))
}

/// Validate the complete manifest, including artifacts not selected by this host.
/// The same schema-1 fixtures are exercised by JavaScript and Rust tests.
fn parse_release(bytes: &[u8]) -> anyhow::Result<Release> {
    ensure!(bytes.len() <= MAX_FEED_BYTES, "Release feed is too large");
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = value["version"]
        .as_str()
        .context("Missing release version")?;
    version_parts(version)?;
    ensure!(
        value.get("schemaVersion").is_none() || value["schemaVersion"] == 1,
        "Invalid release schema"
    );
    ensure!(
        matches!(value["signing"].as_str(), Some("signed" | "unsigned")),
        "Invalid release signing"
    );
    let downloads = value["downloads"]
        .as_array()
        .context("Invalid release downloads")?;
    let empty = Vec::new();
    let cli = match value.get("cliDownloads") {
        Some(files) => files.as_array().context("Invalid CLI downloads")?,
        None => &empty,
    };
    ensure!(
        !downloads.is_empty() && downloads.len() + cli.len() <= 100,
        "Invalid release downloads"
    );
    let mut seen = HashSet::new();
    let mut cli_downloads = Vec::new();
    for (raw, standalone) in downloads
        .iter()
        .map(|f| (f, false))
        .chain(cli.iter().map(|f| (f, true)))
    {
        let file: Artifact = serde_json::from_value(raw.clone())?;
        ensure!(
            file.name.len() <= 200
                && file
                    .name
                    .as_bytes()
                    .first()
                    .is_some_and(u8::is_ascii_alphanumeric)
                && file
                    .name
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                && file.name.contains(&format!("-{version}-"))
                && matches!(file.arch.as_str(), "x64" | "arm64")
                && file.bytes > 0
                && file.bytes <= MAX_ARTIFACT_BYTES
                && file.sha256.len() == 64
                && file
                    .sha256
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                && file.url
                    == format!("https://cdn.axiom.stream/downloads/{version}/{}", file.name),
            "Invalid release artifact"
        );
        if standalone {
            ensure!(
                file.platform == "linux"
                    && file.arch == "x64"
                    && raw["format"] == "tar.gz"
                    && file.name == format!("axiomai-{version}-x86_64-unknown-linux-gnu.tar.gz")
                    && seen.insert("cli:linux:x64".to_owned()),
                "Invalid CLI download"
            );
            cli_downloads.push(file);
        } else {
            let format = desktop_format(&file.name).context("Invalid installer format")?;
            let allowed = match file.platform.as_str() {
                "mac" => ["pkg", "dmg", "zip"].contains(&format),
                "win" => format == "exe",
                "linux" => ["AppImage", "deb", "pacman"].contains(&format),
                _ => false,
            };
            let has_arch_marker = |arch: &str| {
                file.name.contains(&format!("-{arch}.")) || file.name.contains(&format!("-{arch}-"))
            };
            let named_arch = if has_arch_marker("arm64") {
                "arm64"
            } else if ["x64", "x86_64", "amd64"].into_iter().any(has_arch_marker) {
                "x64"
            } else {
                ""
            };
            ensure!(
                allowed
                    && named_arch == file.arch
                    && (raw.get("format").is_none() || raw["format"] == format)
                    && seen.insert(format!("{}:{}:{format}", file.platform, file.arch)),
                "Invalid release download"
            );
        }
    }
    Ok(Release {
        version: version.to_owned(),
        cli_downloads,
    })
}

async fn read_release(mut response: reqwest::Response) -> anyhow::Result<Release> {
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "Release feed is unavailable"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= MAX_FEED_BYTES as u64),
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

fn release_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()?)
}

async fn latest_release() -> anyhow::Result<Release> {
    // A fresh client carries no account headers, cookies, model traffic or API key.
    read_release(
        release_client()?
            .get(RELEASE_API)
            .header("Accept", "application/json")
            .send()
            .await?,
    )
    .await
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Installation {
    Desktop,
    Standalone,
    Development,
}

fn installation_at(executable: &Path) -> Installation {
    // Resolve PATH symlinks before inspecting the owning installation. AppImage
    // wrappers execute the binary inside the mounted resources/bin directory.
    let Ok(executable) = executable.canonicalize() else {
        return Installation::Development;
    };
    let Some(bin) = executable.parent() else {
        return Installation::Development;
    };
    let Some(root) = bin.parent() else {
        return Installation::Development;
    };
    if bin.file_name().is_some_and(|name| name == "bin") {
        if root
            .file_name()
            .is_some_and(|name| name == "resources" || name == "Resources")
            && (root.join("app.asar").is_file() || root.join("app/package.json").is_file())
        {
            return Installation::Desktop;
        }
        if root.join("share/axiomcli/install-manifest").is_file()
            || root.join("share/doc/axiomcli/BUILD-METADATA").is_file()
        {
            return Installation::Standalone;
        }
    }
    Installation::Development
}

fn installation() -> Installation {
    std::env::current_exe().map_or(Installation::Development, |path| installation_at(&path))
}

fn update_message(release: &Release, installed: Installation, os: &str, arch: &str) -> String {
    let intro = format!(
        "Axiom {} is available (installed {}).",
        release.version,
        env!("CARGO_PKG_VERSION")
    );
    match installed {
        Installation::Desktop => format!(
            "{intro}\nOpen Axiom → Settings → Updates. The desktop installer updates Desktop and its bundled CLI together. Quit Axiom and all AxiomCLI sessions before installing."
        ),
        Installation::Standalone => {
            if os == "linux"
                && arch == "x86_64"
                && let Some(file) = release
                    .cli_downloads
                    .iter()
                    .find(|f| f.platform == "linux" && f.arch == "x64")
            {
                return format!(
                    "{intro}\nCLI + local proxy bundle: {}\nSize: {} bytes\nSHA-256: {}\nChecksum file: {}.sha256\nQuit all AxiomCLI and local proxy sessions. Download and verify the bundle, then use scripts/install-release ARCHIVE PREFIX with the same installation prefix to replace both components together.",
                    file.url, file.bytes, file.sha256, file.url
                );
            }
            format!(
                "{intro}\nNo standalone bundle has been published for this installation. Check again after the matching bundle is published."
            )
        }
        Installation::Development => format!(
            "{intro}\nThis executable has no recognized installer. Update your source checkout and rebuild Desktop and AxiomCLI together, or install a matching Axiom release."
        ),
    }
}

pub async fn run() -> anyhow::Result<()> {
    let release = latest_release()
        .await
        .context("Could not check Axiom updates")?;
    if compare_versions(&release.version, env!("CARGO_PKG_VERSION"))?.is_gt() {
        println!(
            "{}",
            update_message(
                &release,
                installation(),
                std::env::consts::OS,
                std::env::consts::ARCH
            )
        );
    } else {
        println!(
            "Axiom {} is up to date (published {}).",
            env!("CARGO_PKG_VERSION"),
            release.version
        );
    }
    Ok(())
}

/// No account access or terminal output; callers decide how to display a notice.
pub async fn startup_notice() -> anyhow::Result<Option<String>> {
    let installed = installation();
    if installed == Installation::Development {
        return Ok(None);
    }
    let release = latest_release().await?;
    if !compare_versions(&release.version, env!("CARGO_PKG_VERSION"))?.is_gt() {
        return Ok(None);
    }
    Ok(Some(update_message(
        &release,
        installed,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )))
}

#[cfg(test)]
mod tests;
