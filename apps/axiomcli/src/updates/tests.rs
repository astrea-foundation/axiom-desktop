use super::*;
use std::fs;

#[test]
fn combined_installers_match_both_native_architectures() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/desktop-releases/universal-fixture.json"
    ))
    .unwrap();
    let release = parse_release(&serde_json::to_vec(&fixture["release"]).unwrap()).unwrap();
    verify_release(&release, fixture["publicKey"].as_str().unwrap()).unwrap();
    assert_eq!(release.downloads.len() + release.cli_downloads.len(), 8);
    for file in release.downloads.iter().chain(&release.cli_downloads) {
        if file.platform == "linux" {
            assert!(matches_target(file, "linux", "x64"));
            assert!(!matches_target(file, "linux", "arm64"));
        } else {
            assert!(matches_target(file, &file.platform, "x64"));
            assert!(matches_target(file, &file.platform, "arm64"));
            assert!(!matches_target(file, &file.platform, "unsupported"));
            assert!(!matches_target(file, "linux", "x64"));
        }
    }
    let mut changed = release;
    changed.downloads[0].arch = "x64".into();
    assert!(parse_release(&serde_json::to_vec(&changed).unwrap()).is_err());
}

#[test]
fn shared_release_contract() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/desktop-releases/contract-fixtures.json"
    ))
    .unwrap();
    for case in contract["cases"].as_array().unwrap() {
        let mut value = contract["release"].clone();
        for change in case["changes"].as_array().unwrap() {
            let path = change["path"].as_str().unwrap();
            if change["remove"] == true {
                let (parent, key) = path.rsplit_once('/').unwrap();
                value
                    .pointer_mut(parent)
                    .unwrap()
                    .as_object_mut()
                    .unwrap()
                    .remove(key);
            } else if let Some(count) = change["repeat"].as_u64() {
                let target = value.pointer_mut(path).unwrap();
                *target = serde_json::Value::Array(vec![
                    target[0].clone();
                    usize::try_from(count).unwrap()
                ]);
            } else {
                *value.pointer_mut(path).unwrap() = change["value"].clone();
            }
        }
        let result = parse_release(&serde_json::to_vec(&value).unwrap());
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {result:?}",
            case["name"]
        );
    }
}

fn signed_fixture() -> (Release, String) {
    let value: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/desktop-releases/signed-fixture.json"
    ))
    .unwrap();
    (
        parse_release(&serde_json::to_vec(&value["release"]).unwrap()).unwrap(),
        value["publicKey"].as_str().unwrap().to_owned(),
    )
}
#[test]
fn node_signature_verifies_in_rust_and_tampering_fails_closed() {
    let (release, key) = signed_fixture();
    verify_release(&release, &key).unwrap();
    verify_release(&release, &format!("{},{}", "c".repeat(64), key)).unwrap();
    assert!(verify_release(&release, "").is_err());
    assert!(verify_release(&release, &"c".repeat(64)).is_err());
    let mut changed = release.clone();
    changed.downloads[0].sha256 = "f".repeat(64);
    assert!(verify_release(&changed, &key).is_err());
    let mut changed = release.clone();
    changed.sequence += 1;
    assert!(verify_release(&changed, &key).is_err());
    let mut changed = release.clone();
    changed.cli_downloads.pop();
    assert!(verify_release(&changed, &key).is_err());
    let mut changed = release;
    changed.signing = "unsigned".into();
    changed.signature = None;
    assert!(verify_release(&changed, &key).is_err());
}
#[test]
fn numeric_versions_and_exact_installation_target() {
    assert!(version_parts("0.10.0").unwrap() > version_parts("0.9.9").unwrap());
    for value in [
        "01.2.3",
        "1.2",
        "1.2.3-beta",
        "1.2.3+build",
        "1000000000.0.0",
    ] {
        assert!(version_parts(value).is_err());
    }
    let (release, _) = signed_fixture();
    let format = if cfg!(windows) {
        "exe"
    } else if cfg!(target_os = "macos") {
        "pkg"
    } else {
        "sh"
    };
    let mut installation = Installation {
        product: "cli".into(),
        format: format.into(),
        root: "unused".into(),
        cli: "unused".into(),
        desktop: None,
        app_image: None,
    };
    let file = select(&release, &installation).unwrap();
    assert_eq!(file.product, "cli");
    assert_eq!(file.platform, host_platform());
    assert_eq!(file.arch, host_arch());
    installation.format = "unsupported".into();
    assert!(select(&release, &installation).is_err());
}
#[test]
fn verified_bytes_require_exact_size_and_digest() {
    let (release, _) = signed_fixture();
    let file = &release.downloads[0];
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("installer");
    fs::write(&path, b"an installer fixture").unwrap();
    verify_file(&path, file).unwrap();
    fs::write(&path, b"an installer fixturE").unwrap();
    assert!(verify_file(&path, file).is_err());
    fs::write(&path, b"truncated").unwrap();
    assert!(verify_file(&path, file).is_err());
    fs::write(&path, b"an installer fixture extra").unwrap();
    assert!(verify_file(&path, file).is_err());
}
#[tokio::test]
async fn download_rejects_corruption_truncation_and_redirects_without_credentials() {
    use axum::{Router, routing::get};
    let app = Router::new()
        .route(
            "/good",
            get(|headers: axum::http::HeaderMap| async move {
                assert!(!headers.contains_key("authorization"));
                assert!(!headers.contains_key("cookie"));
                "an installer fixture"
            }),
        )
        .route("/bad", get(|| async { "an installer fixturE" }))
        .route("/short", get(|| async { "short" }))
        .route(
            "/redirect",
            get(|| async { (axum::http::StatusCode::FOUND, [("location", "/good")]) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (release, _) = signed_fixture();
    let mut file = release.downloads[0].clone();
    let dir = tempfile::tempdir().unwrap();
    for route in ["good", "bad", "short", "redirect"] {
        file.url = format!("http://{address}/{route}");
        let result = download(&file, &dir.path().join(route), false).await;
        assert_eq!(result.is_ok(), route == "good", "{route}: {result:?}");
    }
    server.abort();
}
#[tokio::test]
async fn bounded_anonymous_transport_rejects_redirects_and_oversized_bodies() {
    use axum::{Router, http::StatusCode, response::IntoResponse as _, routing::get};
    let redirected = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = redirected.clone();
    let app = Router::new()
        .route(
            "/redirect",
            get(|| async { (StatusCode::FOUND, [("location", "/secret")]) }),
        )
        .route(
            "/secret",
            get(move || {
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    "unexpected redirect target"
                }
            }),
        )
        .route("/large", get(|| async { " ".repeat(MAX_FEED_BYTES + 1) }))
        .route(
            "/chunked-large",
            get(|| async {
                axum::body::Body::from_stream(futures::stream::iter(vec![
                    Ok::<
                        _,
                        std::convert::Infallible,
                    >(
                        vec![b' '; MAX_FEED_BYTES / 2 + 1]
                    );
                    2
                ]))
            }),
        )
        .route(
            "/feed",
            get(|headers: axum::http::HeaderMap| async move {
                assert!(!headers.contains_key("authorization"));
                assert!(!headers.contains_key("cookie"));
                let cases: serde_json::Value = serde_json::from_str(include_str!(
                    "../../../../packages/desktop-releases/contract-fixtures.json"
                ))
                .unwrap();
                axum::Json(cases["release"].clone()).into_response()
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = release_client().unwrap();
    for route in ["redirect", "large", "chunked-large"] {
        assert!(
            read_release(
                client
                    .get(format!("http://{address}/{route}"))
                    .send()
                    .await
                    .unwrap()
            )
            .await
            .is_err()
        );
    }
    let release = read_release(
        client
            .get(format!("http://{address}/feed"))
            .send()
            .await
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(release.version, "0.2.0");
    assert!(parse_release(&vec![b' '; MAX_FEED_BYTES + 1]).is_err());
    assert_eq!(redirected.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.abort();
}

fn sign_test_release(release: &mut Release) -> String {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use ed25519_dalek::Signer as _;
    let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    let public = key.verifying_key().to_bytes();
    release.signing = "signed".into();
    release.signature = None;
    let bytes = manifest::canonical_json(&serde_json::to_value(&*release).unwrap()).unwrap();
    release.signature = Some(manifest::ReleaseSignature {
        key_id: hex::encode(Sha256::digest(public))[..16].to_owned(),
        value: STANDARD.encode(key.sign(&bytes).to_bytes()),
    });
    hex::encode(public)
}
#[test]
fn remembered_signed_releases_reject_rollback_and_mutation() {
    let root = tempfile::tempdir().unwrap();
    let installation = Installation {
        product: "cli".into(),
        format: "sh".into(),
        root: root.path().into(),
        cli: "unused".into(),
        desktop: None,
        app_image: None,
    };
    let (mut release, key) = signed_fixture();
    accept_sequence_with_keys(&installation, &release, &key).unwrap();
    accept_sequence_with_keys(&installation, &release, &key).unwrap();
    release.sequence -= 1;
    sign_test_release(&mut release);
    assert!(
        accept_sequence_with_keys(&installation, &release, &key)
            .unwrap_err()
            .to_string()
            .contains("older")
    );
    release.sequence += 2;
    sign_test_release(&mut release);
    assert!(
        accept_sequence_with_keys(&installation, &release, &key)
            .unwrap_err()
            .to_string()
            .contains("immutable")
    );
    fs::remove_dir_all(cache(&installation).unwrap()).unwrap();
}
#[test]
#[cfg(target_os = "linux")]
fn native_helper_installs_a_verified_cli_pair_and_refuses_altered_staging() {
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::{
        os::unix::fs::{PermissionsExt as _, symlink},
        process::Command,
    };
    let version = version_parts(env!("CARGO_PKG_VERSION")).unwrap();
    let next = format!("{}.{}.{}", version[0], version[1], version[2] + 1);
    let root = tempfile::tempdir().unwrap();
    let prefix = root.path().join("prefix with spaces");
    let installed = prefix.join(format!("versions/{}", env!("CARGO_PKG_VERSION")));
    let payload = root.path().join("new");
    let marker = br#"{"schemaVersion":1,"product":"cli","format":"sh","versioned":true}"#;
    for (directory, version) in [
        (&installed, env!("CARGO_PKG_VERSION")),
        (&payload, next.as_str()),
    ] {
        fs::create_dir_all(directory.join("bin")).unwrap();
        fs::write(directory.join("axiom-install.json"), marker).unwrap();
        for name in ["axiomcli", "axiom-proxy"] {
            let file = directory.join("bin").join(name);
            fs::write(&file, format!("#!/bin/sh\nprintf '{name} {version}\\n'\n")).unwrap();
            fs::set_permissions(file, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    symlink(&installed, prefix.join("current")).unwrap();
    let installation = installation::discover_at(&installed.join("bin/axiomcli"), None).unwrap();
    let directory = tempfile::tempdir_in(cache(&installation).unwrap()).unwrap();
    let archive = Command::new("tar")
        .args(["-czf", "-", "-C"])
        .arg(&payload)
        .arg(".")
        .output()
        .unwrap();
    assert!(archive.status.success());
    // The normal installer has optional PATH registration. This integration
    // fixture keeps that part local while exercising the exact activation code.
    let header =
        include_str!("../../../../scripts/cli-installer.sh.in").replace("@VERSION@", &next);
    let header = header.replace(
        "bin_dir=\"${XDG_BIN_HOME:-$HOME/.local/bin}\"",
        &format!("bin_dir='{}'", root.path().join("commands").display()),
    );
    let bytes = format!("{header}{}\n", STANDARD.encode(archive.stdout)).into_bytes();
    let (mut release, _) = signed_fixture();
    let previous = release.version.clone();
    release.version.clone_from(&next);
    for file in release
        .downloads
        .iter_mut()
        .chain(release.cli_downloads.iter_mut())
    {
        file.name = file.name.replace(&previous, &next);
        file.url = file.url.replace(&previous, &next);
        file.github_url = file.github_url.replace(&previous, &next);
    }
    let artifact = release
        .cli_downloads
        .iter_mut()
        .find(|f| f.platform == "linux")
        .unwrap();
    artifact.bytes = bytes.len() as u64;
    artifact.sha256 = hex::encode(Sha256::digest(&bytes));
    let name = artifact.name.clone();
    let key = sign_test_release(&mut release);
    let path = directory.path().join("job.json");
    let job = Job {
        installation: installation.clone(),
        release,
        artifact: name.clone(),
        restart: Restart::Tui {
            cwd: root.path().into(),
            resume: Some("saved-session".into()),
        },
    };
    fs::write(&path, serde_json::to_vec(&job).unwrap()).unwrap();
    fs::write(directory.path().join(&name), b"corrupt").unwrap();
    assert!(apply::install_staged(&path, &key).is_err());
    assert_eq!(
        Command::new(&installation.cli)
            .arg("--version")
            .output()
            .unwrap()
            .stdout,
        format!("axiomcli {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
    fs::write(directory.path().join(&name), bytes).unwrap();
    apply::install_staged(&path, &key).unwrap();
    assert_eq!(
        Command::new(&installation.cli)
            .arg("--version")
            .output()
            .unwrap()
            .stdout,
        format!("axiomcli {next}\n").as_bytes()
    );
    assert_eq!(
        Command::new(prefix.join("current/bin/axiom-proxy"))
            .output()
            .unwrap()
            .stdout,
        format!("axiom-proxy {next}\n").as_bytes()
    );
    // A simultaneous updater observes that B is already active, without running
    // the installer twice or reporting a successful update as a failure.
    apply::install_staged(&path, &key).unwrap();
    drop(directory);
    fs::remove_dir_all(cache(&installation).unwrap()).unwrap();
}
