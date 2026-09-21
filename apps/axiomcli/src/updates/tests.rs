use super::*;
use std::{cmp::Ordering, fs};

fn fixture() -> Release {
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../packages/desktop-releases/contract-fixtures.json"
    ))
    .unwrap();
    parse_release(&serde_json::to_vec(&cases["release"]).unwrap()).unwrap()
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

#[test]
fn stable_numeric_versions_never_offer_a_downgrade() {
    assert_eq!(
        compare_versions("0.10.0", "0.9.9").unwrap(),
        Ordering::Greater
    );
    assert_eq!(compare_versions("0.1.2", "0.1.2").unwrap(), Ordering::Equal);
    assert_eq!(compare_versions("0.1.1", "0.1.2").unwrap(), Ordering::Less);
    for value in [
        "01.2.3",
        "1.2",
        "1.2.3-beta",
        "1.2.3+build",
        "1000000000.0.0",
    ] {
        assert!(compare_versions(value, "1.2.3").is_err());
    }
}

#[test]
fn routes_bundled_and_standalone_installations_without_mixing_artifacts() {
    let root = tempfile::tempdir().unwrap();
    for resources in ["resources", "Resources"] {
        let path = root.path().join(resources);
        fs::create_dir_all(path.join("bin")).unwrap();
        fs::write(path.join("app.asar"), b"desktop").unwrap();
        fs::write(path.join("bin/axiomcli"), b"cli").unwrap();
        assert_eq!(
            installation_at(&path.join("bin/axiomcli")),
            Installation::Desktop
        );
        #[cfg(unix)]
        {
            let link = root.path().join(format!("{resources}-link"));
            std::os::unix::fs::symlink(path.join("bin/axiomcli"), &link).unwrap();
            assert_eq!(installation_at(&link), Installation::Desktop);
        }
    }
    fs::create_dir_all(root.path().join("bin")).unwrap();
    let executable = root.path().join("bin/axiomcli");
    fs::write(&executable, b"cli").unwrap();
    assert_eq!(installation_at(&executable), Installation::Development);
    fs::create_dir_all(root.path().join("share/axiomcli")).unwrap();
    fs::write(
        root.path().join("share/axiomcli/install-manifest"),
        b"installation",
    )
    .unwrap();
    assert_eq!(installation_at(&executable), Installation::Standalone);

    let release = fixture();
    let bundled = update_message(&release, Installation::Desktop, "linux", "x86_64");
    assert!(bundled.contains("Settings → Updates"));
    assert!(bundled.contains("all AxiomCLI sessions"));
    assert!(!bundled.contains(".tar.gz"));
    let standalone = update_message(&release, Installation::Standalone, "linux", "x86_64");
    assert!(standalone.contains(&release.cli_downloads[0].url));
    assert!(standalone.contains(&release.cli_downloads[0].sha256));
    assert!(standalone.contains("CLI + local proxy"));
    assert!(
        update_message(&release, Installation::Standalone, "linux", "aarch64")
            .contains("No standalone bundle")
    );
    assert!(
        update_message(
            &Release {
                version: release.version,
                cli_downloads: vec![]
            },
            Installation::Standalone,
            "linux",
            "x86_64"
        )
        .contains("No standalone bundle")
    );
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
    assert_eq!(release.version, "0.1.3");
    assert!(parse_release(&vec![b' '; MAX_FEED_BYTES + 1]).is_err());
    assert_eq!(redirected.load(std::sync::atomic::Ordering::SeqCst), 0);
    server.abort();
}
