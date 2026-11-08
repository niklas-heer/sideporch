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

//! Signing in with passkeys, authenticator codes, recovery codes and email
//! links, and admins requiring them.

mod common;

use std::sync::{Arc, Mutex};

use base64ct::{Base64UrlUnpadded, Encoding as _};
use common::{Browser, admin, between, invite, location, start};
use reqwest::StatusCode;
use ring::{
    rand::SystemRandom,
    signature::{ECDSA_P256_SHA256_ASN1_SIGNING, EcdsaKeyPair, KeyPair as _},
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _, BufReader};

fn b64(bytes: &[u8]) -> String {
    Base64UrlUnpadded::encode_string(bytes)
}

/// A passkey on a pretend device: a P-256 key pair and its credential id.
struct Authenticator {
    pair: EcdsaKeyPair,
    rng: SystemRandom,
    credential: Vec<u8>,
    origin: String,
    rp_id: String,
    count: u32,
}

impl Authenticator {
    fn new(server_base: &str) -> Self {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        // Browsers only use passkeys for names, so the tests reach the
        // server as localhost.
        Self {
            pair,
            rng,
            credential: b"pretend-credential".to_vec(),
            origin: server_base.replace("127.0.0.1", "localhost"),
            rp_id: "localhost".to_owned(),
            count: 0,
        }
    }

    fn client_data(&self, kind: &str, challenge: &str) -> Vec<u8> {
        json!({ "type": kind, "challenge": challenge, "origin": self.origin })
            .to_string()
            .into_bytes()
    }

    fn auth_data(&mut self, attested: bool) -> Vec<u8> {
        self.count += 1;
        let mut data = Sha256::digest(self.rp_id.as_bytes()).to_vec();
        data.push(if attested { 0x45 } else { 0x05 });
        data.extend_from_slice(&self.count.to_be_bytes());
        if attested {
            data.extend_from_slice(&[0; 16]);
            data.extend_from_slice(&u16::try_from(self.credential.len()).unwrap().to_be_bytes());
            data.extend_from_slice(&self.credential);
            data.push(0xa0);
        }
        data
    }

    fn spki(&self) -> Vec<u8> {
        let mut spki = vec![
            0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06,
            0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
        ];
        spki.extend_from_slice(self.pair.public_key().as_ref());
        spki
    }

    /// Answers `navigator.credentials.create`.
    fn create(&mut self, options: &Value) -> Value {
        let challenge = options["publicKey"]["challenge"].as_str().unwrap();
        json!({
            "ceremony": options["ceremony"],
            "id": b64(&self.credential),
            "clientDataJSON": b64(&self.client_data("webauthn.create", challenge)),
            "authenticatorData": b64(&self.auth_data(true)),
            "publicKey": b64(&self.spki()),
            "publicKeyAlgorithm": -7,
            "transports": ["internal"],
            "name": "Test laptop",
        })
    }

    /// Answers `navigator.credentials.get`.
    fn get(&mut self, options: &Value) -> Value {
        let challenge = options["publicKey"]["challenge"].as_str().unwrap();
        let client = self.client_data("webauthn.get", challenge);
        let data = self.auth_data(false);
        let mut signed = data.clone();
        signed.extend_from_slice(&Sha256::digest(&client));
        let signature = self.pair.sign(&self.rng, &signed).unwrap();
        json!({
            "ceremony": options["ceremony"],
            "id": b64(&self.credential),
            "clientDataJSON": b64(&client),
            "authenticatorData": b64(&data),
            "signature": b64(signature.as_ref()),
        })
    }
}

/// Posts JSON the way app.js does, keeping any cookie the server sets.
async fn post_json(browser: &mut Browser, path: &str, body: &Value) -> reqwest::Response {
    let host = browser
        .base
        .trim_start_matches("http://")
        .replace("127.0.0.1", "localhost");
    let response = browser
        .client
        .post(browser.url(path))
        .header("cookie", &browser.cookie)
        .header("host", host)
        .header("x-sideporch-fetch", "1")
        .json(body)
        .send()
        .await
        .unwrap();
    if let Some(cookie) = response.headers().get("set-cookie") {
        cookie
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .clone_into(&mut browser.cookie);
    }
    response
}

/// Sends a form the way a browser at localhost does, keeping any cookie.
async fn submit_as_localhost(
    browser: &mut Browser,
    path: &str,
    form: &[(&str, &str)],
) -> reqwest::Response {
    let host = browser
        .base
        .trim_start_matches("http://")
        .replace("127.0.0.1", "localhost");
    let response = browser
        .client
        .post(browser.url(path))
        .header("cookie", &browser.cookie)
        .header("host", host)
        .form(form)
        .send()
        .await
        .unwrap();
    if let Some(cookie) = response.headers().get("set-cookie") {
        cookie
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .clone_into(&mut browser.cookie);
    }
    response
}

/// Makes a new account with a passkey through `form`, as app.js does:
/// asks for a ceremony, lets the device answer, and sends the form.
async fn create_with_passkey(
    browser: &mut Browser,
    device: &mut Authenticator,
    form: &str,
    display_name: &str,
    username: &str,
) -> reqwest::Response {
    let options = post_json(
        browser,
        "/webauthn/create/options",
        &json!({ "form": form, "display_name": display_name, "username": username }),
    )
    .await;
    assert_eq!(options.status(), StatusCode::OK);
    let options: Value = options.json().await.unwrap();
    assert_eq!(options["publicKey"]["user"]["name"], username);
    let answer = device.create(&options).to_string();
    submit_as_localhost(
        browser,
        form,
        &[
            ("display_name", display_name),
            ("username", username),
            ("passkey", &answer),
        ],
    )
    .await
}

async fn add_passkey(browser: &mut Browser, device: &mut Authenticator) {
    let options: Value = post_json(browser, "/webauthn/register/options", &json!({}))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(
        options["publicKey"]["authenticatorSelection"]["userVerification"],
        "required"
    );
    let answer = device.create(&options);
    let response = post_json(browser, "/webauthn/register", &answer).await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "{}",
        response.text().await.unwrap()
    );
}

async fn passkey_sign_in(browser: &mut Browser, device: &mut Authenticator) -> reqwest::Response {
    let options: Value = post_json(browser, "/webauthn/login/options", &json!({}))
        .await
        .json()
        .await
        .unwrap();
    let answer = device.get(&options);
    post_json(browser, "/webauthn/login", &answer).await
}

async fn sign_in(
    server: &common::Server,
    username: &str,
    password: &str,
) -> (Browser, reqwest::Response) {
    let mut browser = Browser::anonymous(server);
    let response = browser
        .submit("/login", &[("username", username), ("password", password)])
        .await;
    (browser, response)
}

#[tokio::test]
async fn passkeys_sign_in_on_their_own_and_as_a_second_step() {
    let server = start().await;
    let mut ada = admin(&server).await;
    let mut device = Authenticator::new(&server.base);
    add_passkey(&mut ada, &mut device).await;
    let page = ada.page("/settings/security").await;
    assert!(page.contains("Test laptop"));

    // Signing in with the passkey alone.
    let mut fresh = Browser::anonymous(&server);
    let response = passkey_sign_in(&mut fresh, &mut device).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["redirect"], "/");
    assert!(fresh.page("/home").await.contains("Ada Admin"));

    // A password now takes the passkey as a second step.
    let (mut second, response) = sign_in(&server, "ada", "correct horse").await;
    assert_eq!(location(&response), "/login/verify");
    assert_eq!(second.get("/home").await.status(), StatusCode::SEE_OTHER);
    assert!(
        second
            .page("/login/verify")
            .await
            .contains("Use your passkey")
    );
    let options: Value = post_json(&mut second, "/webauthn/login/options", &json!({}))
        .await
        .json()
        .await
        .unwrap();
    // The options name this account's passkey.
    assert_eq!(
        options["publicKey"]["allowCredentials"][0]["id"],
        b64(b"pretend-credential")
    );
    let answer = device.get(&options);
    assert_eq!(
        post_json(&mut second, "/webauthn/login", &answer)
            .await
            .status(),
        StatusCode::OK
    );
    assert!(second.page("/home").await.contains("Ada Admin"));

    // A replayed answer doesn't work twice.
    let mut replay = Browser::anonymous(&server);
    assert_eq!(
        post_json(&mut replay, "/webauthn/login", &answer)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    // Removing the passkey removes the second step.
    let page = ada.page("/settings/security").await;
    let id = between(&page, "/settings/security/passkeys/", "/delete").to_owned();
    ada.post(&format!("/settings/security/passkeys/{id}/delete"), &[])
        .await;
    let (_, response) = sign_in(&server, "ada", "correct horse").await;
    assert_eq!(location(&response), "/");
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

fn step_now() -> u64 {
    (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs())
        / 30
}

#[tokio::test]
async fn new_accounts_can_start_with_a_passkey_instead_of_a_password() {
    let server = start().await;
    let page = Browser::anonymous(&server).page("/setup").await;
    assert!(page.contains("Create account with a passkey"));

    // The admin sets up with a passkey and is signed in right away.
    let mut ada = Browser::anonymous(&server);
    let mut device = Authenticator::new(&server.base);
    let response = create_with_passkey(&mut ada, &mut device, "/setup", "Ada Admin", "ada").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(ada.page("/home").await.contains("Ada Admin"));
    assert!(ada.page("/settings/security").await.contains("Test laptop"));

    // No password works for it, and the passkey signs in on its own.
    let (_, response) = sign_in(&server, "ada", "").await;
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
    let mut again = Browser::anonymous(&server);
    assert_eq!(
        passkey_sign_in(&mut again, &mut device).await.status(),
        StatusCode::OK
    );

    // Its only passkey can't be removed until there's another way in.
    let security = ada.page("/settings/security").await;
    let delete = format!(
        "/settings/security/passkeys/{}/delete",
        between(&security, "/settings/security/passkeys/", "/delete")
    );
    assert_eq!(
        ada.post(&delete, &[]).await.status(),
        StatusCode::BAD_REQUEST
    );
    let account = ada.page("/settings/account").await;
    assert!(account.contains("Add a password"));
    assert!(!account.contains("Current password"));
    let added = ada
        .post("/settings/account", &[("password", "correct horse")])
        .await;
    assert_eq!(added.status(), StatusCode::OK);
    // A password now asks for the passkey as the second step.
    let (_, response) = sign_in(&server, "ada", "correct horse").await;
    assert_eq!(location(&response), "/login/verify");
    assert_eq!(ada.post(&delete, &[]).await.status(), StatusCode::SEE_OTHER);

    // People joining with an invite can start with a passkey too, but only
    // for the name and username the passkey was made for.
    assert_eq!(
        ada.post("/invites", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    let people = ada.page("/people").await;
    let join = format!("/join/{}", between(&people, "/join/", "<"));
    let mut eve = Browser::anonymous(&server);
    let mut eve_device = Authenticator::new(&server.base);
    eve_device.credential = b"eve-credential".to_vec();
    let options: Value = post_json(
        &mut eve,
        "/webauthn/create/options",
        &json!({ "form": join, "display_name": "Eve", "username": "eve" }),
    )
    .await
    .json()
    .await
    .unwrap();
    let answer = eve_device.create(&options).to_string();
    let swapped = submit_as_localhost(
        &mut eve,
        &join,
        &[
            ("display_name", "Eve"),
            ("username", "mallory"),
            ("passkey", &answer),
        ],
    )
    .await;
    assert_eq!(swapped.status(), StatusCode::BAD_REQUEST);
    let mut bea = Browser::anonymous(&server);
    let mut bea_device = Authenticator::new(&server.base);
    bea_device.credential = b"bea-credential".to_vec();
    let joined = create_with_passkey(&mut bea, &mut bea_device, &join, "Bea", "bea").await;
    assert_eq!(joined.status(), StatusCode::SEE_OTHER);
    assert!(bea.page("/home").await.contains("Bea"));

    // Forms that can't make accounts don't start passkeys.
    for form in ["/setup", "/signup", "/join/not-an-invite", "/login"] {
        let refused = post_json(
            &mut Browser::anonymous(&server),
            "/webauthn/create/options",
            &json!({ "form": form, "display_name": "Zed", "username": "zed" }),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{form}");
    }
}

#[tokio::test]
async fn open_sign_up_can_start_with_a_passkey_but_requests_to_join_cannot() {
    let server = start().await;
    let ada = admin(&server).await;
    common::set_registration(&ada, "open", "").await;
    let mut nia = Browser::anonymous(&server);
    let (challenge, proof) = common::sign_up_proof(&nia).await;
    let mut device = Authenticator::new(&server.base);
    let options: Value = post_json(
        &mut nia,
        "/webauthn/create/options",
        &json!({ "form": "/signup", "display_name": "Nia", "username": "nia" }),
    )
    .await
    .json()
    .await
    .unwrap();
    let answer = device.create(&options).to_string();
    let response = submit_as_localhost(
        &mut nia,
        "/signup",
        &[
            ("display_name", "Nia"),
            ("username", "nia"),
            ("passkey", &answer),
            ("website", ""),
            ("challenge", &challenge),
            ("proof", &proof),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(nia.page("/settings/security").await.contains("Test laptop"));

    // Waiting requests keep a password until someone approves them.
    common::set_registration(&ada, "approval", "").await;
    assert!(
        !Browser::anonymous(&server)
            .page("/signup")
            .await
            .contains("Create account with a passkey")
    );
    let refused = post_json(
        &mut Browser::anonymous(&server),
        "/webauthn/create/options",
        &json!({ "form": "/signup", "display_name": "Ola", "username": "ola" }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_setup_code_is_checked_before_a_passkey_is_made() {
    let server = common::start_with(|config| {
        config.setup_code = Some("porch-light-0123456789".to_owned());
    })
    .await;
    let mut visitor = Browser::anonymous(&server);
    let refused = post_json(
        &mut visitor,
        "/webauthn/create/options",
        &json!({ "form": "/setup", "display_name": "Ada", "username": "ada", "setup_code": "wrong" }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(
        refused
            .text()
            .await
            .unwrap()
            .contains("That isn't the setup code.")
    );
    // A made-up setup link doesn't get around the code either.
    let linked = post_json(
        &mut visitor,
        "/webauthn/create/options",
        &json!({ "form": "/setup/porch-light-0123456789", "display_name": "Ada", "username": "ada" }),
    )
    .await;
    assert_eq!(linked.status(), StatusCode::BAD_REQUEST);
    let options: Value = post_json(
        &mut visitor,
        "/webauthn/create/options",
        &json!({ "form": "/setup", "display_name": "Ada", "username": "ada", "setup_code": "porch-light-0123456789" }),
    )
    .await
    .json()
    .await
    .unwrap();
    let mut device = Authenticator::new(&server.base);
    let answer = device.create(&options).to_string();
    // The form still needs the code itself.
    let without = submit_as_localhost(
        &mut visitor,
        "/setup",
        &[
            ("display_name", "Ada"),
            ("username", "ada"),
            ("passkey", &answer),
        ],
    )
    .await;
    assert_eq!(without.status(), StatusCode::FORBIDDEN);
}

/// Sends a request with `browser`'s session as a browser at localhost
/// would, but holds back its body until `meanwhile` ran, and returns the
/// raw answer. Requests are let in before their body arrives.
async fn hold_open(
    server: &common::Server,
    browser: &Browser,
    path: &str,
    content_type: &str,
    body: &str,
    meanwhile: impl FnOnce(),
) -> String {
    let address = server.base.trim_start_matches("http://").to_owned();
    let port = address.rsplit(':').next().unwrap();
    let mut stream = tokio::net::TcpStream::connect(&address).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost:{port}\r\nCookie: {}\r\nContent-Type: {content_type}\r\nx-sideporch-fetch: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        browser.cookie,
        body.len()
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    meanwhile();
    stream.write_all(body.as_bytes()).await.unwrap();
    let mut answer = String::new();
    tokio::io::AsyncReadExt::read_to_string(&mut stream, &mut answer)
        .await
        .unwrap();
    answer
}

#[tokio::test]
async fn requests_held_open_across_recovery_add_no_way_in() {
    let server = start().await;
    let mut ada = admin(&server).await;
    let mut device = Authenticator::new(&server.base);
    let options: Value = post_json(&mut ada, "/webauthn/register/options", &json!({}))
        .await
        .json()
        .await
        .unwrap();
    let passkey = device.create(&options).to_string();
    let recover = || {
        sideporch::reset_link(server.data_dir(), "ada", true).unwrap();
    };
    let answer = hold_open(
        &server,
        &ada,
        "/webauthn/register",
        "application/json",
        &passkey,
        recover,
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    let database = rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap();
    let passkeys: i64 = database
        .query_row("SELECT COUNT(*) FROM passkeys", [], |row| row.get(0))
        .unwrap();
    assert_eq!(passkeys, 0);

    // A password change held open the same way doesn't end the recovery.
    let server = start().await;
    let ada = admin(&server).await;
    let answer = hold_open(
        &server,
        &ada,
        "/settings/account",
        "application/x-www-form-urlencoded",
        "current=correct+horse&password=the+thief+chose+this",
        || {
            sideporch::reset_link(server.data_dir(), "ada", true).unwrap();
        },
    )
    .await;
    assert!(answer.starts_with("HTTP/1.1 403"), "{answer}");
    let (_, response) = sign_in(&server, "ada", "the thief chose this").await;
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn an_account_being_recovered_gets_no_session() {
    let server = start().await;
    drop(admin(&server).await);
    // Recovery begun (as the command does) while the password still works:
    // signing in with it starts no session.
    let database = rusqlite::Connection::open(server.data_dir().join("sideporch.db")).unwrap();
    database
        .execute("UPDATE users SET recovering_since = 1", [])
        .unwrap();
    let (_, response) = sign_in(&server, "ada", "correct horse").await;
    assert_ne!(response.status(), StatusCode::SEE_OTHER);
    assert!(response.text().await.unwrap().contains("being recovered"));
}

#[tokio::test]
async fn reset_links_from_the_command_line_let_admins_back_in() {
    let server = start().await;
    drop(admin(&server).await);
    assert!(sideporch::reset_link(server.data_dir(), "nobody", false).is_err());
    let link = sideporch::reset_link(server.data_dir(), "@Ada", false).unwrap();
    assert!(link.path.starts_with("/reset/"));
    assert!(!link.second_step);
    let mut browser = Browser::anonymous(&server);
    assert!(
        browser
            .page(&link.path)
            .await
            .contains("Choose a new password")
    );
    let response = browser
        .submit(&link.path, &[("password", "a brand new one")])
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let (_, response) = sign_in(&server, "ada", "a brand new one").await;
    assert_eq!(location(&response), "/");
}

#[tokio::test]
async fn an_admin_who_lost_their_only_passkey_gets_back_in_from_the_server() {
    let server = start().await;
    let mut ada = Browser::anonymous(&server);
    // The passkey is lost, maybe with a laptop someone else now has.
    let mut lost = Authenticator::new(&server.base);
    let created = create_with_passkey(&mut ada, &mut lost, "/setup", "Ada Admin", "ada").await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);

    // A reset link alone still asks for the lost passkey, and says so.
    let link = sideporch::reset_link(server.data_dir(), "ada", false).unwrap();
    assert!(link.second_step);
    let mut stuck = Browser::anonymous(&server);
    let response = stuck
        .submit(&link.path, &[("password", "a brand new one")])
        .await;
    assert_eq!(location(&response), "/login/verify");

    // Whoever has the lost device is signed in, and knows the password.
    let mut thief = Browser::anonymous(&server);
    assert_eq!(
        passkey_sign_in(&mut thief, &mut lost).await.status(),
        StatusCode::OK
    );
    assert_eq!(thief.get("/home").await.status(), StatusCode::OK);

    // Resetting sign-in too shuts every other way in at once: the session,
    // the passkey and the password. Only the new link works.
    let link = sideporch::reset_link(server.data_dir(), "ada", true).unwrap();
    assert!(!link.second_step);
    assert_eq!(thief.get("/home").await.status(), StatusCode::SEE_OTHER);
    assert_ne!(
        passkey_sign_in(&mut Browser::anonymous(&server), &mut lost)
            .await
            .status(),
        StatusCode::OK
    );
    let (_, old_password) = sign_in(&server, "ada", "a brand new one").await;
    assert_ne!(old_password.status(), StatusCode::SEE_OTHER);

    let mut back = Browser::anonymous(&server);
    let response = back
        .submit(&link.path, &[("password", "another new one")])
        .await;
    assert_eq!(location(&response), "/");
    assert!(back.page("/home").await.contains("Ada Admin"));
    assert!(
        !back
            .page("/settings/security")
            .await
            .contains("Test laptop")
    );
}

#[tokio::test]
async fn passkeys_need_a_name_not_an_ip_address() {
    let server = start().await;
    let ada = admin(&server).await;
    let response = ada
        .client
        .post(ada.url("/webauthn/register/options"))
        .header("cookie", &ada.cookie)
        .json(&json!({}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(response.text().await.unwrap().contains("not an IP address"));
}

#[tokio::test]
async fn authenticator_apps_and_recovery_codes() {
    let server = start().await;
    let ada = admin(&server).await;
    let page = ada.page("/settings/security/totp").await;
    assert!(page.contains("<svg"), "shows a QR code");
    let key: String = between(&page, "select-all", "</code>")
        .split('>')
        .nth(1)
        .unwrap()
        .to_owned();
    let secret = from_base32(&key);
    let sealed = between(&page, "name=\"secret\" value=\"", "\"").to_owned();
    // A wrong code doesn't turn it on.
    let response = ada
        .post(
            "/settings/security/totp",
            &[("secret", &sealed), ("code", "000000")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let now = step_now();
    let response = ada
        .post(
            "/settings/security/totp",
            &[("secret", &sealed), ("code", &code(&secret, now))],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = response.text().await.unwrap();
    assert!(page.contains("Your recovery codes"));
    let codes: Vec<String> = between(&page, "grid-cols-2 gap-1 font-mono", "</ul>")
        .split("<li>")
        .skip(1)
        .map(|item| item.split('<').next().unwrap().to_owned())
        .collect();
    assert_eq!(codes.len(), 10);

    // Signing in asks for a code now; a code works once.
    let (mut browser, response) = sign_in(&server, "ada", "correct horse").await;
    assert_eq!(location(&response), "/login/verify");
    let used = code(&secret, now);
    let response = browser.submit("/login/verify", &[("code", &used)]).await;
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the step it was set up with is used"
    );
    let next = code(&secret, now + 1);
    let response = browser.submit("/login/verify", &[("code", &next)]).await;
    assert_eq!(location(&response), "/");
    assert!(browser.page("/home").await.contains("Ada Admin"));

    // A recovery code works once, too.
    let (mut browser, _) = sign_in(&server, "ada", "correct horse").await;
    let response = browser
        .submit("/login/verify", &[("recovery", &codes[0].to_uppercase())])
        .await;
    assert_eq!(location(&response), "/");
    let (mut browser, _) = sign_in(&server, "ada", "correct horse").await;
    let response = browser
        .submit("/login/verify", &[("recovery", &codes[0])])
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // After five wrong tries the pending sign-in ends.
    for _ in 0..5 {
        browser.post("/login/verify", &[("code", "111111")]).await;
    }
    let response = browser
        .post("/login/verify", &[("recovery", &codes[1])])
        .await;
    assert_eq!(location(&response), "/login");
}

#[tokio::test]
async fn admins_can_require_a_second_step_or_passkeys() {
    let server = start().await;
    let mut ada = admin(&server).await;
    let mut mo = invite(&server, &ada, "Mo Member", "mo").await;
    // Admins must meet the policy themselves first.
    let response = ada.post("/admin/sign-in", &[("require", "everyone")]).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("your own account first")
    );
    let mut ada_device = Authenticator::new(&server.base);
    add_passkey(&mut ada, &mut ada_device).await;
    assert_eq!(
        ada.post("/admin/sign-in", &[("require", "everyone")])
            .await
            .status(),
        StatusCode::OK
    );
    // Mo is sent to set something up before anything else.
    let response = mo.get("/home").await;
    assert_eq!(location(&response), "/settings/security");
    assert!(
        mo.page("/settings/security")
            .await
            .contains("asks you to add a passkey")
    );
    let mut mo_device = Authenticator::new(&server.base);
    mo_device.credential = b"mo-credential".to_vec();
    add_passkey(&mut mo, &mut mo_device).await;
    assert_eq!(mo.get("/home").await.status(), StatusCode::OK);

    // With passkeys required, a password no longer signs Mo in.
    ada.post("/admin/sign-in", &[("require", "passkeys")]).await;
    let (_, response) = sign_in(&server, "mo", "a long password").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("signs in with passkeys")
    );
    let mut fresh = Browser::anonymous(&server);
    assert_eq!(
        passkey_sign_in(&mut fresh, &mut mo_device).await.status(),
        StatusCode::OK
    );

    // An admin resets Mo's passkeys after a lost phone; Mo's password gets
    // them in to add a new one.
    let mo_id = mo.user_id().await;
    ada.post(&format!("/people/{mo_id}/reset-security"), &[])
        .await;
    let (mut again, response) = sign_in(&server, "mo", "a long password").await;
    assert_eq!(location(&response), "/");
    assert_eq!(location(&again.get("/home").await), "/settings/security");
    let mut new_phone = Authenticator::new(&server.base);
    new_phone.credential = b"mo-new-phone".to_vec();
    add_passkey(&mut again, &mut new_phone).await;
    assert_eq!(again.get("/home").await.status(), StatusCode::OK);
}

/// A mail server that accepts everything and keeps what it got.
async fn fake_smtp() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mails = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&mails);
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            let kept = Arc::clone(&kept);
            tokio::spawn(async move {
                let (read, mut write) = socket.into_split();
                let mut lines = BufReader::new(read).lines();
                write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                let mut data = String::new();
                let mut in_data = false;
                while let Ok(Some(line)) = lines.next_line().await {
                    if in_data {
                        if line == "." {
                            in_data = false;
                            kept.lock().unwrap().push(std::mem::take(&mut data));
                            write.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            data.push_str(&line);
                            data.push('\n');
                        }
                        continue;
                    }
                    let command = line.to_uppercase();
                    let reply: &[u8] = if command.starts_with("EHLO") {
                        b"250-fake\r\n250 8BITMIME\r\n"
                    } else if command.starts_with("DATA") {
                        in_data = true;
                        b"354 go ahead\r\n"
                    } else if command.starts_with("QUIT") {
                        write.write_all(b"221 bye\r\n").await.unwrap();
                        break;
                    } else {
                        b"250 ok\r\n"
                    };
                    write.write_all(reply).await.unwrap();
                }
            });
        }
    });
    (port, mails)
}

/// The link a mail holds, without the server's address.
fn link_in(mail: &str, path: &str) -> String {
    // Quoted-printable may wrap long lines with a trailing `=`.
    let joined = mail.replace("=\n", "");
    let start = joined.find(path).unwrap();
    joined[start..]
        .split(|c: char| c.is_whitespace())
        .next()
        .unwrap()
        .to_owned()
}

/// Sets up email through a fake mail server, confirms ada@example.com for
/// `ada`, and turns on sign-in links; returns the mail sent.
async fn with_email(ada: &Browser) -> Arc<Mutex<Vec<String>>> {
    let (port, mails) = fake_smtp().await;
    let port = port.to_string();
    let settings = [
        ("require", "none"),
        ("mail_host", "127.0.0.1"),
        ("mail_port", port.as_str()),
        ("mail_security", "none"),
        ("mail_from", "Sideporch <chat@example.com>"),
        ("email_links", "on"),
    ];
    assert_eq!(
        ada.post("/admin/sign-in", &settings).await.status(),
        StatusCode::OK
    );
    ada.post("/settings/security/email", &[("email", "ada@example.com")])
        .await;
    let mail = mails.lock().unwrap().last().unwrap().clone();
    let confirm = link_in(&mail, "/settings/security/email/");
    assert_eq!(ada.get(&confirm).await.status(), StatusCode::SEE_OTHER);
    mails
}

/// Asks for ada's email sign-in link, or with `reset`, a reset link.
async fn ask_for_link(browser: &Browser, purpose: &str) {
    browser
        .post(
            "/login/email",
            &[("email", "ada@example.com"), ("purpose", purpose)],
        )
        .await;
}

#[tokio::test]
async fn while_the_server_recovers_an_account_email_sends_no_links() {
    let server = start().await;
    let ada = admin(&server).await;
    let mails = with_email(&ada).await;
    let anonymous = Browser::anonymous(&server);
    // The mailbox may be open on the lost device, with a reset link in it.
    ask_for_link(&anonymous, "reset").await;
    let early = link_in(&mails.lock().unwrap().last().unwrap().clone(), "/reset/");
    let recovery = sideporch::reset_link(server.data_dir(), "ada", true).unwrap();
    let sent = mails.lock().unwrap().len();
    ask_for_link(&anonymous, "").await;
    ask_for_link(&anonymous, "reset").await;
    assert_eq!(
        mails.lock().unwrap().len(),
        sent,
        "no mail while recovering"
    );
    assert_eq!(anonymous.get(&early).await.status(), StatusCode::GONE);

    let mut browser = Browser::anonymous(&server);
    let response = browser
        .submit(&recovery.path, &[("password", "recovered at last")])
        .await;
    assert_eq!(location(&response), "/");
    // Afterwards, email works again.
    ask_for_link(&anonymous, "").await;
    assert_eq!(mails.lock().unwrap().len(), sent + 1);
}

#[tokio::test]
async fn email_confirms_addresses_signs_in_and_resets_passwords() {
    let server = start().await;
    let (port, mails) = fake_smtp().await;
    let ada = admin(&server).await;
    let port = port.to_string();
    let settings = [
        ("require", "none"),
        ("mail_host", "127.0.0.1"),
        ("mail_port", port.as_str()),
        ("mail_security", "none"),
        ("mail_from", "Sideporch <chat@example.com>"),
    ];
    assert_eq!(
        ada.post("/admin/sign-in", &settings).await.status(),
        StatusCode::OK
    );
    let response = ada
        .post("/admin/sign-in/test-email", &[("to", "ada@example.com")])
        .await;
    assert!(response.text().await.unwrap().contains("Sent a test email"));
    assert!(mails.lock().unwrap()[0].contains("It works."));

    // Confirming an address.
    let response = ada
        .post("/settings/security/email", &[("email", "Ada@Example.com")])
        .await;
    assert_eq!(location(&response), "/settings/security?notice=sent");
    let mail = mails.lock().unwrap().last().unwrap().clone();
    assert!(mail.contains("To: ada@example.com"));
    let confirm = link_in(&mail, "/settings/security/email/");
    assert_eq!(ada.get(&confirm).await.status(), StatusCode::SEE_OTHER);
    assert!(
        ada.page("/settings/security")
            .await
            .contains("ada@example.com")
    );

    // Sign-in links, once turned on.
    let mut settings = settings.to_vec();
    settings.push(("email_links", "on"));
    assert_eq!(
        ada.post("/admin/sign-in", &settings).await.status(),
        StatusCode::OK
    );
    let anonymous = Browser::anonymous(&server);
    assert!(
        anonymous
            .page("/login")
            .await
            .contains("Email me a sign-in link")
    );
    let sent = mails.lock().unwrap().len();
    // Unknown addresses get the same answer, and no mail.
    let page = anonymous
        .post("/login/email", &[("email", "nobody@example.com")])
        .await
        .text()
        .await
        .unwrap();
    assert!(page.contains("If an account here has that address"));
    assert_eq!(mails.lock().unwrap().len(), sent);
    anonymous
        .post("/login/email", &[("email", "ada@example.com")])
        .await;
    let mail = mails.lock().unwrap().last().unwrap().clone();
    let link = link_in(&mail, "/login/link/");
    // Opening the link doesn't sign in yet: mail scanners open links too.
    assert!(anonymous.page(&link).await.contains("Continue to sign in"));
    let mut browser = Browser::anonymous(&server);
    let response = browser.submit(&link, &[]).await;
    assert_eq!(location(&response), "/");
    assert!(browser.page("/home").await.contains("Ada Admin"));
    assert_eq!(browser.post(&link, &[]).await.status(), StatusCode::GONE);

    // Forgotten passwords.
    anonymous
        .post(
            "/login/email",
            &[("email", "ada@example.com"), ("purpose", "reset")],
        )
        .await;
    let mail = mails.lock().unwrap().last().unwrap().clone();
    assert!(mail.contains("choose a new password"));
    let reset = link_in(&mail, "/reset/");
    let mut browser = Browser::anonymous(&server);
    let response = browser
        .submit(&reset, &[("password", "a brand new one")])
        .await;
    assert_eq!(location(&response), "/");
    let (_, response) = sign_in(&server, "ada", "a brand new one").await;
    assert_eq!(location(&response), "/");
}
