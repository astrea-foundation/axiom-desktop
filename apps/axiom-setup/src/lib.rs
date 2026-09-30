use anyhow::{Context as _, ensure};
use axiom_update_client::manifest::{Artifact, Release, version_parts};

pub const TRUSTED_KEYS: &str = match option_env!("AXIOM_UPDATE_PUBLIC_KEYS") {
    Some(keys) => keys,
    None => "",
};
pub const PUBLISHER: &str = match option_env!("AXIOM_SIGNING_PUBLISHER") {
    Some(name) => name,
    None => "Astrea Labs, Inc.",
};

/// The combined installer detects the native CPU even when setup is emulated.
pub fn windows_installer(release: &Release) -> anyhow::Result<&Artifact> {
    ensure!(
        release.schema_version == 3,
        "A combined Windows installer is required"
    );
    release
        .downloads
        .iter()
        .find(|file| {
            file.product == "desktop"
                && file.platform == "win"
                && file.arch == "universal"
                && file.format == "exe"
        })
        .context("No Windows installer has been published")
}

pub fn needs_install(release: &Release, installed_version: Option<&str>) -> anyhow::Result<bool> {
    let target = version_parts(&release.version)?;
    Ok(installed_version
        .map(version_parts)
        .transpose()?
        .is_none_or(|version| version < target))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn universal_package_is_selected_and_existing_versions_never_downgrade() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../packages/desktop-releases/universal-fixture.json"
        ))
        .unwrap();
        let release = axiom_update_client::manifest::parse_release(
            &serde_json::to_vec(&fixture["release"]).unwrap(),
        )
        .unwrap();
        axiom_update_client::manifest::verify_release(
            &release,
            fixture["publicKey"].as_str().unwrap(),
        )
        .unwrap();
        let artifact = windows_installer(&release).unwrap();
        assert_eq!(artifact.arch, "universal");
        assert_eq!(artifact.product, "desktop");
        assert!(needs_install(&release, None).unwrap());
        assert!(!needs_install(&release, Some(&release.version)).unwrap());
        assert!(!needs_install(&release, Some("999.0.0")).unwrap());
        assert!(needs_install(&release, Some("not-a-version")).is_err());
        let mut legacy = release.clone();
        legacy.schema_version = 2;
        assert!(windows_installer(&legacy).is_err());
        let mut missing = release;
        missing.downloads.retain(|file| file.platform != "win");
        assert!(windows_installer(&missing).is_err());
    }
}
