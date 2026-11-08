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
use reqwest::StatusCode;

use common::{Browser, admin, invite, start};

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

    let bundle = WEATHER_FILE.replace('\n', "");
    let confirm = admin
        .post(
            "/automations/import/confirm",
            &[("bundle", &bundle), ("action_0", "import")],
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

/// Serves a catalogue listing `file` from a fake host. Returns the
/// catalogue's address and the file's.
async fn fake_store(file: &str) -> (String, String) {
    let digest = <sha2::Sha256 as sha2::Digest>::digest(file.as_bytes());
    let sha256: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let url = serve("welcome-new-members.sideporch.json", file.to_owned()).await;
    let index = serde_json::json!({
        "format": "sideporch-catalogue",
        "version": 1,
        "entries": [{
            "name": "Welcome new members",
            "description": "Posts a welcome.",
            "author": "Ann",
            "license": "MIT",
            "version": "1.0.0",
            "file": url,
            "sha256": sha256,
        }],
    });
    (serve("index.json", index.to_string()).await, url)
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
