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
