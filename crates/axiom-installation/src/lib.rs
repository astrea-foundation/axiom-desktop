use anyhow::{Context as _, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Marker {
    pub schema_version: u32,
    pub product: String,
    pub format: String,
    pub versioned: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Installation {
    pub product: String,
    pub format: String,
    pub root: PathBuf,
    pub cli: PathBuf,
    pub desktop: Option<PathBuf>,
    pub app_image: Option<PathBuf>,
}

pub fn discover_at(executable: &Path, app_image: Option<&Path>) -> anyhow::Result<Installation> {
    let cli = executable.canonicalize()?;
    let resources = cli
        .parent()
        .and_then(Path::parent)
        .context("Unknown installation")?;
    let marker: Marker =
        serde_json::from_slice(&std::fs::read(resources.join("axiom-install.json"))?)?;
    ensure!(
        marker.schema_version == 1 && matches!(marker.product.as_str(), "desktop" | "cli"),
        "Unknown installation owner"
    );
    let supported = if cfg!(windows) {
        marker.format == "exe"
    } else if cfg!(target_os = "macos") {
        marker.format == "pkg"
    } else {
        match marker.product.as_str() {
            "desktop" => matches!(marker.format.as_str(), "AppImage" | "deb" | "pacman"),
            _ => marker.format == "sh",
        }
    };
    ensure!(
        supported && marker.versioned == (marker.product == "cli" && marker.format == "sh"),
        "Unsupported installation format"
    );
    let desktop = if marker.product == "desktop" {
        let root = resources.parent().context("Unknown Desktop installation")?;
        Some(if cfg!(target_os = "macos") {
            root.parent().context("Unknown app bundle")?.to_path_buf()
        } else {
            root.join(if cfg!(windows) {
                "Axiom.exe"
            } else {
                "axiom-desktop"
            })
        })
    } else {
        None
    };
    let image = if marker.format == "AppImage" {
        Some(
            app_image
                .context("Run this AppImage through its installed launcher")?
                .canonicalize()?,
        )
    } else {
        None
    };
    let root = if let Some(image) = &image {
        image.clone()
    } else if marker.product == "desktop" {
        if cfg!(target_os = "macos") {
            desktop.clone().context("Missing app bundle")?
        } else {
            resources
                .parent()
                .context("Missing installation root")?
                .to_path_buf()
        }
    } else if marker.versioned {
        resources
            .parent()
            .and_then(Path::parent)
            .context("Invalid versioned installation")?
            .to_path_buf()
    } else {
        resources.to_path_buf()
    };
    let cli = if marker.versioned {
        root.join("current/bin/axiomcli")
    } else {
        resources.join("bin").join(if cfg!(windows) {
            "axiomcli.exe"
        } else {
            "axiomcli"
        })
    };
    Ok(Installation {
        product: marker.product,
        format: marker.format,
        root,
        cli,
        desktop,
        app_image: image,
    })
}

pub fn discover() -> anyhow::Result<Installation> {
    discover_at(
        &std::env::current_exe()?,
        std::env::var_os("APPIMAGE").as_deref().map(Path::new),
    )
}

pub fn cache(installation: &Installation) -> anyhow::Result<PathBuf> {
    let base = directories::BaseDirs::new().context("No user cache directory")?;
    let id = hex::encode(Sha256::digest(
        installation.root.to_string_lossy().as_bytes(),
    ));
    let directory = base
        .cache_dir()
        .join("axiom")
        .join("updates")
        .join(&id[..24]);
    std::fs::create_dir_all(&directory)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

pub fn lock(installation: &Installation) -> anyhow::Result<File> {
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache(installation)?.join("installation.lock"))?)
}

/// All installed CLI/ACP/proxy processes hold a shared lease. The installer
/// requires an exclusive lease after every old process has stopped.
pub fn lease() -> anyhow::Result<Option<File>> {
    let executable = std::env::current_exe()?;
    let executable = executable.canonicalize()?;
    let resources = executable
        .parent()
        .and_then(Path::parent)
        .context("Unknown executable directory")?;
    if !resources.join("axiom-install.json").try_exists()? {
        return Ok(None);
    }
    let installation = discover()?;
    let file = lock(&installation)?;
    fs2::FileExt::try_lock_shared(&file)
        .context("An Axiom update is installing; retry when it finishes")?;
    Ok(Some(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installed_cli_and_proxy_share_the_owner_and_exclusive_lease() {
        let fixture = tempfile::tempdir().unwrap();
        let root = fixture.path().canonicalize().unwrap();
        let versioned = cfg!(target_os = "linux");
        let payload = if versioned {
            root.join("versions/0.1.0")
        } else {
            root.clone()
        };
        std::fs::create_dir_all(payload.join("bin")).unwrap();
        let extension = if cfg!(windows) { ".exe" } else { "" };
        let cli = payload.join(format!("bin/axiomcli{extension}"));
        let proxy = payload.join(format!("bin/axiom-proxy{extension}"));
        std::fs::write(&cli, b"cli").unwrap();
        std::fs::write(&proxy, b"proxy").unwrap();
        let format = if cfg!(windows) {
            "exe"
        } else if cfg!(target_os = "macos") {
            "pkg"
        } else {
            "sh"
        };
        let marker = Marker {
            schema_version: 1,
            product: "cli".into(),
            format: format.into(),
            versioned,
        };
        std::fs::write(
            payload.join("axiom-install.json"),
            serde_json::to_vec(&marker).unwrap(),
        )
        .unwrap();
        let installation = discover_at(&cli, None).unwrap();
        assert_eq!(installation.root, root);
        assert_eq!(installation, discover_at(&proxy, None).unwrap());
        let a = lock(&installation).unwrap();
        let b = lock(&installation).unwrap();
        fs2::FileExt::try_lock_shared(&a).unwrap();
        assert!(fs2::FileExt::try_lock_exclusive(&b).is_err());
        drop(a);
        fs2::FileExt::try_lock_exclusive(&b).unwrap();
        let c = lock(&installation).unwrap();
        assert!(fs2::FileExt::try_lock_shared(&c).is_err());
        drop(b);
        drop(c);
        std::fs::remove_dir_all(cache(&installation).unwrap()).unwrap();
    }
    #[test]
    fn unknown_or_wrong_platform_marker_is_rejected() {
        let fixture = tempfile::tempdir().unwrap();
        std::fs::create_dir(fixture.path().join("bin")).unwrap();
        let executable = fixture.path().join("bin/axiomcli");
        std::fs::write(&executable, b"cli").unwrap();
        assert!(discover_at(&executable, None).is_err());
        std::fs::write(
            fixture.path().join("axiom-install.json"),
            br#"{"schemaVersion":1,"product":"cli","format":"zip","versioned":false}"#,
        )
        .unwrap();
        assert!(discover_at(&executable, None).is_err());
    }
}
