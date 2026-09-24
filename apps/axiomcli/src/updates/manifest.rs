use std::collections::HashSet;

use anyhow::{Context as _, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

pub const MAX_FEED_BYTES: usize = 128 * 1024;
pub const MAX_ARTIFACT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Artifact {
    pub name: String,
    pub product: String,
    pub platform: String,
    pub arch: String,
    pub format: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
    pub github_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseSignature {
    pub key_id: String,
    pub value: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub schema_version: u32,
    pub version: String,
    pub revision: String,
    pub sequence: u64,
    pub signing: String,
    pub downloads: Vec<Artifact>,
    pub cli_downloads: Vec<Artifact>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<ReleaseSignature>,
}

pub fn version_parts(version: &str) -> anyhow::Result<[u32; 3]> {
    let parts = version.split('.').collect::<Vec<_>>();
    ensure!(parts.len() == 3, "Invalid release version");
    let mut result = [0; 3];
    for (i, part) in parts.iter().enumerate() {
        ensure!(
            !part.is_empty()
                && part.len() <= 9
                && part.bytes().all(|c| c.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0')),
            "Invalid release version"
        );
        result[i] = part.parse()?;
    }
    Ok(result)
}

fn lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

pub fn parse_release(bytes: &[u8]) -> anyhow::Result<Release> {
    ensure!(bytes.len() <= MAX_FEED_BYTES, "Release feed is too large");
    let release: Release = serde_json::from_slice(bytes)?;
    ensure!(
        matches!(release.schema_version, 2 | 3)
            && release.sequence > 0
            && release.sequence <= 9_007_199_254_740_991
            && lower_hex(&release.revision, 40)
            && matches!(release.signing.as_str(), "signed" | "unsigned")
            && !release.downloads.is_empty()
            && release.downloads.len() + release.cli_downloads.len() <= 100,
        "Invalid release manifest"
    );
    version_parts(&release.version)?;
    match (&release.signature, release.signing.as_str()) {
        (Some(signature), "signed") => {
            ensure!(
                lower_hex(&signature.key_id, 16) && STANDARD.decode(&signature.value)?.len() == 64,
                "Invalid release signature"
            );
        }
        (None, "unsigned") => {}
        _ => anyhow::bail!("Missing release signature"),
    }
    let mut seen = HashSet::new();
    for (product, files) in [
        ("desktop", &release.downloads),
        ("cli", &release.cli_downloads),
    ] {
        for file in files {
            let allowed: &[&str] = match (product, file.platform.as_str()) {
                (_, "mac") => &["pkg"],
                (_, "win") => &["exe"],
                ("desktop", "linux") => &["AppImage", "deb", "pacman"],
                ("cli", "linux") => &["sh"],
                _ => &[],
            };
            let format =
                if file.name.ends_with(".pkg.tar.xz") || file.name.ends_with(".pkg.tar.zst") {
                    "pacman"
                } else {
                    file.name.rsplit('.').next().unwrap_or_default()
                };
            let marker = |a: &str| {
                file.name.contains(&format!("-{a}.")) || file.name.contains(&format!("-{a}-"))
            };
            let arch = if marker("universal") {
                "universal"
            } else if marker("arm64") {
                "arm64"
            } else if ["x64", "x86_64", "amd64"].iter().any(|a| marker(a)) {
                "x64"
            } else {
                ""
            };
            ensure!(
                file.product == product
                    && file.name.len() <= 200
                    && file
                        .name
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_alphanumeric)
                    && file
                        .name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                    && file.name.contains(&format!("-{}-", release.version))
                    && match (release.schema_version, file.platform.as_str()) {
                        (3, "linux") => file.arch == "x64",
                        (3, _) => file.arch == "universal",
                        _ => matches!(file.arch.as_str(), "x64" | "arm64"),
                    }
                    && arch == file.arch
                    && allowed.contains(&format)
                    && file.format == format
                    && file.bytes > 0
                    && file.bytes <= MAX_ARTIFACT_BYTES
                    && lower_hex(&file.sha256, 64)
                    && file.url
                        == format!(
                            "https://cdn.axiom.stream/downloads/{}/{}",
                            release.version, file.name
                        )
                    && file.github_url
                        == format!(
                            "https://github.com/astrea-foundation/axiom-releases/releases/download/v{}/{}",
                            release.version, file.name
                        )
                    && seen.insert((product, &file.platform, &file.arch, &file.format)),
                "Invalid release artifact"
            );
        }
    }
    Ok(release)
}

pub fn verify_release(release: &Release, trusted_keys: &str) -> anyhow::Result<()> {
    ensure!(
        release.signing == "signed",
        "Automatic updates require a signed release"
    );
    let signature = release
        .signature
        .as_ref()
        .context("Missing release signature")?;
    let key = trusted_keys
        .split(',')
        .filter_map(|hex| hex::decode(hex.trim()).ok())
        .find(|key| {
            key.len() == 32 && hex::encode(Sha256::digest(key)).starts_with(&signature.key_id)
        })
        .context("This build does not trust the update signing key")?;
    let key: [u8; 32] = key
        .try_into()
        .map_err(|_| anyhow::anyhow!("Invalid update key"))?;
    let mut value = serde_json::to_value(release)?;
    value
        .as_object_mut()
        .context("Invalid release")?
        .remove("signature");
    let payload = canonical_json(&value)?;
    let signature = Signature::from_slice(&STANDARD.decode(&signature.value)?)?;
    VerifyingKey::from_bytes(&key)?
        .verify_strict(&payload, &signature)
        .context("Invalid update signature")
}

pub(super) fn canonical_json(value: &serde_json::Value) -> anyhow::Result<Vec<u8>> {
    fn write(value: &serde_json::Value, out: &mut Vec<u8>) -> anyhow::Result<()> {
        match value {
            serde_json::Value::Object(map) => {
                out.push(b'{');
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                for (i, key) in keys.into_iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    serde_json::to_writer(&mut *out, key)?;
                    out.push(b':');
                    write(&map[key], out)?;
                }
                out.push(b'}');
            }
            serde_json::Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    write(item, out)?;
                }
                out.push(b']');
            }
            _ => serde_json::to_writer(out, value)?,
        }
        Ok(())
    }
    let mut out = Vec::new();
    write(value, &mut out)?;
    Ok(out)
}
