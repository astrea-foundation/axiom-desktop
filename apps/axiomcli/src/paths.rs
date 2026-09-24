//! Platform paths shared by Axiom's command-line and desktop front ends.

use std::{
    ffi::OsString,
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
            // Honor explicit XDG roots on every OS. Besides being useful for
            // portable CLI installations, this keeps subprocess tests away
            // from the real macOS Keychain-adjacent and Windows profile data.
            config_root: platform_root_override(
                std::env::var_os("XDG_CONFIG_HOME"),
                base.config_dir(),
                "XDG_CONFIG_HOME",
            )?
            .join("axiom"),
            data_root: platform_root_override(
                std::env::var_os("XDG_DATA_HOME"),
                base.data_local_dir(),
                "XDG_DATA_HOME",
            )?
            .join("axiom"),
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

    pub fn account_data_dir(&self, account_id: &str) -> Result<PathBuf> {
        validate_account_id(account_id)?;
        Ok(self.data_root.join("accounts").join(account_id))
    }

    pub fn account_frontend_data_dir(
        &self,
        account_id: &str,
        frontend: FrontendKind,
    ) -> Result<PathBuf> {
        Ok(self.account_data_dir(account_id)?.join(match frontend {
            FrontendKind::Cli => "cli",
            FrontendKind::DesktopChat => "desktop",
        }))
    }

    pub fn account_state_database(
        &self,
        account_id: &str,
        frontend: FrontendKind,
    ) -> Result<PathBuf> {
        Ok(self
            .account_frontend_data_dir(account_id, frontend)?
            .join("state.sqlite3"))
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

    /// Create and resolve the account-isolated workspace used by Desktop.
    pub fn account_desktop_chat_cwd(&self, account_id: &str) -> Result<PathBuf> {
        let desktop_root = self.account_frontend_data_dir(account_id, FrontendKind::DesktopChat)?;
        prepare_owned_directory(&desktop_root)?;
        let chat = desktop_root.join("chat");
        prepare_owned_directory(&chat)?;
        let root = desktop_root.canonicalize()?;
        let chat = chat.canonicalize()?;
        if !chat.starts_with(&root) {
            return Err(AxiomError::OutsideWorkspace(chat));
        }
        Ok(chat)
    }

    /// Create a separate application-owned workspace for exactly one thread.
    pub fn account_desktop_thread_cwd(
        &self,
        account_id: &str,
        thread_id: &crate::app::SessionId,
    ) -> Result<PathBuf> {
        let root = self.account_desktop_chat_cwd(account_id)?;
        let directory = root.join(thread_id.to_string());
        prepare_owned_directory(&directory)?;
        let directory = directory.canonicalize()?;
        if directory.parent() != Some(root.as_path()) {
            return Err(AxiomError::OutsideWorkspace(directory));
        }
        Ok(directory)
    }

    /// Explicitly reset exactly one account/frontend database while the app is stopped.
    pub fn reset_account_frontend_state(
        &self,
        account_id: &str,
        frontend: FrontendKind,
    ) -> Result<Vec<PathBuf>> {
        let directory = self.account_frontend_data_dir(account_id, frontend)?;
        if !directory.exists() {
            return Ok(Vec::new());
        }
        ensure_directory(&self.data_root)?;
        ensure_directory(&self.data_root.join("accounts"))?;
        ensure_directory(&self.account_data_dir(account_id)?)?;
        ensure_directory(&directory)?;
        let database = directory.join("state.sqlite3");
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
        prepare_owned_directory(&self.config_root)?;
        prepare_owned_directory(&self.frontend_config_dir(FrontendKind::Cli))?;
        prepare_owned_directory(&self.frontend_config_dir(FrontendKind::DesktopChat))?;
        prepare_owned_directory(&self.data_root)?;
        let shared = self.data_root.join("shared");
        prepare_owned_directory(&shared)?;
        prepare_owned_directory(&self.data_root.join("accounts"))?;
        if let Some(parent) = self.identity_path().parent() {
            prepare_owned_directory(parent)?;
        }
        for path in [self.identity_path(), self.trust_policy_cache_path()] {
            ensure_regular_file_if_present(&path)?;
        }
        Ok(())
    }

    pub fn prepare_account(&self, account_id: &str) -> Result<()> {
        let account = self.account_data_dir(account_id)?;
        prepare_owned_directory(&account)?;
        for frontend in [FrontendKind::Cli, FrontendKind::DesktopChat] {
            let directory = self.account_frontend_data_dir(account_id, frontend)?;
            prepare_owned_directory(&directory)?;
            ensure_regular_file_if_present(&directory.join("state.sqlite3"))?;
        }
        Ok(())
    }
}

fn platform_root_override(
    value: Option<OsString>,
    default: &Path,
    variable: &str,
) -> Result<PathBuf> {
    let Some(value) = value else {
        return Ok(default.to_path_buf());
    };
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(AxiomError::Storage(format!(
            "{variable} must be an absolute path when set"
        )));
    }
    Ok(path)
}

fn validate_account_id(account_id: &str) -> Result<()> {
    if account_id.is_empty()
        || account_id.len() > 128
        || !account_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(AxiomError::Storage(
            "account identifier is not safe for account-scoped local storage".into(),
        ));
    }
    Ok(())
}

fn prepare_owned_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure_directory(path)?;
    set_owner_directory(path)?;
    Ok(())
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
    fn portable_root_overrides_require_absolute_paths() {
        let default = std::env::current_dir().expect("absolute current directory");
        let isolated = tempfile::tempdir().expect("isolated absolute root");
        assert_eq!(
            platform_root_override(None, &default, "TEST_ROOT").expect("default"),
            default
        );
        assert_eq!(
            platform_root_override(
                Some(isolated.path().as_os_str().to_owned()),
                &default,
                "TEST_ROOT",
            )
            .expect("absolute override"),
            isolated.path()
        );
        assert!(
            platform_root_override(
                Some(OsString::from("relative/config")),
                &default,
                "TEST_ROOT",
            )
            .is_err()
        );
    }

    #[test]
    fn account_and_frontend_paths_are_strictly_isolated() {
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
        let cli_a = paths
            .account_state_database("account-a", FrontendKind::Cli)
            .expect("account A CLI path");
        let desktop_a = paths
            .account_state_database("account-a", FrontendKind::DesktopChat)
            .expect("account A desktop path");
        let desktop_b = paths
            .account_state_database("account-b", FrontendKind::DesktopChat)
            .expect("account B desktop path");
        assert_ne!(cli_a, desktop_a);
        assert_ne!(desktop_a, desktop_b);
        assert_eq!(
            desktop_a,
            PathBuf::from("/data/axiom/accounts/account-a/desktop/state.sqlite3")
        );
        assert!(paths.identity_path().starts_with(paths.data_root()));
        for invalid in ["", ".", "../escape", "email@example.test", "wallet/0x1"] {
            assert!(paths.account_data_dir(invalid).is_err());
        }
    }

    #[test]
    fn desktop_chat_workspace_is_inside_the_exact_account() {
        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare");
        paths.prepare_account("account-a").expect("prepare account");
        let chat = paths
            .account_desktop_chat_cwd("account-a")
            .expect("chat cwd");
        assert!(
            chat.starts_with(
                paths
                    .account_frontend_data_dir("account-a", FrontendKind::DesktopChat)
                    .expect("desktop root")
                    .canonicalize()
                    .expect("root")
            )
        );
        let first = crate::app::SessionId::new();
        let second = crate::app::SessionId::new();
        let first_path = paths
            .account_desktop_thread_cwd("account-a", &first)
            .unwrap();
        assert_eq!(first_path.parent(), Some(chat.as_path()));
        assert_eq!(
            paths
                .account_desktop_thread_cwd("account-a", &first)
                .unwrap(),
            first_path
        );
        assert_ne!(
            paths
                .account_desktop_thread_cwd("account-a", &second)
                .unwrap(),
            first_path
        );
        paths.prepare_account("account-b").unwrap();
        assert_ne!(
            paths
                .account_desktop_thread_cwd("account-b", &first)
                .unwrap(),
            first_path
        );
    }

    #[test]
    fn reset_is_scoped_to_one_frontend_database() {
        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare");
        let cli = paths
            .account_state_database("account-a", FrontendKind::Cli)
            .unwrap();
        let desktop = paths
            .account_state_database("account-a", FrontendKind::DesktopChat)
            .unwrap();
        fs::create_dir_all(cli.parent().expect("CLI parent")).expect("CLI directory");
        fs::create_dir_all(desktop.parent().expect("desktop parent")).expect("desktop directory");
        fs::write(&cli, "cli").expect("CLI state");
        fs::write(cli.with_file_name("state.sqlite3-wal"), "wal").expect("CLI WAL");
        fs::write(&desktop, "desktop").expect("desktop state");
        paths.prepare_account("account-b").expect("prepare account");
        let account_state = paths
            .account_state_database("account-b", FrontendKind::Cli)
            .expect("account state");
        fs::write(&account_state, "account").expect("account state file");

        let removed = paths
            .reset_account_frontend_state("account-a", FrontendKind::Cli)
            .expect("reset CLI state");
        assert_eq!(removed.len(), 2);
        assert!(!cli.exists());
        assert!(desktop.exists());
        assert!(account_state.exists());
    }

    #[cfg(unix)]
    #[test]
    fn desktop_chat_workspace_rejects_a_symlink() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temp dir");
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().expect("prepare");
        paths.prepare_account("account-a").expect("prepare account");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).expect("outside");
        let chat = paths
            .account_frontend_data_dir("account-a", FrontendKind::DesktopChat)
            .expect("desktop root")
            .join("chat");
        fs::create_dir(&chat).expect("chat directory");
        fs::remove_dir(&chat).expect("remove chat directory");
        symlink(&outside, &chat).expect("chat symlink");
        assert!(paths.account_desktop_chat_cwd("account-a").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn desktop_thread_workspace_rejects_a_symlink() {
        let root = tempfile::tempdir().unwrap();
        let paths = AxiomPaths::from_roots(root.path().join("config"), root.path().join("data"));
        paths.prepare().unwrap();
        paths.prepare_account("account-a").unwrap();
        let chat = paths.account_desktop_chat_cwd("account-a").unwrap();
        let id = crate::app::SessionId::new();
        std::os::unix::fs::symlink(root.path(), chat.join(id.to_string())).unwrap();
        assert!(paths.account_desktop_thread_cwd("account-a", &id).is_err());
    }
}
