//! Platform paths shared by Axiom's command-line and desktop front ends.

use std::{
    fmt, fs,
    path::{Path, PathBuf},
};

use clap::ValueEnum;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};

use crate::{AxiomError, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum FrontendKind {
    #[default]
    Cli,
    DesktopChat,
}

impl fmt::Display for FrontendKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Cli => "cli",
            Self::DesktopChat => "desktop-chat",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AxiomPaths {
    config_root: PathBuf,
    data_root: PathBuf,
}

impl AxiomPaths {
    pub fn discover() -> Result<Self> {
        let base = BaseDirs::new().ok_or_else(|| {
            AxiomError::Storage("platform config and data directories are unavailable".into())
        })?;
        Ok(Self {
            config_root: base.config_dir().join("axiom"),
            data_root: base.data_local_dir().join("axiom"),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_roots(config_root: PathBuf, data_root: PathBuf) -> Self {
        Self {
            config_root,
            data_root,
        }
    }

    #[must_use]
    pub fn config_root(&self) -> &Path {
        &self.config_root
    }

    #[must_use]
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    #[must_use]
    pub fn shared_config_path(&self) -> PathBuf {
        self.config_root.join("config.toml")
    }

    #[must_use]
    pub fn frontend_config_dir(&self, frontend: FrontendKind) -> PathBuf {
        self.config_root.join(match frontend {
            FrontendKind::Cli => "cli",
            FrontendKind::DesktopChat => "desktop",
        })
    }

    #[must_use]
    pub fn frontend_config_path(&self, frontend: FrontendKind) -> PathBuf {
        self.frontend_config_dir(frontend).join("config.toml")
    }

    #[must_use]
    pub fn frontend_data_dir(&self, frontend: FrontendKind) -> PathBuf {
        self.data_root.join(match frontend {
            FrontendKind::Cli => "cli",
            FrontendKind::DesktopChat => "desktop",
        })
    }

    #[must_use]
    pub fn state_database(&self, frontend: FrontendKind) -> PathBuf {
        self.frontend_data_dir(frontend).join("state.sqlite3")
    }

    #[must_use]
    pub fn credentials_dir(&self) -> PathBuf {
        self.data_root.join("shared").join("credentials")
    }

    #[must_use]
    pub fn identity_path(&self) -> PathBuf {
        self.data_root
            .join("shared")
            .join("identity")
            .join("client-id")
    }

    #[must_use]
    pub fn trust_policy_cache_path(&self) -> PathBuf {
        self.data_root.join("shared").join("trust-policy.json")
    }

    /// Create and resolve the only workspace used by the desktop chat profile.
    pub fn desktop_chat_cwd(&self) -> Result<PathBuf> {
        let desktop_root = self.frontend_data_dir(FrontendKind::DesktopChat);
        let chat = desktop_root.join("chat");
        fs::create_dir_all(&chat)?;
        ensure_directory(&chat)?;
        let root = desktop_root.canonicalize()?;
        let chat = chat.canonicalize()?;
        if !chat.starts_with(&root) {
            return Err(AxiomError::OutsideWorkspace(chat));
        }
        Ok(chat)
    }

    /// Explicitly delete only one frontend's development state database and
    /// `SQLite` sidecars. Shared identity, credentials, trust policy, config,
    /// and the desktop chat directory are never included.
    pub fn reset_frontend_state(&self, frontend: FrontendKind) -> Result<Vec<PathBuf>> {
        let directory = self.frontend_data_dir(frontend);
        ensure_directory(&directory)?;
        let database = self.state_database(frontend);
        let mut removed = Vec::new();
        for path in [
            database.clone(),
            database.with_file_name("state.sqlite3-wal"),
            database.with_file_name("state.sqlite3-shm"),
            database.with_file_name("state.sqlite3-journal"),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_dir() => {
                    return Err(AxiomError::Storage(format!(
                        "local state file is unexpectedly a directory: {}",
                        path.display()
                    )));
                }
                Ok(_) => {
                    fs::remove_file(&path)?;
                    removed.push(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(removed)
    }

    pub fn prepare(&self) -> Result<()> {
        fs::create_dir_all(&self.config_root)?;
        ensure_directory(&self.config_root)?;
        fs::create_dir_all(self.frontend_config_dir(FrontendKind::Cli))?;
        fs::create_dir_all(self.frontend_config_dir(FrontendKind::DesktopChat))?;
        ensure_directory(&self.frontend_config_dir(FrontendKind::Cli))?;
        ensure_directory(&self.frontend_config_dir(FrontendKind::DesktopChat))?;
        fs::create_dir_all(&self.data_root)?;
        ensure_directory(&self.data_root)?;
        fs::create_dir_all(self.frontend_data_dir(FrontendKind::Cli))?;
        fs::create_dir_all(self.frontend_data_dir(FrontendKind::DesktopChat))?;
        ensure_directory(&self.frontend_data_dir(FrontendKind::Cli))?;
        ensure_directory(&self.frontend_data_dir(FrontendKind::DesktopChat))?;
        let desktop_chat = self
            .frontend_data_dir(FrontendKind::DesktopChat)
            .join("chat");
        fs::create_dir_all(&desktop_chat)?;
        ensure_directory(&desktop_chat)?;
        let shared = self.data_root.join("shared");
        fs::create_dir_all(&shared)?;
        ensure_directory(&shared)?;
        fs::create_dir_all(self.credentials_dir())?;
        ensure_directory(&self.credentials_dir())?;
        set_owner_directory(&self.data_root)?;
        set_owner_directory(&self.frontend_data_dir(FrontendKind::Cli))?;
        set_owner_directory(&self.frontend_data_dir(FrontendKind::DesktopChat))?;
        set_owner_directory(&desktop_chat)?;
        set_owner_directory(&self.credentials_dir())?;
        if let Some(parent) = self.identity_path().parent() {
            fs::create_dir_all(parent)?;
            ensure_directory(parent)?;
            set_owner_directory(parent)?;
        }
        for path in [
            self.state_database(FrontendKind::Cli),
            self.state_database(FrontendKind::DesktopChat),
            self.credentials_dir().join("api-key"),
            self.identity_path(),
            self.trust_policy_cache_path(),
        ] {
            ensure_regular_file_if_present(&path)?;
        }
        Ok(())
    }
}

#[cfg(unix)]
fn set_owner_directory(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_owner_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AxiomError::Storage(format!(
            "Axiom directory must be a real directory, not a symlink: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_regular_file_if_present(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(AxiomError::Storage(format!(
                "Axiom data file must be a regular non-symlink file: {}",
                path.display()
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontends_share_a_root_but_not_state_databases() {
        let paths =
            AxiomPaths::from_roots(PathBuf::from("/config/axiom"), PathBuf::from("/data/axiom"));
        assert_eq!(
            paths.shared_config_path(),
            PathBuf::from("/config/axiom/config.toml")
        );
        assert_eq!(
            paths.frontend_config_path(FrontendKind::Cli),
            PathBuf::from("/config/axiom/cli/config.toml")
        );
        assert_eq!(
            paths.frontend_config_path(FrontendKind::DesktopChat),
            PathBuf::from("/config/axiom/desktop/config.toml")
        );
        assert_ne!(
            paths.state_database(FrontendKind::Cli),
            paths.state_database(FrontendKind::DesktopChat)
        );
        assert!(paths.identity_path().starts_with(paths.data_root()));
    }

    #[test]
    fn desktop_chat_workspace_is_inside_desktop_data() {
        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare");
        let chat = paths.desktop_chat_cwd().expect("chat cwd");
        assert!(
            chat.starts_with(
                paths
                    .frontend_data_dir(FrontendKind::DesktopChat)
                    .canonicalize()
                    .expect("root")
            )
        );
    }

    #[test]
    fn reset_is_scoped_to_one_frontend_database() {
        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare");
        let cli = paths.state_database(FrontendKind::Cli);
        let desktop = paths.state_database(FrontendKind::DesktopChat);
        fs::write(&cli, "cli").expect("CLI state");
        fs::write(cli.with_file_name("state.sqlite3-wal"), "wal").expect("CLI WAL");
        fs::write(&desktop, "desktop").expect("desktop state");
        let chat = paths.desktop_chat_cwd().expect("chat");

        let removed = paths
            .reset_frontend_state(FrontendKind::Cli)
            .expect("reset CLI state");
        assert_eq!(removed.len(), 2);
        assert!(!cli.exists());
        assert!(desktop.exists());
        assert!(chat.exists());
    }

    #[cfg(unix)]
    #[test]
    fn desktop_chat_workspace_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let chat = paths
            .frontend_data_dir(FrontendKind::DesktopChat)
            .join("chat");
        fs::remove_dir(&chat).expect("remove prepared chat directory");
        symlink(&outside, &chat).expect("chat symlink");
        assert!(paths.desktop_chat_cwd().is_err());
    }
}
