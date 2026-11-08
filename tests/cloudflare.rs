// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation
)]

//! Signing in through Cloudflare Access, against a fake Cloudflare that
//! serves the team's keys, with assertions signed by a test key.

mod common;

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Json, Router, routing::get};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use common::{Browser, Server, between, invite, location, start_with};
use reqwest::StatusCode;
use ring::{
    rand::SystemRandom,
    signature::{RSA_PKCS1_SHA256, RsaKeyPair},
};
use serde_json::{Value, json};

const HEADER: &str = "cf-access-jwt-assertion";
const TEAM: &str = "https://team.cloudflareaccess.com";
const AUD: &str = "aud-tag";

/// Serves the test key the way Cloudflare publishes a team's keys, and
/// counts how often it was asked.
async fn fake_cloudflare() -> (String, Arc<AtomicUsize>) {
    let jwk: Value = serde_json::from_str(include_str!("fixtures/cloudflare/jwk.json")).unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&asked);
    let app = Router::new().route(
        "/cdn-cgi/access/certs",
        get(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            let keys =
                json!({ "keys": [jwk.clone()], "public_cert": { "kid": "test-key", "cert": "" } });
            async move { Json(keys) }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/cdn-cgi/access/certs",
        listener.local_addr().unwrap()
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, asked)
}

/// A server behind (fake) Cloudflare Access.
async fn server(required: bool) -> (Server, Arc<AtomicUsize>) {
    let (certs, asked) = fake_cloudflare().await;
    let server = start_with(|config| {
        config.cloudflare_access_team = Some("team".to_owned());
        config.cloudflare_access_aud = Some(AUD.to_owned());
        config.cloudflare_access_required = required;
        config.cloudflare_access_certs_url = Some(certs);
    })
    .await;
    (server, asked)
}

fn part(value: &Value) -> String {
    Base64UrlUnpadded::encode_string(value.to_string().as_bytes())
}

fn now() -> i64 {
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
    )
    .unwrap()
}

/// Signs `claims` with the test key, under `header`.
fn sign(header: &Value, claims: &Value) -> String {
    let key = RsaKeyPair::from_pkcs8(include_bytes!("fixtures/cloudflare/key.pk8")).unwrap();
    let signed = format!("{}.{}", part(header), part(claims));
    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signed.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{signed}.{}", Base64UrlUnpadded::encode_string(&signature))
}

fn rs256() -> Value {
    json!({ "alg": "RS256", "kid": "test-key", "typ": "JWT" })
}

/// The claims Cloudflare Access sends for a person.
fn claims(email: &str) -> Value {
    let now = now();
    json!({
        "aud": [AUD],
        "email": email,
        "exp": now + 300,
        "iat": now,
        "nbf": now,
        "iss": TEAM,
        "type": "app",
        "identity_nonce": "nonce",
        "sub": "7335d417-61da-459d-899c-0a01c76a2f94",
        "country": "DE",
    })
}

fn assertion(email: &str) -> String {
    sign(&rs256(), &claims(email))
}

/// A browser that comes through Cloudflare Access as `assertion`.
fn through_access(server: &Server, assertion: &str) -> Browser {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(HEADER, assertion.parse().unwrap());
    let mut browser = Browser::anonymous(server);
    browser.client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .default_headers(headers)
        .build()
        .unwrap();
    browser
}

fn session_cookie(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|value| {
            value
                .to_str()
                .unwrap()
                .split(';')
                .next()
                .unwrap()
                .to_owned()
        })
        .find(|cookie| cookie.starts_with("sideporch_session=") && cookie.len() > 18)
}

/// Sets up the server through Access as `email`, and returns that browser.
async fn set_up(server: &Server, email: &str) -> Browser {
    let mut ada = through_access(server, &assertion(email));
    let response = ada
        .submit(
            "/setup",
            &[
                ("display_name", "Ada Admin"),
                ("username", "ada"),
                ("password", "correct horse"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    ada
}

#[tokio::test]
async fn setup_needs_access_and_access_signs_the_admin_back_in() {
    let (server, _) = server(false).await;
    // Without Access, or with a service token, nobody can claim the server.
    let stranger = Browser::anonymous(&server);
    assert_eq!(stranger.get("/setup").await.status(), StatusCode::FORBIDDEN);
    let response = stranger
        .post(
            "/setup",
            &[
                ("display_name", "Mallory"),
                ("username", "mallory"),
                ("password", "correct horse"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let mut service_claims = claims("");
    service_claims["common_name"] = "robot.access".into();
    let robot = through_access(&server, &sign(&rs256(), &service_claims));
    assert_eq!(robot.get("/setup").await.status(), StatusCode::FORBIDDEN);

    // Through Access, the page says Access is what claims it.
    let ada = through_access(&server, &assertion("ada@example.com"));
    let page = ada.page("/setup").await;
    assert!(page.contains("Cloudflare Access let you in"), "{page}");
    let ada = set_up(&server, "Ada@Example.com").await;
    let security = ada.page("/settings/security").await;
    assert!(security.contains("ada@example.com"), "stores the address");

    // Signing out also signs out of Access, which would sign them back in.
    let response = ada.post("/logout", &[]).await;
    assert_eq!(location(&response), "/cdn-cgi/access/logout");

    // Opening the site through Access signs in without a password.
    let mut fresh = through_access(&server, &assertion("ada@example.com"));
    let response = fresh.get("/").await;
    assert_eq!(location(&response), "/login");
    let response = fresh.get("/login?next=/people").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/people");
    fresh.cookie = session_cookie(&response).expect("signed in");
    assert!(fresh.page("/home").await.contains("Ada Admin"));
}

#[tokio::test]
async fn invalid_assertions_sign_nobody_in() {
    let (server, _) = server(false).await;
    set_up(&server, "ada@example.com").await;
    let changed = |key: &str, value: Value| {
        let mut claims = claims("ada@example.com");
        claims[key] = value;
        sign(&rs256(), &claims)
    };
    let valid = assertion("ada@example.com");
    let (signed, _) = valid.rsplit_once('.').unwrap();
    let (_, signature) = valid.rsplit_once('.').unwrap();
    let mut tampered_claims = claims("ada@example.com");
    tampered_claims["email"] = "eve@example.com".into();
    let refused = [
        ("wrong audience", changed("aud", json!(["other-app"]))),
        (
            "another team",
            changed("iss", "https://evil.cloudflareaccess.com".into()),
        ),
        ("expired", changed("exp", (now() - 120).into())),
        ("not valid yet", changed("nbf", (now() + 600).into())),
        ("not an app token", changed("type", "org".into())),
        (
            "alg none",
            format!(
                "{}.{}.",
                part(&json!({ "alg": "none", "kid": "test-key" })),
                part(&claims("ada@example.com"))
            ),
        ),
        (
            "HS256",
            format!(
                "{}.{}.{signature}",
                part(&json!({ "alg": "HS256", "kid": "test-key" })),
                part(&claims("ada@example.com"))
            ),
        ),
        (
            "unknown key",
            sign(
                &json!({ "alg": "RS256", "kid": "other-key" }),
                &claims("ada@example.com"),
            ),
        ),
        (
            "tampered",
            format!(
                "{}.{}.{signature}",
                signed.split('.').next().unwrap(),
                part(&tampered_claims)
            ),
        ),
        ("garbage", "not-a-jwt".to_owned()),
    ];
    for (what, token) in refused {
        let browser = through_access(&server, &token);
        let response = browser.get("/login").await;
        assert_eq!(response.status(), StatusCode::OK, "{what}");
        assert_eq!(session_cookie(&response), None, "{what}");
        let page = response.text().await.unwrap();
        assert!(
            page.contains("couldn&#39;t be verified") || page.contains("couldn't be verified"),
            "{what}: {page}"
        );
    }
    // A service token is a program: it signs nobody in, quietly.
    let mut service_claims = claims("");
    service_claims["common_name"] = "robot.access".into();
    let robot = through_access(&server, &sign(&rs256(), &service_claims));
    let response = robot.get("/login").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(session_cookie(&response), None);
    // The valid one still works.
    let response = through_access(&server, &valid).get("/login").await;
    assert!(session_cookie(&response).is_some());
}

#[tokio::test]
async fn unknown_keys_ask_cloudflare_at_most_once_a_minute() {
    let (server, asked) = server(false).await;
    set_up(&server, "ada@example.com").await;
    let before = asked.load(Ordering::SeqCst);
    assert!(before >= 1, "fetched the keys");
    for _ in 0..3 {
        let token = sign(
            &json!({ "alg": "RS256", "kid": "new-key" }),
            &claims("ada@example.com"),
        );
        let response = through_access(&server, &token).get("/login").await;
        assert_eq!(session_cookie(&response), None);
    }
    assert_eq!(asked.load(Ordering::SeqCst), before);
}

#[tokio::test]
async fn only_active_local_accounts_with_the_address_are_signed_in() {
    let (server, _) = server(false).await;
    let ada = set_up(&server, "ada@example.com").await;
    let mut bob = invite(&server, &ada, "Bob", "bob").await;

    // An address nobody uses explains itself, and signing in with a
    // password then offers to link it.
    let visitor = through_access(&server, &assertion("bob@example.com"));
    let response = visitor.get("/login").await;
    assert_eq!(session_cookie(&response), None);
    let page = response.text().await.unwrap();
    assert!(page.contains("no account here uses that address"), "{page}");
    assert!(
        page.contains(r#"value="/settings/cloudflare-access""#),
        "{page}"
    );

    let bob_access = through_access(&server, &assertion("bob@example.com"));
    let link = |path: &str| {
        bob_access
            .client
            .get(bob_access.url(path))
            .header("cookie", &bob.cookie)
    };
    let page = link("/settings/cloudflare-access")
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(page.contains("Use bob@example.com"), "{page}");
    let response = bob_access
        .client
        .post(bob_access.url("/settings/cloudflare-access"))
        .header("cookie", &bob.cookie)
        .form(&[("email", "bob@example.com")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    // Another account's address can't be taken over.
    let ada_address = through_access(&server, &assertion("ada@example.com"));
    let response = ada_address
        .client
        .post(ada_address.url("/settings/cloudflare-access"))
        .header("cookie", &bob.cookie)
        .form(&[("email", "ada@example.com")])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);

    let response = bob_access.get("/login").await;
    bob.cookie = session_cookie(&response).expect("bob signs in through Access");
    let bob_id = bob.user_id().await;

    // Deactivated accounts don't sign in.
    let response = ada.post(&format!("/people/{bob_id}/deactivate"), &[]).await;
    assert!(response.status().is_redirection(), "{}", response.status());
    let response = bob_access.get("/login").await;
    assert_eq!(session_cookie(&response), None);
    let response = ada.post(&format!("/people/{bob_id}/reactivate"), &[]).await;
    assert!(response.status().is_redirection());
    assert!(session_cookie(&bob_access.get("/login").await).is_some());

    // Nor do accounts of people from other servers.
    let conn = rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap();
    conn.execute(
        "INSERT INTO instances (url, handle, status, created_at, updated_at)
         VALUES ('https://elsewhere.example', 'elsewhere.example', 'connected', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE users SET instance_id = last_insert_rowid(), remote_id = 7 WHERE id = ?1",
        [bob_id],
    )
    .unwrap();
    assert_eq!(session_cookie(&bob_access.get("/login").await), None);
}

/// The code an authenticator app shows for a secret at a 30-second step.
fn code(secret: &[u8], step: u64) -> String {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, secret);
    let tag = ring::hmac::sign(&key, &step.to_be_bytes());
    let digest = tag.as_ref();
    let offset = (digest[19] & 0x0f) as usize;
    let number = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    format!("{:06}", number % 1_000_000)
}

fn from_base32(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let (mut buffer, mut bits) = (0_u32, 0_u32);
    for c in text.chars().filter(|c| !c.is_whitespace()) {
        let value = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            _ => c as u32 - '2' as u32 + 26,
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xff) as u8);
        }
    }
    out
}

#[tokio::test]
async fn access_is_only_the_first_step() {
    let (server, _) = server(false).await;
    let ada = set_up(&server, "ada@example.com").await;
    let page = ada.page("/settings/security/totp").await;
    let key: String = between(&page, "select-all", "</code>")
        .split('>')
        .nth(1)
        .unwrap()
        .to_owned();
    let sealed = between(&page, "name=\"secret\" value=\"", "\"").to_owned();
    let step = u64::try_from(now()).unwrap() / 30;
    let response = ada
        .post(
            "/settings/security/totp",
            &[
                ("secret", &sealed),
                ("code", &code(&from_base32(&key), step)),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = through_access(&server, &assertion("ada@example.com"))
        .get("/login")
        .await;
    assert_eq!(location(&response), "/login/verify");
    assert_eq!(session_cookie(&response), None, "asks for the code first");
}

#[tokio::test]
async fn access_does_not_get_around_requiring_passkeys() {
    let (server, _) = server(false).await;
    let ada = set_up(&server, "ada@example.com").await;
    // Admins must meet the policy before choosing it; a stored passkey does.
    let database = rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap();
    database
        .execute(
            "INSERT INTO passkeys (user_id, credential_id, public_key, algorithm, name, created_at)
             SELECT id, x'01', x'02', -7, 'Laptop', 0 FROM users WHERE username = 'ada'",
            [],
        )
        .unwrap();
    let saved = ada.post("/admin/sign-in", &[("require", "passkeys")]).await;
    assert_eq!(saved.status(), StatusCode::OK);
    let response = through_access(&server, &assertion("ada@example.com"))
        .get("/login")
        .await;
    assert_eq!(session_cookie(&response), None, "a passkey is required");
}

#[tokio::test]
async fn access_waits_while_the_account_is_recovered_from_the_server() {
    let (server, _) = server(false).await;
    set_up(&server, "ada@example.com").await;
    // A lost laptop may still have its Access session.
    let lost = through_access(&server, &assertion("ada@example.com"));
    let link = sideporch::reset_link(server.data_dir(), "ada", true).unwrap();
    let response = lost.get("/login").await;
    assert_eq!(session_cookie(&response), None);
    assert!(response.text().await.unwrap().contains("being recovered"));

    // Once the link is used, Access signs in again.
    let mut ada = Browser::anonymous(&server);
    let used = ada
        .submit(&link.path, &[("password", "a brand new one")])
        .await;
    assert_eq!(location(&used), "/");
    let response = through_access(&server, &assertion("ada@example.com"))
        .get("/login")
        .await;
    assert!(session_cookie(&response).is_some());
}

#[tokio::test]
async fn signing_out_without_access_stays_on_this_server() {
    let (server, _) = server(false).await;
    set_up(&server, "ada@example.com").await;
    let mut direct = Browser::anonymous(&server);
    let response = direct
        .submit(
            "/login",
            &[("username", "ada"), ("password", "correct horse")],
        )
        .await;
    assert_eq!(location(&response), "/");
    assert_eq!(location(&direct.post("/logout", &[]).await), "/login");
}

#[tokio::test]
async fn required_access_closes_pages_but_not_webhooks_or_health_checks() {
    let (server, _) = server(true).await;
    set_up(&server, "ada@example.com").await;
    let direct = Browser::anonymous(&server);
    for path in ["/login", "/", "/setup", "/people"] {
        assert_eq!(
            direct.get(path).await.status(),
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }
    let forged = through_access(
        &server,
        &sign(
            &json!({ "alg": "RS256", "kid": "test-key" }),
            &json!({ "iss": TEAM, "aud": "other", "exp": now() + 60, "email": "ada@example.com" }),
        ),
    );
    assert_eq!(forged.get("/login").await.status(), StatusCode::FORBIDDEN);
    for path in [
        "/healthz",
        "/assets/app.css",
        "/manifest.webmanifest",
        "/sw.js",
    ] {
        assert_eq!(direct.get(path).await.status(), StatusCode::OK, "{path}");
    }
    // Webhooks and MCP answer with their own errors, not Access's.
    let hook = direct.post("/hooks/not-a-token", &[("text", "hi")]).await;
    assert_ne!(hook.status(), StatusCode::FORBIDDEN);
    let mcp = direct.post_json("/mcp", &json!({})).await;
    assert_eq!(mcp.status(), StatusCode::UNAUTHORIZED);
    // Through Access, pages open, for people and service tokens alike.
    let ada = through_access(&server, &assertion("ada@example.com"));
    assert_eq!(ada.get("/login").await.status(), StatusCode::SEE_OTHER);
    let mut service_claims = claims("");
    service_claims["common_name"] = "robot.access".into();
    let robot = through_access(&server, &sign(&rs256(), &service_claims));
    assert_eq!(robot.get("/login").await.status(), StatusCode::OK);
}

#[tokio::test]
async fn access_needs_both_its_settings() {
    for (team, aud, required) in [
        (Some("team"), None, false),
        (None, Some(AUD), false),
        (None, None, true),
        (Some("not a domain"), Some(AUD), false),
    ] {
        let data = tempfile::tempdir().unwrap();
        let opened = sideporch::Sideporch::open(sideporch::Config {
            cloudflare_access_team: team.map(ToOwned::to_owned),
            cloudflare_access_aud: aud.map(ToOwned::to_owned),
            cloudflare_access_required: required,
            ..common::config(data.path())
        })
        .await;
        assert!(opened.is_err(), "{team:?} {aud:?} {required}");
    }
}
