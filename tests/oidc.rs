// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects,
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::too_many_lines
)]

//! Signing in with an `OpenID Connect` provider, against a fake provider on a
//! random port that serves discovery and keys, checks PKCE, and signs ID
//! tokens with test keys.

mod common;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64ct::{Base64, Base64UrlUnpadded, Encoding as _};
use common::{Browser, Server, admin, between, invite, location, start};
use reqwest::StatusCode as HttpStatus;
use ring::{
    rand::SystemRandom,
    signature::{
        ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair as _, RSA_PKCS1_SHA256, RsaKeyPair,
    },
};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

const CLIENT_ID: &str = "sideporch-test";
const CLIENT_SECRET: &str = "s3cret-value-xyz";
const EC_KID: &str = "ec-key";

/// Signs tokens the way a provider's key does.
struct Keys {
    rsa: RsaKeyPair,
    ec: EcdsaKeyPair,
    /// The RSA modulus, for the HS256 confusion attempt.
    modulus: Vec<u8>,
}

impl Keys {
    fn new() -> Self {
        let rsa = RsaKeyPair::from_pkcs8(include_bytes!("fixtures/cloudflare/key.pk8")).unwrap();
        let jwk: Value =
            serde_json::from_str(include_str!("fixtures/cloudflare/jwk.json")).unwrap();
        let modulus = Base64UrlUnpadded::decode_vec(jwk["n"].as_str().unwrap()).unwrap();
        let pkcs8 =
            EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
                .unwrap();
        let ec = EcdsaKeyPair::from_pkcs8(
            &ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
            &SystemRandom::new(),
        )
        .unwrap();
        Self { rsa, ec, modulus }
    }

    /// The key set a provider publishes: the RSA key and the P-256 key.
    fn jwks(&self) -> Value {
        let rsa: Value =
            serde_json::from_str(include_str!("fixtures/cloudflare/jwk.json")).unwrap();
        let point = self.ec.public_key().as_ref();
        let (x, y) = point[1..].split_at(32);
        json!({ "keys": [
            rsa,
            {
                "kty": "EC", "crv": "P-256", "kid": EC_KID, "use": "sig", "alg": "ES256",
                "x": Base64UrlUnpadded::encode_string(x),
                "y": Base64UrlUnpadded::encode_string(y),
            }
        ]})
    }
}

fn part(value: &Value) -> String {
    Base64UrlUnpadded::encode_string(value.to_string().as_bytes())
}

/// Signs `header.claims` with the algorithm the header names, as a
/// provider would (or, for the attacks, as an attacker would).
fn sign(keys: &Keys, header: &Value, claims: &Value) -> String {
    let signed = format!("{}.{}", part(header), part(claims));
    let signature = match header["alg"].as_str().unwrap_or_default() {
        "RS256" => {
            let mut signature = vec![0; keys.rsa.public().modulus_len()];
            keys.rsa
                .sign(
                    &RSA_PKCS1_SHA256,
                    &SystemRandom::new(),
                    signed.as_bytes(),
                    &mut signature,
                )
                .unwrap();
            signature
        }
        "ES256" => keys
            .ec
            .sign(&SystemRandom::new(), signed.as_bytes())
            .unwrap()
            .as_ref()
            .to_vec(),
        "HS256" => {
            let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, &keys.modulus);
            ring::hmac::sign(&key, signed.as_bytes()).as_ref().to_vec()
        }
        _ => Vec::new(),
    };
    format!("{signed}.{}", Base64UrlUnpadded::encode_string(&signature))
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

/// What the fake provider remembers about one authorization.
#[derive(Clone)]
struct Grant {
    nonce: String,
    challenge: String,
    redirect_uri: String,
}

/// How the next ID token is made: the header's algorithm and key id, and
/// claims changed from the honest ones (`Null` removes a claim).
#[derive(Clone)]
struct Mint {
    alg: String,
    kid: String,
    claims: Map<String, Value>,
    /// Signs with nothing at all, for `alg: none`.
    unsigned: bool,
}

impl Default for Mint {
    fn default() -> Self {
        Self {
            alg: "RS256".to_owned(),
            kid: "test-key".to_owned(),
            claims: Map::new(),
            unsigned: false,
        }
    }
}

struct FakeState {
    issuer: String,
    keys: Keys,
    grants: Mutex<HashMap<String, Grant>>,
    mint: Mutex<Mint>,
    /// Every authorization the provider was asked for, for the tests.
    authorizations: Mutex<usize>,
}

type Fake = Arc<FakeState>;

/// Starts a fake provider and returns it with its issuer address.
async fn fake_provider() -> Fake {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let state = Arc::new(FakeState {
        issuer,
        keys: Keys::new(),
        grants: Mutex::new(HashMap::new()),
        mint: Mutex::new(Mint::default()),
        authorizations: Mutex::new(0),
    });
    let app = Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(|State(fake): State<Fake>| async move {
                Json(json!({
                    "issuer": fake.issuer,
                    "authorization_endpoint": format!("{}/authorize", fake.issuer),
                    "token_endpoint": format!("{}/token", fake.issuer),
                    "jwks_uri": format!("{}/jwks", fake.issuer),
                    "code_challenge_methods_supported": ["S256"],
                }))
            }),
        )
        .route(
            "/jwks",
            get(|State(fake): State<Fake>| async move { Json(fake.keys.jwks()) }),
        )
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .with_state(Arc::clone(&state));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    state
}

/// The user's browser arrives at the provider; the provider approves the
/// sign-in and sends the browser back with a code.
async fn authorize(
    State(fake): State<Fake>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    *fake.authorizations.lock().unwrap() += 1;
    let code = format!("code-{}", fake.authorizations.lock().unwrap());
    let grant = Grant {
        nonce: query.get("nonce").cloned().unwrap_or_default(),
        challenge: query.get("code_challenge").cloned().unwrap_or_default(),
        redirect_uri: query.get("redirect_uri").cloned().unwrap_or_default(),
    };
    fake.grants.lock().unwrap().insert(code.clone(), grant);
    let back = format!(
        "{}?code={code}&state={}&iss={}",
        query.get("redirect_uri").cloned().unwrap_or_default(),
        query.get("state").cloned().unwrap_or_default(),
        fake.issuer
    );
    Redirect::to(&back).into_response()
}

/// The token endpoint: one use per code, with PKCE and the client's secret
/// checked, then an ID token made as the current `Mint` says.
async fn token(State(fake): State<Fake>, headers: HeaderMap, body: String) -> Response {
    let form: HashMap<String, String> = url_form(&body);
    let expected = format!(
        "Basic {}",
        Base64::encode_string(format!("{CLIENT_ID}:{CLIENT_SECRET}").as_bytes())
    );
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "invalid_client" })),
        )
            .into_response();
    }
    let grant = form
        .get("code")
        .and_then(|code| fake.grants.lock().unwrap().remove(code));
    let Some(grant) = grant else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    let challenge = Base64UrlUnpadded::encode_string(&Sha256::digest(verifier.as_bytes()));
    if challenge != grant.challenge || form.get("redirect_uri") != Some(&grant.redirect_uri) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_grant" })),
        )
            .into_response();
    }
    let mint = fake.mint.lock().unwrap().clone();
    let issued = now();
    let mut claims = json!({
        "iss": fake.issuer,
        "aud": CLIENT_ID,
        "sub": "subject-1",
        "exp": issued + 300,
        "iat": issued,
        "nonce": grant.nonce,
        "email": "mo@example.com",
        "email_verified": true,
        "preferred_username": "mo.sso",
        "name": "Mo From SSO",
    });
    for (key, value) in &mint.claims {
        if value.is_null() {
            claims.as_object_mut().unwrap().remove(key);
        } else {
            claims[key] = value.clone();
        }
    }
    let header = json!({ "alg": mint.alg, "kid": mint.kid, "typ": "JWT" });
    let id_token = if mint.unsigned {
        format!("{}.{}.", part(&json!({ "alg": "none" })), part(&claims))
    } else {
        sign(&fake.keys, &header, &claims)
    };
    Json(json!({ "access_token": "not-used", "token_type": "Bearer", "id_token": id_token }))
        .into_response()
}

fn url_form(body: &str) -> HashMap<String, String> {
    body.split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| {
            (
                key.to_owned(),
                reqwest::Url::parse(&format!("http://x/?v={value}"))
                    .unwrap()
                    .query_pairs()
                    .next()
                    .unwrap()
                    .1
                    .into_owned(),
            )
        })
        .collect()
}

/// A server with a fake provider set up for it by the admin. `create` lets
/// a first sign-in make an account, which also needs sign-up to be open.
async fn setup(create: bool) -> (Server, Fake, Browser) {
    let fake = fake_provider().await;
    let server = start().await;
    let ada = admin(&server).await;
    let mut form = vec![
        ("issuer", fake.issuer.as_str()),
        ("client_id", CLIENT_ID),
        ("secret", CLIENT_SECRET),
        ("label", "Sign in with Example SSO"),
    ];
    if create {
        form.push(("create_accounts", "on"));
    }
    let response = ada.post("/admin/sign-in/oidc", &form).await;
    assert_eq!(response.status(), HttpStatus::OK, "{}", response.status());
    if create {
        let response = ada
            .post("/admin/community", &[("registration", "open")])
            .await;
        assert!(response.status().is_success() || response.status().is_redirection());
    }
    (server, fake, ada)
}

/// Whether the browser has a session: `/home` answers 200 only then.
async fn signed_in(browser: &Browser) -> bool {
    browser.get("/home").await.status() == HttpStatus::OK
}

/// Keeps the cookies a response sets, so the browser sends them back.
fn remember(browser: &mut Browser, response: &reqwest::Response) {
    let mut cookies: Vec<(String, String)> = browser
        .cookie
        .split("; ")
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_owned(), value.to_owned()))
        .filter(|(name, _)| !name.is_empty())
        .collect();
    for value in response.headers().get_all(header::SET_COOKIE) {
        let Some(pair) = value.to_str().ok().and_then(|text| text.split(';').next()) else {
            continue;
        };
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        cookies.retain(|(existing, _)| existing != name);
        if !value.is_empty() {
            cookies.push((name.to_owned(), value.to_owned()));
        }
    }
    browser.cookie = cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
}

/// Starts a sign-in at `path` (`/login/oidc` or a link), lets the provider
/// approve it, and returns the callback's address as the provider sent it.
async fn approve(browser: &mut Browser, path: &str) -> String {
    let response = browser.get(path).await;
    assert!(
        response.status().is_redirection(),
        "{path}: {}",
        response.status()
    );
    remember(browser, &response);
    let provider_page = location(&response);
    let approved = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(provider_page)
        .send()
        .await
        .unwrap();
    location(&approved)
}

/// Sends the browser back to Sideporch with the provider's answer.
async fn answer(browser: &mut Browser, callback: &str) -> reqwest::Response {
    let query = &callback[callback.find("/login/oidc/callback").unwrap()..];
    let response = browser.get(query).await;
    remember(browser, &response);
    response
}

/// Runs a whole sign-in from the browser's sign-in button.
async fn sign_in_with(browser: &mut Browser) -> reqwest::Response {
    let callback = approve(browser, "/login/oidc").await;
    answer(browser, &callback).await
}

fn sets_mint(fake: &Fake, mint: Mint) {
    *fake.mint.lock().unwrap() = mint;
}

fn claims_for(claims: &Value) -> Mint {
    Mint {
        claims: claims.as_object().unwrap().clone(),
        ..Mint::default()
    }
}

/// Connects a member's account to the provider, from their own settings.
async fn link(member: &mut Browser) {
    let callback = approve_post(member, "/settings/security/oidc").await;
    let response = answer(member, &callback).await;
    assert_eq!(location(&response), "/settings/security?notice=oidc");
}

/// Starts a link (a POST from the settings page) and returns the callback.
async fn approve_post(browser: &mut Browser, path: &str) -> String {
    let response = browser.post(path, &[]).await;
    // Browsers apply `form-action` to a form's redirect, so a POST here must
    // answer with a page that links to the provider instead.
    assert_eq!(response.status(), HttpStatus::OK, "{path}");
    remember(browser, &response);
    let page = response.text().await.unwrap();
    let href = page
        .split(r#"href=""#)
        .find(|part| part.starts_with("http"))
        .and_then(|part| part.split('"').next())
        .expect("a link to the provider")
        .replace("&amp;", "&");
    let approved = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(&href)
        .send()
        .await
        .unwrap();
    location(&approved)
}

fn database(server: &Server) -> rusqlite::Connection {
    rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap()
}

/// A member who signed in with their password, after linking the provider.
async fn linked_member(server: &Server, fake: &Fake, ada: &Browser) -> Browser {
    let mut mo = invite(server, ada, "Mo Member", "mo").await;
    link(&mut mo).await;
    let _ = fake;
    mo
}

/// Signs in with the provider and checks that a session was made for Mo.
async fn assert_signed_in_as_mo(response: reqwest::Response, browser: &Browser) {
    assert_eq!(location(&response), "/", "signed in");
    assert!(browser.page("/home").await.contains("Mo Member"));
}

#[tokio::test]
async fn a_first_sign_in_makes_no_account_while_there_are_rules_to_agree_to() {
    let (server, _fake, ada) = setup(true).await;
    let response = ada
        .post(
            "/admin/community",
            &[("registration", "open"), ("rules", "Be kind.")],
        )
        .await;
    assert!(response.status().is_success() || response.status().is_redirection());
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(
        response.text().await.unwrap().contains("isn't linked"),
        "the visitor is told why"
    );
}

#[tokio::test]
async fn link_then_sign_in_with_the_provider() {
    let (server, fake, ada) = setup(false).await;
    let mo = linked_member(&server, &fake, &ada).await;
    let page = mo.page("/settings/security").await;
    assert!(page.contains("Linked to this account"));

    // A new browser, not signed in, signs in with the provider.
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_signed_in_as_mo(response, &browser).await;
    drop(mo);
}

#[tokio::test]
async fn the_provider_sign_in_button_shows_on_the_login_page_only_when_set_up() {
    let server = start().await;
    let ada = admin(&server).await;
    assert!(
        !Browser::anonymous(&server)
            .page("/login")
            .await
            .contains("/login/oidc")
    );
    let fake = fake_provider().await;
    let response = ada
        .post(
            "/admin/sign-in/oidc",
            &[
                ("issuer", &fake.issuer),
                ("client_id", CLIENT_ID),
                ("secret", CLIENT_SECRET),
            ],
        )
        .await;
    assert_eq!(response.status(), HttpStatus::OK);
    let page = Browser::anonymous(&server).page("/login").await;
    assert!(page.contains("href=\"/login/oidc\""));
    assert!(page.contains("Sign in with single sign-on"));
}

#[tokio::test]
async fn second_factor_is_still_asked_after_the_provider() {
    let (server, fake, ada) = setup(false).await;
    let mo = linked_member(&server, &fake, &ada).await;
    // Mo sets up an authenticator app, which asks for a code at sign-in.
    let page = mo.page("/settings/security/totp").await;
    let key = between(&page, "select-all", "</code>")
        .split('>')
        .nth(1)
        .unwrap()
        .to_owned();
    let secret = base32(&key);
    let sealed = between(&page, "name=\"secret\" value=\"", "\"").to_owned();
    let step = step_now();
    let response = mo
        .post(
            "/settings/security/totp",
            &[("secret", &sealed), ("code", &code(&secret, step))],
        )
        .await;
    assert_eq!(response.status(), HttpStatus::OK);

    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(
        location(&response),
        "/login/verify",
        "a second step still applies"
    );
    assert!(!signed_in(&browser).await, "not signed in yet");
    let response = browser
        .submit("/login/verify", &[("code", &code(&secret, step + 1))])
        .await;
    assert_eq!(location(&response), "/");
    assert!(browser.page("/home").await.contains("Mo Member"));
}

#[tokio::test]
async fn an_es256_token_signs_in_too() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    sets_mint(
        &fake,
        Mint {
            alg: "ES256".to_owned(),
            kid: EC_KID.to_owned(),
            ..Mint::default()
        },
    );
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_signed_in_as_mo(response, &browser).await;
}

#[tokio::test]
async fn a_first_sign_in_creates_an_account_when_allowed_and_sign_up_is_open() {
    let (server, _fake, _ada) = setup(true).await;
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(location(&response), "/");
    // The name comes from preferred_username, and the account is new.
    assert!(browser.page("/home").await.contains("Mo From SSO"));
    let conn = database(&server);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM oidc_identities", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn a_first_sign_in_without_a_preferred_username_takes_a_name_from_the_email() {
    let (server, fake, _ada) = setup(true).await;
    sets_mint(&fake, claims_for(&json!({ "preferred_username": null })));
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(location(&response), "/");
    let conn = database(&server);
    let username: String = conn
        .query_row(
            "SELECT u.username FROM users u JOIN oidc_identities i ON i.user_id = u.id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(username, "mo");
}

#[tokio::test]
async fn an_unusable_preferred_username_also_falls_back_to_the_email() {
    let (server, fake, _ada) = setup(true).await;
    sets_mint(&fake, claims_for(&json!({ "preferred_username": "李明" })));
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(location(&response), "/");
    let conn = database(&server);
    let username: String = conn
        .query_row(
            "SELECT u.username FROM users u JOIN oidc_identities i ON i.user_id = u.id",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(username, "mo");
}

#[tokio::test]
async fn first_sign_in_is_refused_when_the_admin_did_not_allow_accounts() {
    let (server, _fake, _ada) = setup(false).await;
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
    let conn = database(&server);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM users", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1, "only Ada; no account was made");
}

#[tokio::test]
async fn an_email_that_matches_a_local_account_neither_signs_in_nor_links() {
    let (server, fake, ada) = setup(false).await;
    let _mo = invite(&server, &ada, "Mo Member", "mo").await;
    // Mo's address is confirmed, and the provider says it's the same one.
    database(&server)
        .execute(
            "UPDATE users SET email = 'mo@example.com' WHERE username = 'mo'",
            [],
        )
        .unwrap();
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
    let conn = database(&server);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM oidc_identities", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "an email never links an account");
    drop(fake);
}

#[tokio::test]
async fn a_sign_in_from_another_browser_cannot_finish_the_first() {
    let (server, fake, ada) = setup(false).await;
    let mut mo = linked_member(&server, &fake, &ada).await;
    let callback = approve(&mut mo, "/login/oidc").await;
    // The callback reaches another browser that never started the sign-in.
    let mut other = Browser::anonymous(&server);
    let response = answer(&mut other, &callback).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&other).await);
}

#[tokio::test]
async fn a_wrong_state_is_refused() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    let mut browser = Browser::anonymous(&server);
    let callback = approve(&mut browser, "/login/oidc").await;
    let forged = callback.replace("state=", "state=forged-");
    let response = answer(&mut browser, &forged).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
}

#[tokio::test]
async fn a_replayed_callback_is_refused() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    let mut browser = Browser::anonymous(&server);
    let callback = approve(&mut browser, "/login/oidc").await;
    // The browser's cookies as they were before its answer came back.
    let before = browser.cookie.clone();
    let first = answer(&mut browser, &callback).await;
    assert_eq!(location(&first), "/");
    let mut replayed = Browser::anonymous(&server);
    replayed.cookie = before;
    let second = answer(&mut replayed, &callback).await;
    assert_eq!(
        second.status(),
        HttpStatus::BAD_REQUEST,
        "a used login is gone"
    );
}

/// Runs a sign-in whose ID token the provider makes with `mint`, and returns
/// the answer and whether Mo's session exists afterwards.
async fn refused_with(mint: Mint) {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    sets_mint(&fake, mint);
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
}

#[tokio::test]
async fn a_wrong_nonce_is_refused() {
    refused_with(claims_for(&json!({ "nonce": "some-other-sign-in" }))).await;
}

#[tokio::test]
async fn a_wrong_audience_is_refused() {
    refused_with(claims_for(&json!({ "aud": "another-client" }))).await;
}

#[tokio::test]
async fn a_wrong_issuer_is_refused() {
    refused_with(claims_for(&json!({ "iss": "https://evil.example" }))).await;
}

#[tokio::test]
async fn an_expired_token_is_refused() {
    refused_with(claims_for(
        &json!({ "exp": now() - 600, "iat": now() - 900 }),
    ))
    .await;
}

#[tokio::test]
async fn alg_none_is_refused() {
    refused_with(Mint {
        unsigned: true,
        ..Mint::default()
    })
    .await;
}

#[tokio::test]
async fn an_hs256_token_signed_with_the_public_key_is_refused() {
    refused_with(Mint {
        alg: "HS256".to_owned(),
        ..Mint::default()
    })
    .await;
}

#[tokio::test]
async fn an_unknown_key_id_is_refused() {
    refused_with(Mint {
        kid: "rotated-away".to_owned(),
        ..Mint::default()
    })
    .await;
}

#[tokio::test]
async fn a_deactivated_account_is_refused() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    database(&server)
        .execute(
            "UPDATE users SET deactivated_at = 1 WHERE username = 'mo'",
            [],
        )
        .unwrap();
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
}

#[tokio::test]
async fn an_account_from_another_server_is_refused() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    let conn = database(&server);
    conn.execute(
        "INSERT INTO instances (url, handle, status, created_at, updated_at) VALUES ('https://other.example', 'other', 'connected', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE users SET instance_id = (SELECT id FROM instances WHERE handle = 'other') WHERE username = 'mo'",
        [],
    )
    .unwrap();
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
}

#[tokio::test]
async fn the_client_secret_never_appears_in_a_page() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    for path in ["/admin/sign-in", "/settings/security"] {
        let page = ada.page(path).await;
        assert!(!page.contains(CLIENT_SECRET), "{path} shows the secret");
    }
    let page = Browser::anonymous(&server).page("/login").await;
    assert!(!page.contains(CLIENT_SECRET));
    // Saving again without a secret keeps the stored one.
    let response = ada
        .post(
            "/admin/sign-in/oidc",
            &[
                ("issuer", &fake.issuer),
                ("client_id", CLIENT_ID),
                ("secret", ""),
            ],
        )
        .await;
    assert_eq!(response.status(), HttpStatus::OK);
    // The stored secret still works: the provider's token exchange needs it.
    let mut browser = Browser::anonymous(&server);
    assert_eq!(location(&sign_in_with(&mut browser).await), "/");
}

#[tokio::test]
async fn unlinking_stops_sign_ins_with_the_provider() {
    let (server, fake, ada) = setup(false).await;
    let mo = linked_member(&server, &fake, &ada).await;
    // Mo has a password from her invite, so the provider's sign-in can go.
    let response = mo.post("/settings/security/oidc/unlink", &[]).await;
    assert_eq!(
        location(&response),
        "/settings/security?notice=oidc-unlinked"
    );
    let mut browser = Browser::anonymous(&server);
    let response = sign_in_with(&mut browser).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(!signed_in(&browser).await);
}

#[tokio::test]
async fn an_admin_can_refuse_a_link_to_another_account() {
    let (server, fake, ada) = setup(false).await;
    let _mo = linked_member(&server, &fake, &ada).await;
    let mut ben = invite(&server, &ada, "Ben Member", "ben").await;
    // Ben tries to link the identity Mo already has.
    let callback = approve_post(&mut ben, "/settings/security/oidc").await;
    let response = answer(&mut ben, &callback).await;
    assert_eq!(response.status(), HttpStatus::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("already linked to another account")
    );
}

fn base32(text: &str) -> Vec<u8> {
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

fn step_now() -> u64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs())
        / 30
}
