//! The automation store: importing from a web address, the catalogue
//! admins browse, what it checks before installing, updates, and settings
//! automations declare. A fake catalogue host serves the files.

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

use axum::{Router, routing::get};
use base64ct::{Base64, Encoding as _};
use blake2::Digest as _;
use reqwest::StatusCode;
use ring::signature::{Ed25519KeyPair, KeyPair as _};
use serde_json::json;

use common::{Browser, admin, invite, start, start_with};

/// Signs like `minisign -S` (prehashed with BLAKE2b-512). Returns the public
/// key and the `.minisig` text.
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

/// The SHA-256 of `text`, in hex, as a store lists it.
fn sha256(text: &str) -> String {
    <sha2::Sha256 as sha2::Digest>::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Serves `body` at `/<path>` on a local port; returns the file's URL.
async fn serve(path: &'static str, body: String) -> String {
    let app = Router::new().route(
        &format!("/{path}"),
        get(move || {
            let body = body.clone();
            async move { body }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}/{path}")
}

/// One automation, as a sharing file.
const WEATHER_FILE: &str = r#"{"format":"sideporch-automations","version":1,
  "items":[{"kind":"automation","name":"Weather","source":"sideporch.on_message(function(msg)\n  print(msg.text)\nend)"}]}"#;

/// Lets this server's automations reach private addresses, as an admin
/// does for a home lab. Tests serve files on this machine.
async fn allow_private_network(admin: &Browser) {
    let response = admin
        .post(
            "/settings/automations",
            &[("timezone", "UTC"), ("allow_private_network", "on")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn importing_from_a_web_address_previews_and_stays_off() {
    let server = start().await;
    let admin = admin(&server).await;
    allow_private_network(&admin).await;
    let url = serve("weather.sideporch.json", WEATHER_FILE.to_owned()).await;

    let preview = admin
        .post("/automations/import/url", &[("url", &url)])
        .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let page = preview.text().await.unwrap();
    assert!(page.contains("Downloaded from"), "{page}");
    assert!(page.contains("Weather"), "{page}");
    // Nothing is added until the preview is confirmed.
    let list = admin.page("/automations").await;
    assert!(!list.contains(">Weather<"), "{list}");

    assert!(page.contains("Not signed"), "{page}");
    let bundle = WEATHER_FILE.replace('\n', "");
    // Unsigned, so it is installed only once the admin confirms it.
    let unconfirmed = admin
        .post(
            "/automations/import/confirm",
            &[
                ("bundle", &bundle),
                ("action_0", "import"),
                ("trust", "unsigned"),
            ],
        )
        .await;
    assert_eq!(unconfirmed.status(), StatusCode::BAD_REQUEST);
    let confirm = admin
        .post(
            "/automations/import/confirm",
            &[
                ("bundle", &bundle),
                ("action_0", "import"),
                ("trust", "unsigned"),
                ("confirm_unsigned", "on"),
            ],
        )
        .await;
    assert!(confirm.status().is_redirection(), "{}", confirm.status());
    let list = admin.page("/automations").await;
    let weather = list.split(">Weather<").nth(1).unwrap_or_default();
    assert!(
        weather.contains(">Off<"),
        "imported scripts start switched off"
    );
}

#[tokio::test]
async fn the_guard_refuses_internal_addresses_unless_allowed() {
    let server = start().await;
    let admin = admin(&server).await;
    let url = serve("weather.sideporch.json", WEATHER_FILE.to_owned()).await;

    let refused = admin
        .post("/automations/import/url", &[("url", &url)])
        .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let page = refused.text().await.unwrap();
    assert!(page.contains("internal address"), "{page}");
    assert!(!page.contains("Downloaded from"), "{page}");

    let other = admin
        .post(
            "/automations/import/url",
            &[("url", "ftp://example.com/weather.json")],
        )
        .await;
    assert_eq!(other.status(), StatusCode::BAD_REQUEST);
    assert!(other.text().await.unwrap().contains("https://"));
}

#[tokio::test]
async fn only_admins_import_from_a_web_address() {
    let server = start().await;
    let admin = admin(&server).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let response = member
        .post(
            "/automations/import/url",
            &[("url", "https://example.com/a.json")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

/// The example automation the website publishes.
const WELCOME: &str =
    include_str!("../website/static/automations/welcome-new-members.sideporch.json");

/// Points the store at `catalogue` and lets this server reach this machine,
/// as an admin does to try a store on a home network.
async fn use_store(admin: &Browser, catalogue: &str) {
    let response = admin
        .post(
            "/settings/automations",
            &[
                ("timezone", "UTC"),
                ("allow_private_network", "on"),
                ("catalogue_url", catalogue),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

/// Serves a catalogue listing `file` from a fake host, with `checksum` and
/// the optional `signature` address in its entry. Returns the catalogue's
/// address and the file's.
async fn fake_store_entry(
    file: &str,
    checksum: &str,
    signature: Option<String>,
) -> (String, String) {
    let url = serve("welcome-new-members.sideporch.json", file.to_owned()).await;
    let index = json!({
        "format": "sideporch-catalogue",
        "version": 1,
        "entries": [{
            "name": "Welcome new members",
            "description": "Posts a welcome.",
            "author": "Ann",
            "license": "MIT",
            "version": "1.0.0",
            "file": url,
            "sha256": checksum,
            "signature": signature,
        }],
    });
    (serve("index.json", index.to_string()).await, url)
}

/// A catalogue entry for `file`, with its real checksum and no signature.
async fn fake_store(file: &str) -> (String, String) {
    fake_store_entry(file, &sha256(file), None).await
}

#[tokio::test]
async fn the_store_lists_what_the_catalogue_holds_and_previews_an_install() {
    let server = start().await;
    let admin = admin(&server).await;
    let (catalogue, file) = fake_store(WELCOME).await;
    use_store(&admin, &catalogue).await;

    let store = admin.page("/automations/browse").await;
    assert!(store.contains("Welcome new members"), "{store}");
    assert!(store.contains("Preview install"), "{store}");
    assert!(store.contains("Sideporch doesn't review"), "{store}");

    let preview = admin
        .post("/automations/browse/install", &[("file", &file)])
        .await;
    assert_eq!(preview.status(), StatusCode::OK);
    let page = preview.text().await.unwrap();
    assert!(page.contains("Downloaded from"), "{page}");
    assert!(page.contains("Welcome new members"), "{page}");

    // The store only installs what it lists.
    let stranger = admin
        .post(
            "/automations/browse/install",
            &[("file", "http://127.0.0.1:9/elsewhere.json")],
        )
        .await;
    let page = stranger.text().await.unwrap();
    assert!(page.contains("isn't in the store"), "{page}");
}

#[tokio::test]
async fn an_unreachable_store_says_so() {
    let server = start().await;
    let admin = admin(&server).await;
    use_store(&admin, "http://127.0.0.1:9/index.json").await;
    let store = admin.page("/automations/browse").await;
    assert!(store.contains("can't be reached"), "{store}");
    assert!(store.contains("Automation settings"), "{store}");
}

#[tokio::test]
async fn the_store_address_must_be_a_web_address() {
    let server = start().await;
    let admin = admin(&server).await;
    let response = admin
        .post(
            "/settings/automations",
            &[
                ("timezone", "UTC"),
                ("catalogue_url", "ftp://example.com/i.json"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

/// Posts an install of `file` from the store, returning the response.
async fn install(admin: &Browser, file: &str) -> reqwest::Response {
    admin
        .post("/automations/browse/install", &[("file", file)])
        .await
}

/// The signature's address on a fake host, for an entry to name.
async fn serve_signature(signature: String) -> String {
    serve("welcome-new-members.sideporch.json.minisig", signature).await
}

#[tokio::test]
async fn a_signed_entry_installs_once_its_checksum_and_signature_check_out() {
    let (key, signature) = sign(
        9,
        WELCOME.as_bytes(),
        &format!("sideporch-automation {}", sha256(WELCOME)),
    );
    let server = start_with(|config| config.automation_public_key = Some(key)).await;
    let admin = admin(&server).await;
    let signature = serve_signature(signature).await;
    let (catalogue, file) = fake_store_entry(WELCOME, &sha256(WELCOME), Some(signature)).await;
    use_store(&admin, &catalogue).await;

    let preview = install(&admin, &file).await;
    assert_eq!(preview.status(), StatusCode::OK);
    let page = preview.text().await.unwrap();
    assert!(page.contains("Signed by Sideporch"), "{page}");
    assert!(!page.contains("name=\"confirm_unsigned\""), "{page}");

    let confirm = admin
        .post(
            "/automations/import/confirm",
            &[
                ("bundle", WELCOME),
                ("action_0", "import"),
                ("trust", "signed"),
            ],
        )
        .await;
    assert!(confirm.status().is_redirection(), "{}", confirm.status());
    let list = admin.page("/automations").await;
    let welcome = list
        .split(">Welcome new members<")
        .nth(1)
        .unwrap_or_default();
    assert!(welcome.contains(">Off<"), "{list}");
}

#[tokio::test]
async fn a_file_that_does_not_match_its_checksum_or_signature_is_refused() {
    let (key, _) = sign(9, b"", "unused");
    let server = start_with(|config| config.automation_public_key = Some(key)).await;
    let admin = admin(&server).await;

    // The store's checksum is for other bytes.
    let (catalogue, file) = fake_store_entry(WELCOME, &"0".repeat(64), None).await;
    use_store(&admin, &catalogue).await;
    let refused = install(&admin, &file).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let page = refused.text().await.unwrap();
    assert!(page.contains("doesn't match the checksum"), "{page}");
    assert!(!page.contains("Downloaded from"), "{page}");

    // A signature over other bytes.
    let (_, other) = sign(
        9,
        b"something else",
        &format!("sideporch-automation {}", sha256(WELCOME)),
    );
    let signature = serve_signature(other).await;
    let (catalogue, file) = fake_store_entry(WELCOME, &sha256(WELCOME), Some(signature)).await;
    use_store(&admin, &catalogue).await;
    let page = install(&admin, &file).await.text().await.unwrap();
    assert!(page.contains("doesn't check out"), "{page}");

    // A release's signature, which names a release rather than a file.
    let (_, release) = sign(9, WELCOME.as_bytes(), "sideporch 1.0.0");
    let signature = serve_signature(release).await;
    let (catalogue, file) = fake_store_entry(WELCOME, &sha256(WELCOME), Some(signature)).await;
    use_store(&admin, &catalogue).await;
    let refused = install(&admin, &file).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let page = refused.text().await.unwrap();
    assert!(page.contains("another file"), "{page}");
}

#[tokio::test]
async fn an_unsigned_entry_installs_only_when_confirmed() {
    let server = start().await;
    let admin = admin(&server).await;
    let (catalogue, file) = fake_store(WELCOME).await;
    use_store(&admin, &catalogue).await;

    let page = install(&admin, &file).await.text().await.unwrap();
    assert!(page.contains("Not signed"), "{page}");
    assert!(page.contains("name=\"confirm_unsigned\""), "{page}");
    let unconfirmed = admin
        .post(
            "/automations/import/confirm",
            &[
                ("bundle", WELCOME),
                ("action_0", "import"),
                ("trust", "unsigned"),
            ],
        )
        .await;
    assert_eq!(unconfirmed.status(), StatusCode::BAD_REQUEST);
    let list = admin.page("/automations").await;
    assert!(!list.contains(">Welcome new members<"), "nothing was added");
}
