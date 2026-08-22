#![cfg(target_os = "linux")]

use std::{os::unix::fs::PermissionsExt as _, path::Path, process::Command};

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
}

fn run(command: &mut Command, label: &str) -> std::process::Output {
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{label}: {error}"));
    assert!(
        output.status.success(),
        "{label} failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn package(stub: &Path, dist: &Path, gpg_home: Option<&Path>) -> std::path::PathBuf {
    let mut command = Command::new(workspace_root().join("scripts/package-release"));
    command
        .env("AXIOMCLI_DIST_DIR", dist)
        .env("AXIOMCLI_PACKAGE_BINARY", stub)
        .env("SOURCE_DATE_EPOCH", "1700000000");
    if let Some(gpg_home) = gpg_home {
        command
            .arg("--sign")
            .env("GNUPGHOME", gpg_home)
            .env("AXIOMCLI_SIGNING_KEY", "release-test@axiom.invalid");
    }
    let output = run(&mut command, "package release");
    let path = String::from_utf8(output.stdout).expect("artifact path");
    std::path::PathBuf::from(path.trim())
}

#[test]
fn release_archive_is_reproducible_signed_installable_and_cleanly_uninstalled() {
    let fixture = tempfile::tempdir().expect("fixture");
    let stub = fixture.path().join("axiomcli");
    std::fs::write(&stub, "#!/usr/bin/bash\nprintf 'axiomcli 0.1.0\\n'\n")
        .expect("write executable fixture");
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
        .expect("make fixture executable");

    let first_dist = fixture.path().join("dist-a");
    let second_dist = fixture.path().join("dist-b");
    let first = package(&stub, &first_dist, None);
    let second = package(&stub, &second_dist, None);
    assert_eq!(
        std::fs::read(&first).expect("first archive"),
        std::fs::read(&second).expect("second archive"),
        "fixed-input artifacts must be byte-for-byte reproducible"
    );

    let gpg_home = fixture.path().join("gnupg");
    std::fs::create_dir(&gpg_home).expect("GPG home");
    std::fs::set_permissions(&gpg_home, std::fs::Permissions::from_mode(0o700))
        .expect("GPG permissions");
    run(
        Command::new("gpg").args([
            "--batch",
            "--homedir",
            gpg_home.to_str().expect("GPG path"),
            "--passphrase",
            "",
            "--quick-gen-key",
            "AxiomCLI Release Test <release-test@axiom.invalid>",
            "ed25519",
            "sign",
            "1d",
        ]),
        "create ephemeral signing key",
    );
    let signed_dist = fixture.path().join("dist-signed");
    let signed = package(&stub, &signed_dist, Some(&gpg_home));
    let signature = signed.with_extension("gz.asc");
    assert!(signature.is_file(), "detached signature missing");
    run(
        Command::new(workspace_root().join("scripts/verify-release"))
            .arg(&signed)
            .arg(&signature)
            .env("GNUPGHOME", &gpg_home),
        "verify signed release",
    );

    let prefix = fixture.path().join("install-root/usr/local");
    run(
        Command::new(workspace_root().join("scripts/install-release"))
            .arg(&signed)
            .arg(&prefix),
        "install release",
    );
    let installed = prefix.join("bin/axiomcli");
    assert_eq!(
        String::from_utf8(
            run(
                Command::new(&installed).arg("--version"),
                "start installed binary"
            )
            .stdout
        )
        .expect("version output"),
        "axiomcli 0.1.0\n"
    );
    assert!(
        prefix
            .join("share/doc/axiomcli/third-party/MANIFEST.tsv")
            .is_file(),
        "installed dependency notice manifest is missing"
    );
    if Path::new("/usr/bin/bwrap").is_file() {
        run(
            Command::new("/usr/bin/bwrap").args([
                "--die-with-parent",
                "--unshare-all",
                "--ro-bind",
                "/usr",
                "/usr",
                "--ro-bind",
                "/bin",
                "/bin",
                "--ro-bind",
                "/lib",
                "/lib",
                "--ro-bind",
                "/lib64",
                "/lib64",
                "--ro-bind",
                "/etc",
                "/etc",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--ro-bind",
                prefix.to_str().expect("prefix"),
                "/opt/axiomcli",
                "--",
                "/opt/axiomcli/bin/axiomcli",
                "--version",
            ]),
            "start release in clean mount/network namespace",
        );
    }
    run(
        Command::new(workspace_root().join("scripts/uninstall-release")).arg(&prefix),
        "uninstall release",
    );
    assert!(!installed.exists());
    assert!(!prefix.join("share/doc/axiomcli").exists());
}
