//! Update checks, reminders and installing a release, against a fake
//! GitHub that serves signed releases.

// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

mod common;

use axum::{Json, Router, routing::get};
use base64ct::{Base64, Encoding as _};
use blake2::Digest as _;
use reqwest::StatusCode;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::json;
use sha2::Sha256;
use sideporch::updates::{Source, TARGET, install::archive_name};

use common::{Server, admin, invite, start_with};

/// Signs like `minisign -S`: prehashed with BLAKE2b-512.
fn sign(seed: u8, data: &[u8], trusted_comment: &str) -> (String, String) {
    let pair = Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap();
    let keynum = [seed, 1, 2, 3, 4, 5, 6, 7];
    let public = [b"Ed".as_slice(), &keynum, pair.public_key().as_ref()].concat();
    let signature = pair.sign(&blake2::Blake2b512::digest(data));
    let line = [b"ED".as_slice(), &keynum, signature.as_ref()].concat();
    let global = pair.sign(&[signature.as_ref(), trusted_comment.as_bytes()].concat());
    let text = format!(
        "untrusted comment: signature from a test key\n{}\ntrusted comment: {trusted_comment}\n{}\n",
        Base64::encode_string(&line),
        Base64::encode_string(global.as_ref()),
    );
    (Base64::encode_string(&public), text)
}

/// A release archive holding `program` as `sideporch`.
fn archive(program: &[u8]) -> Vec<u8> {
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    {
        let mut tar = tar::Builder::new(&mut gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(u64::try_from(program.len()).unwrap());
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "sideporch", program).unwrap();
        tar.finish().unwrap();
    }
    gz.finish().unwrap()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut hex, byte| {
        use std::fmt::Write as _;
        write!(hex, "{byte:02x}").unwrap();
        hex
    })
}

const NEW_PROGRAM: &[u8] = b"#!/bin/sh\necho sideporch 0.99.0\n";

/// A fake GitHub with a security release 0.99.0 of `program`. With
/// `tamper`, the checksums served aren't the ones that were signed.
async fn fake_github(program: &'static [u8], tamper: bool) -> (String, String) {
    let archive = archive(program);
    let name = archive_name("0.99.0", TARGET);
    let sums = format!(
        "{}  {name}\n",
        hex(&<Sha256 as sha2::Digest>::digest(&archive))
    );
    let (key, signature) = sign(9, sums.as_bytes(), "sideporch 0.99.0");
    let served_sums = if tamper { sums.replace('a', "b") } else { sums };
    let releases = json!([
        {
            "tag_name": "v0.99.0",
            "published_at": "2026-10-01T09:00:00Z",
            "body": "## Sideporch 0.99.0\n\n### Security\n\n- Keep tokens out of logs\n",
            "html_url": "https://github.com/niklas-heer/sideporch/releases/tag/v0.99.0"
        },
        {
            "tag_name": "v0.1.0",
            "published_at": "2026-09-01T09:00:00Z",
            "body": "",
            "html_url": "https://github.com/niklas-heer/sideporch/releases/tag/v0.1.0"
        }
    ]);
    let app = Router::new()
        .route(
            "/repos/sideporch/releases",
            get(move || async move { Json(releases) }),
        )
        .route(
            "/download/v0.99.0/SHA256SUMS",
            get(move || async move { served_sums }),
        )
        .route(
            "/download/v0.99.0/SHA256SUMS.minisig",
            get(move || async move { signature }),
        )
        .route(
            &format!("/download/v0.99.0/{name}"),
            get(move || async move { archive }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (base, key)
}

/// A server whose program is `program` in its own directory, updating from
/// a fake GitHub.
async fn server_with_fake_github(tamper: bool) -> (Server, tempfile::TempDir) {
    let (github, key) = fake_github(NEW_PROGRAM, tamper).await;
    let install_dir = tempfile::tempdir().unwrap();
    let program = install_dir.path().join("sideporch");
    std::fs::write(&program, b"the old program").unwrap();
    let server = start_with(|config| {
        config.update_check = true;
        config.update_source = Some((
            Source {
                api: format!("{github}/repos/sideporch"),
                downloads: format!("{github}/download"),
                public_key: key,
                local: true,
            },
            program,
        ));
    })
    .await;
    (server, install_dir)
}

#[tokio::test]
async fn admins_hear_about_security_releases_and_install_them() {
    let (server, install_dir) = server_with_fake_github(false).await;
    let ada = admin(&server).await;
    let bea = invite(&server, &ada, "Bea", "bea").await;

    // Before a check, nothing is known.
    let page = ada.page("/admin/updates").await;
    assert!(page.contains("hasn't checked yet"), "{page}");

    assert_eq!(
        ada.post("/admin/updates/check", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    let page = ada.page("/admin/updates").await;
    assert!(page.contains("0.99.0"), "{page}");
    assert!(page.contains("Security"), "{page}");
    assert!(page.contains("Update now"), "{page}");

    // A security release reminds admins on every page; members see nothing.
    assert!(ada.page("/people").await.contains("data-update-notice"));
    assert!(!bea.page("/people").await.contains("data-update-notice"));
    assert_eq!(
        bea.get("/admin/updates").await.status(),
        StatusCode::FORBIDDEN
    );

    // Hidden for a day.
    assert_eq!(
        ada.post("/updates/hide", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    assert!(!ada.page("/people").await.contains("data-update-notice"));

    assert_eq!(
        ada.post("/admin/updates/install", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    let program = install_dir.path().join("sideporch");
    assert_eq!(std::fs::read(&program).unwrap(), NEW_PROGRAM);
    assert_eq!(
        std::fs::read(install_dir.path().join("sideporch.previous")).unwrap(),
        b"the old program"
    );
    let page = ada.page("/admin/updates").await;
    assert!(page.contains("Restart Sideporch to use it"), "{page}");
    // Nothing is left behind next to the program.
    let mut left: Vec<String> = std::fs::read_dir(install_dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["sideporch", "sideporch.previous"]);
}

#[tokio::test]
async fn nothing_is_installed_unless_the_signature_matches() {
    let (server, install_dir) = server_with_fake_github(true).await;
    let ada = admin(&server).await;
    ada.post("/admin/updates/check", &[]).await;
    ada.post("/admin/updates/install", &[]).await;
    let program = install_dir.path().join("sideporch");
    assert_eq!(std::fs::read(&program).unwrap(), b"the old program");
    let page = ada.page("/admin/updates").await;
    assert!(
        page.contains("aren&#x27;t signed with Sideporch")
            || page.contains("aren't signed with Sideporch"),
        "{page}"
    );
    assert_eq!(std::fs::read_dir(install_dir.path()).unwrap().count(), 1);
}

#[tokio::test]
async fn admins_choose_whether_to_check_and_what_installs_by_itself() {
    let (server, _install_dir) = server_with_fake_github(false).await;
    let ada = admin(&server).await;
    let response = ada
        .post("/admin/updates/settings", &[("install", "all")])
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let page = ada.page("/admin/updates").await;
    assert!(page.contains(r#"value="all" checked"#), "{page}");
    assert!(
        !page.contains(r#"name="check" value="on" checked"#),
        "checking is off: {page}"
    );
    let bea = invite(&server, &ada, "Bea", "bea").await;
    assert_eq!(
        bea.post(
            "/admin/updates/settings",
            &[("check", "on"), ("install", "off")]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
}
