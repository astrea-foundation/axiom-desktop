use assert_cmd::cargo::cargo_bin_cmd;

#[test]
fn native_proxy_rejects_inherited_automation_credentials_before_startup() {
    let output = cargo_bin_cmd!("axiomcli")
        .args(["desktop-proxy", "--account-id", "test-account"])
        .env("AXIOM_API_KEY", "test-upstream-credential-never-print")
        .env(
            "AXIOM_PROXY_TOKEN",
            "test-local-token-never-print-0123456789",
        )
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("requires a native account session"));
    assert!(!error.contains("test-upstream-credential-never-print"));
    assert!(!error.contains("test-local-token-never-print"));
}

#[test]
fn desktop_proxy_is_available_in_the_bundled_cli() {
    cargo_bin_cmd!("axiomcli")
        .args(["desktop-proxy", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--account-id"))
        .stdout(predicates::str::contains("--bind"));
}
