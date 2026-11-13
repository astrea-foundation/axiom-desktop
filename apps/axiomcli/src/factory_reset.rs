//! Offline recovery, independent of configuration, account or database initialization.
use std::{
    collections::{BTreeSet, HashMap},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::{Context as _, ensure};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use directories::BaseDirs;
use sha2::{Digest as _, Sha256};

use crate::{auth, desktop_mcp, paths::AxiomPaths};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Profile {
    #[default]
    System,
    Staging,
    Local,
}

#[derive(Debug, clap::Args)]
pub struct Arguments {
    /// Permanently erase all local Axiom accounts, settings and saved credentials.
    #[arg(long)]
    pub confirm: bool,
    #[arg(long, value_enum, default_value_t = Profile::System, hide = true)]
    pub profile: Profile,
    /// Desktop supplies its profile path; arbitrary deletion paths are rejected.
    #[arg(long, hide = true)]
    pub desktop_user_data: Option<PathBuf>,
    #[arg(long, hide = true)]
    pub restart_executable: Option<PathBuf>,
    #[arg(
        long,
        hide = true,
        requires = "restart_executable",
        allow_hyphen_values = true
    )]
    pub restart_arg: Vec<OsString>,
}

/// All native frontends hold a shared lease. Reset takes an exclusive lease
/// outside the deleted tree, so a CLI cannot recreate credentials or databases
/// while Desktop is resetting. The empty coordination file intentionally stays.
fn open_lease(paths: &AxiomPaths) -> anyhow::Result<File> {
    let parent = paths.data_root().parent().context("Missing data parent")?;
    fs::create_dir_all(parent)?;
    let path = parent.join(".axiom-factory-reset.lock");
    if let Ok(metadata) = fs::symlink_metadata(&path) {
        ensure!(
            metadata.is_file() && !metadata.is_symlink(),
            "Invalid Axiom reset lock"
        );
    }
    Ok(OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)?)
}

pub fn runtime_lease(paths: &AxiomPaths) -> anyhow::Result<File> {
    let lease = open_lease(paths)?;
    fs2::FileExt::try_lock_shared(&lease)
        .context("Axiom is being factory reset. Try again after it finishes.")?;
    Ok(lease)
}

fn reset_lease(paths: &AxiomPaths) -> anyhow::Result<File> {
    let lease = open_lease(paths)?;
    fs2::FileExt::try_lock_exclusive(&lease).context(
        "Close all other AxiomCLI and Axiom proxy processes, then try factory reset again.",
    )?;
    Ok(lease)
}

struct ResetPlan {
    paths: AxiomPaths,
    directories: BTreeSet<PathBuf>,
    profile: Profile,
}

impl ResetPlan {
    fn discover(profile: Profile, desktop_user_data: Option<&Path>) -> anyhow::Result<Self> {
        ensure!(
            profile == Profile::System || cfg!(debug_assertions),
            "Development profiles require a debug build"
        );
        let base = BaseDirs::new().context("Platform data directories are unavailable")?;
        let (paths, names) = match profile {
            Profile::System => (AxiomPaths::discover()?, vec!["Axiom", "axiom-desktop"]),
            Profile::Staging | Profile::Local => {
                let (directory, name) = if profile == Profile::Staging {
                    (".axiom-staging", "Axiom Staging")
                } else {
                    (".axiom-local", "Axiom Local")
                };
                let root = base.home_dir().join(directory);
                (
                    AxiomPaths::from_roots(root.join("config/axiom"), root.join("data/axiom")),
                    vec![name],
                )
            }
        };
        // Electron's appData follows XDG_CONFIG_HOME on Linux, APPDATA on
        // Windows and Application Support on macOS, independently of native XDG overrides.
        let desktop_root = base.config_dir();
        let mut directories =
            BTreeSet::from([paths.config_root().to_owned(), paths.data_root().to_owned()]);
        for name in names {
            let user_data = desktop_root.join(name);
            directories.insert(user_data.clone());
            directories.insert(base.cache_dir().join(name));
            #[cfg(target_os = "macos")]
            directories.insert(base.home_dir().join("Library/Logs").join(name));
        }
        if let Some(path) = desktop_user_data {
            ensure!(
                directories.contains(path),
                "The Desktop data path is not a standard Axiom profile; no data was deleted."
            );
        }
        if profile == Profile::System {
            directories.insert(base.cache_dir().join("axiom"));
        }
        Ok(Self {
            paths,
            directories,
            profile,
        })
    }

    fn erase(&self, erase_credentials: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
        let _lease = reset_lease(&self.paths)?;
        // Credentials go first. If the vault is locked, preserve files and
        // their scope references so the user can unlock it and retry.
        erase_credentials()?;
        for path in &self.directories {
            remove_owned_tree(path).with_context(|| {
                format!(
                    "Could not delete {}. Some local data may remain.",
                    path.display()
                )
            })?;
        }
        Ok(())
    }
}

fn remove_owned_tree(path: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(path) {
        // remove_dir_all does not follow symlinks inside the tree. Handle a
        // symlink at the root separately, too; never erase its destination.
        Ok(metadata) if metadata.is_symlink() => {
            #[cfg(windows)]
            {
                use std::os::windows::fs::FileTypeExt as _;
                if metadata.file_type().is_symlink_dir() {
                    return fs::remove_dir(path);
                }
            }
            fs::remove_file(path)
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn credential_search(service: &str) -> HashMap<&'static str, String> {
    #[cfg(windows)]
    {
        HashMap::from([("pattern", format!(r"\.{}$", service.replace('.', r"\.")))])
    }
    #[cfg(not(windows))]
    {
        HashMap::from([("service", service.to_owned())])
    }
}

fn erase_matching_credentials(
    search: impl Fn(&HashMap<&str, &str>) -> keyring_core::Result<Vec<keyring_core::Entry>>,
    include: impl Fn(&str, &keyring_core::Entry) -> anyhow::Result<bool>,
) -> anyhow::Result<()> {
    for service in [auth::KEYRING_SERVICE, desktop_mcp::KEYRING_SERVICE] {
        let spec = credential_search(service);
        let spec = spec
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        for entry in search(&spec).context(
            "Could not find Axiom credentials. Unlock your system credential store and retry.",
        )? {
            if include(service, &entry)? {
                match entry.delete_credential() {
                    Ok(()) | Err(keyring_core::Error::NoEntry) => {}
                    Err(_) => anyhow::bail!(
                        "Could not remove all Axiom credentials. Unlock your system credential store and retry."
                    ),
                }
            }
        }
    }
    Ok(())
}

fn erase_system_credentials(plan: &ResetPlan) -> anyhow::Result<()> {
    // Search Axiom's services rather than loading a session or parsing chat
    // databases. This also removes orphaned/retired MCP secrets and old logins.
    let scope = if plan.profile == Profile::System {
        None
    } else {
        Some(development_credential_scope(plan)?)
    };
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let result = (|| {
            keyring::Entry::store_status().as_ref().map_err(|_| {
                anyhow::anyhow!("System credential store is unavailable. Unlock it and retry.")
            })?;
            erase_matching_credentials(keyring_core::Entry::search, |service, entry| {
                let Some((login, mcp)) = &scope else {
                    return Ok(true);
                };
                #[cfg(windows)]
                let user = entry
                    .get_attributes()?
                    .remove("username")
                    .context("Missing credential account")?;
                #[cfg(not(windows))]
                let user = entry
                    .get_specifiers()
                    .map(|(_, user)| user)
                    .context("Missing credential account")?;
                Ok(if service == auth::KEYRING_SERVICE {
                    &user == login
                } else {
                    mcp.iter().any(|scope| user.starts_with(scope))
                })
            })
        })();
        let _ = send.send(result);
    });
    receive.recv_timeout(Duration::from_secs(30)).context(
        "The system credential store did not respond. Unlock it and retry factory reset.",
    )?
}

fn development_credential_scope(plan: &ResetPlan) -> anyhow::Result<(String, BTreeSet<String>)> {
    let origin = if plan.profile == Profile::Staging {
        "https://api-staging.axiom.stream".to_owned()
    } else {
        std::env::var("AXIOM_BASE_URL").context("Local profile requires an API origin")?
    };
    let origin = url::Url::parse(&origin)?;
    if plan.profile == Profile::Local {
        ensure!(
            matches!(origin.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")),
            "Local reset requires a loopback origin"
        );
    }
    let (login, _) = auth::credential_scope(&origin, &plan.paths.identity_path());
    let mut mcp = BTreeSet::new();
    let accounts = plan.paths.data_root().join("accounts");
    if accounts.try_exists()? {
        for account in fs::read_dir(accounts)? {
            let chat = account?.path().join("desktop/chat");
            if let Ok(chat) = chat.canonicalize() {
                let hash = Sha256::digest(chat.as_os_str().to_string_lossy().as_bytes());
                mcp.insert(format!("{hash:x}:"));
            }
        }
    }
    Ok((login, mcp))
}

pub fn command(arguments: &Arguments) -> anyhow::Result<()> {
    ensure!(
        arguments.confirm,
        "Refusing factory reset without --confirm. This permanently deletes all local Axiom chats, credentials and settings."
    );
    // Validate relaunch before changing anything. Only Desktop's trusted main
    // process supplies these arguments; renderer input never selects a path.
    if let Some(executable) = &arguments.restart_executable {
        ensure!(
            executable.is_absolute() && executable.is_file(),
            "Desktop restart executable is unavailable"
        );
    }
    let result = (|| {
        let plan = ResetPlan::discover(arguments.profile, arguments.desktop_user_data.as_deref())?;
        plan.erase(|| erase_system_credentials(&plan))
    })();
    if let Some(executable) = &arguments.restart_executable {
        let mut restart = Command::new(executable);
        restart
            .args(&arguments.restart_arg)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Err(error) = &result {
            let message = format!("{error:#}");
            let message: String = message.chars().take(2048).collect();
            restart.arg(format!(
                "--axiom-factory-reset-error={}",
                URL_SAFE_NO_PAD.encode(message)
            ));
        }
        restart
            .spawn()
            .context("Could not reopen Axiom. Start it manually.")?;
    }
    result?;
    println!("Factory reset complete. All local Axiom data and saved credentials were deleted.");
    Ok(())
}

#[cfg(test)]
mod tests;
