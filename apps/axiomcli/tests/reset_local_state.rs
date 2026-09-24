use std::process::Command;

#[test]
fn explicit_reset_removes_only_selected_obsolete_account_database_before_authentication() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let selected = data.join("axiom/accounts/account-a/desktop/state.sqlite3");
    let other = data.join("axiom/accounts/account-b/desktop/state.sqlite3");
    for path in [&selected, &other] {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        rusqlite::Connection::open(path).unwrap().execute_batch(
            "CREATE TABLE threads(id TEXT); PRAGMA application_id=1096302898; PRAGMA user_version=7;"
        ).unwrap();
    }
    let reset = |confirm: bool| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_axiomcli"));
        command
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_DATA_HOME", &data)
            .env_remove("AXIOM_API_KEY")
            .args([
                "reset-local-state",
                "--account",
                "account-a",
                "--frontend",
                "desktop-chat",
            ]);
        if confirm {
            command.arg("--confirm");
        }
        command.output().unwrap()
    };
    assert!(!reset(false).status.success());
    assert!(selected.exists());
    let result = reset(true);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!selected.exists());
    assert!(other.exists());
    assert!(reset(true).status.success());
}
