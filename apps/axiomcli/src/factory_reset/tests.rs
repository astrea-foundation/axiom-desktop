use super::*;

fn isolated_plan(root: &Path) -> ResetPlan {
    let paths = AxiomPaths::from_roots(root.join("config/axiom"), root.join("data/axiom"));
    ResetPlan {
        directories: BTreeSet::from([
            paths.config_root().to_owned(),
            paths.data_root().to_owned(),
            root.join("electron/Axiom"),
            root.join("cache/axiom"),
        ]),
        paths,
        profile: Profile::System,
    }
}

#[test]
fn deletes_all_accounts_corrupt_config_identity_and_browser_storage_without_loading_them() {
    let root = tempfile::tempdir().unwrap();
    let plan = isolated_plan(root.path());
    for relative in [
        "config/axiom/config.toml",
        "config/axiom/desktop/config.toml",
        "config/axiom/cli/config.toml",
        "data/axiom/accounts/one/desktop/state.sqlite3",
        "data/axiom/accounts/one/desktop/state.sqlite3-wal",
        "data/axiom/accounts/two/cli/state.sqlite3",
        "data/axiom/accounts/two/desktop/chat/thread/attachment.png",
        "data/axiom/shared/identity/client-id",
        "data/axiom/shared/trust-policy.json",
        "electron/Axiom/Local Storage/leveldb/000001.log",
        "electron/Axiom/Network/Cookies",
        "cache/axiom/updates/pending/job.json",
    ] {
        let path = root.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "invalid old data, deliberately not parseable").unwrap();
    }
    let external = root.path().join("my-project.txt");
    fs::write(&external, "keep user files").unwrap();
    let other_app = root.path().join("electron/OtherApp");
    fs::create_dir_all(&other_app).unwrap();
    plan.erase(|| Ok(())).unwrap();
    assert!(plan.directories.iter().all(|path| !path.exists()));
    assert_eq!(fs::read_to_string(external).unwrap(), "keep user files");
    assert!(other_app.exists());
    plan.erase(|| Ok(())).unwrap(); // A missing/already-reset installation is safe.
}

#[test]
fn credential_failure_preserves_files_for_retry() {
    let root = tempfile::tempdir().unwrap();
    let plan = isolated_plan(root.path());
    fs::create_dir_all(plan.paths.data_root()).unwrap();
    let database = plan.paths.data_root().join("state.sqlite3");
    fs::write(&database, "saved chat").unwrap();
    assert!(plan.erase(|| anyhow::bail!("vault locked")).is_err());
    assert_eq!(fs::read_to_string(database).unwrap(), "saved chat");
}

#[cfg(unix)]
#[test]
fn filesystem_failure_reports_an_incomplete_reset_after_credential_cleanup() {
    use std::{cell::Cell, os::unix::fs::PermissionsExt as _};
    let root = tempfile::tempdir().unwrap();
    let plan = isolated_plan(root.path());
    let protected = plan.paths.config_root();
    fs::create_dir_all(protected).unwrap();
    fs::write(protected.join("config.toml"), "saved settings").unwrap();
    fs::set_permissions(protected, fs::Permissions::from_mode(0o500)).unwrap();
    let credentials_cleared = Cell::new(false);
    let result = plan.erase(|| {
        credentials_cleared.set(true);
        Ok(())
    });
    // Restore access even when an assertion fails, so the fixture is removable.
    fs::set_permissions(protected, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(credentials_cleared.get());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Some local data may remain")
    );
    assert!(protected.join("config.toml").exists());
}

#[test]
fn live_native_frontends_exclude_reset_and_reset_excludes_startup() {
    let root = tempfile::tempdir().unwrap();
    let plan = isolated_plan(root.path());
    let cli = runtime_lease(&plan.paths).unwrap();
    let proxy = runtime_lease(&plan.paths).unwrap();
    assert!(
        plan.erase(|| panic!("must fail before touching credentials"))
            .is_err()
    );
    drop(cli);
    assert!(reset_lease(&plan.paths).is_err());
    drop(proxy);
    let reset = reset_lease(&plan.paths).unwrap();
    assert!(runtime_lease(&plan.paths).is_err());
    drop(reset);
    plan.erase(|| Ok(())).unwrap();
}

#[cfg(unix)]
#[test]
fn root_and_nested_symlinks_are_removed_without_following_them() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let plan = isolated_plan(root.path());
    let outside = root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep.txt"), "keep").unwrap();
    fs::create_dir_all(plan.paths.config_root().parent().unwrap()).unwrap();
    symlink(&outside, plan.paths.config_root()).unwrap();
    fs::create_dir_all(plan.paths.data_root()).unwrap();
    symlink(&outside, plan.paths.data_root().join("external-workspace")).unwrap();
    plan.erase(|| Ok(())).unwrap();
    assert_eq!(
        fs::read_to_string(outside.join("keep.txt")).unwrap(),
        "keep"
    );
    assert!(fs::symlink_metadata(plan.paths.config_root()).is_err());
}

#[test]
fn confirmation_is_required_before_platform_or_credential_access() {
    let error = command(&Arguments {
        confirm: false,
        profile: Profile::System,
        desktop_user_data: None,
        restart_executable: None,
        restart_arg: vec![],
    })
    .unwrap_err();
    assert!(error.to_string().contains("without --confirm"));
}

#[test]
fn arbitrary_desktop_deletion_paths_are_rejected_before_any_mutation() {
    let root = tempfile::tempdir().unwrap();
    assert!(ResetPlan::discover(Profile::System, Some(root.path())).is_err());
}

#[cfg(not(windows))]
#[test]
fn vault_cleanup_erases_old_logins_and_orphaned_mcp_secrets_only_in_axiom_services() {
    use keyring_core::api::CredentialStoreApi as _;
    let store = keyring_core::mock::Store::new().unwrap();
    let login = store
        .build(auth::KEYRING_SERVICE, "old-native-session", None)
        .unwrap();
    let mcp = store
        .build(desktop_mcp::KEYRING_SERVICE, "orphaned:server:old-id", None)
        .unwrap();
    let other = store.build("other-app", "saved-account", None).unwrap();
    for entry in [&login, &mcp, &other] {
        entry.set_password("synthetic secret").unwrap();
    }
    erase_matching_credentials(|spec| store.search(spec), |_, _| Ok(true)).unwrap();
    assert!(matches!(
        login.get_password(),
        Err(keyring_core::Error::NoEntry)
    ));
    assert!(matches!(
        mcp.get_password(),
        Err(keyring_core::Error::NoEntry)
    ));
    assert_eq!(other.get_password().unwrap(), "synthetic secret");
}

#[cfg(windows)]
#[test]
fn windows_search_anchors_the_full_axiom_service_name() {
    assert_eq!(
        credential_search(auth::KEYRING_SERVICE)["pattern"],
        r"\.stream\.axiom\.axiomcli$"
    );
    assert_eq!(
        credential_search(desktop_mcp::KEYRING_SERVICE)["pattern"],
        r"\.stream\.axiom\.desktop\.mcp$"
    );
}
