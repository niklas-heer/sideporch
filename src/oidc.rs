//! Signing in with an `OpenID Connect` provider (Keycloak, Authentik, Google
//! and the like). The admin configures one provider. Sideporch then runs the
//! authorization-code flow with PKCE, and checks the ID token it gets back
//! with ring: only RS256 and ES256, the key chosen by `kid` from the
//! provider's own keys, and the issuer, audience, time, and nonce.
//!
//! A sign-in finds the account that the provider identity (its issuer and
//! subject) is linked to. An email claim never links one: it isn't proof that
//! someone owns a local account. Flows live in [`crate::routes::oidc`].

use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    http::{Request, header},
};
use base64ct::{Base64, Base64UrlUnpadded, Encoding as _};
use http_body_util::{BodyExt as _, Full, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use ring::signature::{
    ECDSA_P256_SHA256_FIXED, RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents, UnparsedPublicKey,
};
use rusqlite::{Connection, OptionalExtension as _};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use crate::{
    auth,
    error::{AppError, AppResult},
    secrets::Vault,
};

/// Where the browser that starts a sign-in comes back to, at the provider's
/// redirect.
pub const CALLBACK_PATH: &str = "/login/oidc/callback";
/// The browser's proof that it started the sign-in, which only it has.
pub const BROWSER_COOKIE: &str = "sideporch_oidc";
/// How long a sign-in may take, from the redirect to the provider.
pub const LOGIN_MINUTES: i64 = 10;
/// How far clocks may disagree.
const SKEW_SECONDS: i64 = 60;
/// How long a provider's discovery document is kept.
const DISCOVERY_EVERY: Duration = Duration::from_hours(1);
/// How often the keys are refreshed anyway, to follow rotation.
const REFRESH_EVERY: Duration = Duration::from_hours(1);
/// How often unknown key ids may make Sideporch ask the provider again.
const REFETCH_EVERY: Duration = Duration::from_mins(1);
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DOCUMENT_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 32 * 1024;
const MAX_ID_TOKEN_BYTES: usize = 16 * 1024;

// Settings, in the `settings` table. The secret is sealed with the vault.
const KEY_ISSUER: &str = "oidc.issuer";
const KEY_CLIENT_ID: &str = "oidc.client_id";
const KEY_SECRET: &str = "oidc.client_secret";
const KEY_LABEL: &str = "oidc.label";
const KEY_DOMAIN: &str = "oidc.domain";
const KEY_CREATE: &str = "oidc.create_accounts";
const DEFAULT_LABEL: &str = "Sign in with single sign-on";

type HttpClient = Client<HttpsConnector<HttpConnector>, Full<Bytes>>;

/// What the admin set up. The secret itself is never part of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Setup {
    pub issuer: String,
    pub client_id: String,
    pub label: String,
    pub domain: String,
    pub create_accounts: bool,
    /// Whether a client secret is stored, sealed.
    pub secret_saved: bool,
}

/// A provider ready to sign people in, secret included.
#[derive(Debug, Clone)]
pub struct Provider {
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
    /// Only people whose verified address is in this domain may sign in.
    pub domain: Option<String>,
    /// Whether a first sign-in may create an account (sign-up must be open).
    pub create_accounts: bool,
}

/// What the admin submitted. An empty `secret` keeps the stored one.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub issuer: String,
    pub client_id: String,
    pub label: String,
    pub domain: String,
    pub create_accounts: bool,
    pub secret: String,
}

impl Setup {
    /// Whether the provider can be used: its address, client id and secret.
    pub const fn configured(&self) -> bool {
        !self.issuer.is_empty() && !self.client_id.is_empty() && self.secret_saved
    }

    /// What the admin set up, without decrypting anything.
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let text = |key: &str| crate::store::setting(conn, key).map(Option::unwrap_or_default);
        Ok(Self {
            issuer: text(KEY_ISSUER)?,
            client_id: text(KEY_CLIENT_ID)?,
            label: text(KEY_LABEL)?,
            domain: text(KEY_DOMAIN)?,
            create_accounts: crate::store::setting(conn, KEY_CREATE)?.as_deref() == Some("true"),
            secret_saved: crate::store::setting(conn, KEY_SECRET)?.is_some(),
        })
    }

    /// The label to show on buttons, or the default.
    pub fn button_label(&self) -> &str {
        if self.label.is_empty() {
            DEFAULT_LABEL
        } else {
            &self.label
        }
    }
}

/// The provider to sign in with, if the admin set one up. Decrypts the
/// secret, so only the flows that talk to the provider call it.
pub fn provider(conn: &Connection, vault: &Vault) -> AppResult<Option<Provider>> {
    let setup = Setup::load(conn)?;
    if !setup.configured() {
        return Ok(None);
    }
    let sealed = crate::store::setting(conn, KEY_SECRET)?.unwrap_or_default();
    let client_secret = vault.open_text(KEY_SECRET, &sealed)?;
    Ok(Some(Provider {
        domain: (!setup.domain.is_empty()).then_some(setup.domain),
        issuer: setup.issuer,
        client_id: setup.client_id,
        client_secret,
        create_accounts: setup.create_accounts,
    }))
}

/// Checks what the admin submitted and stores it. An empty secret keeps the
/// stored one; a new one needs a stored one or a new value.
///
/// # Errors
///
/// Returns `BadRequest` with what's wrong, or a database error.
pub fn save(conn: &Connection, vault: &Vault, input: &Input) -> AppResult<()> {
    let issuer = parse_issuer(&input.issuer).map_err(AppError::BadRequest)?;
    let client_id = input.client_id.trim();
    if client_id.is_empty()
        || client_id.len() > 255
        || client_id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AppError::bad_request(
            "Enter the client id the provider gave Sideporch.",
        ));
    }
    let label = input.label.trim();
    if label.chars().count() > 60 {
        return Err(AppError::bad_request(
            "The button label can be at most 60 characters.",
        ));
    }
    let domain = input.domain.trim().trim_start_matches('@').to_lowercase();
    if !domain.is_empty() && !valid_domain(&domain) {
        return Err(AppError::bad_request(
            "The email domain looks like example.com, without an @ or a path.",
        ));
    }
    let secret = input.secret.trim();
    if secret.len() > 2048 {
        return Err(AppError::bad_request("That client secret is too long."));
    }
    let saved = crate::store::setting(conn, KEY_SECRET)?.is_some();
    if secret.is_empty() && !saved {
        return Err(AppError::bad_request(
            "Enter the client secret the provider gave Sideporch.",
        ));
    }
    let text = |key: &str, value: &str| crate::store::set_setting(conn, key, value);
    text(KEY_ISSUER, &issuer)?;
    text(KEY_CLIENT_ID, client_id)?;
    text(KEY_LABEL, label)?;
    text(KEY_DOMAIN, &domain)?;
    text(
        KEY_CREATE,
        if input.create_accounts {
            "true"
        } else {
            "false"
        },
    )?;
    if !secret.is_empty() {
        let sealed = vault.seal_text(KEY_SECRET, secret)?;
        text(KEY_SECRET, &sealed)?;
    }
    Ok(())
}

/// Turns the provider off. Accounts keep their links, which work again if
/// the provider is set up again with the same address.
pub fn clear(conn: &Connection) -> AppResult<()> {
    for key in [
        KEY_ISSUER,
        KEY_CLIENT_ID,
        KEY_SECRET,
        KEY_LABEL,
        KEY_DOMAIN,
        KEY_CREATE,
    ] {
        conn.execute("DELETE FROM settings WHERE key = ?1", [key])?;
    }
    Ok(())
}

/// The issuer exactly as the provider writes it. Only `https` is accepted,
/// or `http` for a loopback address, which is for testing.
///
/// # Errors
///
/// Says what's wrong with an address that can't be the issuer.
pub fn parse_issuer(input: &str) -> Result<String, String> {
    let issuer = input.trim();
    let refused = || {
        "The issuer is the provider's address, like https://login.example.com/realms/team, and must use https."
            .to_owned()
    };
    if !transport_allowed(issuer) || issuer.contains(['?', '#', ' ', '\t']) {
        return Err(refused());
    }
    let after_scheme = issuer.split_once("://").map_or("", |(_, rest)| rest);
    let authority = after_scheme.split('/').next().unwrap_or_default();
    if authority.is_empty() || authority.contains('@') {
        return Err(refused());
    }
    Ok(issuer.to_owned())
}

/// Whether a URL may be fetched: `https`, or `http` to a loopback address.
pub fn transport_allowed(url: &str) -> bool {
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    let host = authority.strip_prefix('[').map_or_else(
        || authority.split(':').next().unwrap_or_default(),
        |bracketed| bracketed.split(']').next().unwrap_or_default(),
    );
    match scheme {
        "https" => !host.is_empty(),
        "http" => matches!(host, "localhost" | "127.0.0.1" | "::1"),
        _ => false,
    }
}

fn valid_domain(domain: &str) -> bool {
    domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
}

/// A random value for PKCE: 256 bits, URL-safe, 43 characters.
pub fn random_verifier() -> AppResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(AppError::internal)?;
    Ok(Base64UrlUnpadded::encode_string(&bytes))
}

/// The S256 code challenge for a verifier (RFC 7636).
pub fn pkce_challenge(verifier: &str) -> String {
    Base64UrlUnpadded::encode_string(&Sha256::digest(verifier.as_bytes()))
}

/// The provider's authorization URL for a sign-in.
pub fn authorize_url(
    endpoint: &str,
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    let params = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", "openid profile email"),
        ("state", state),
        ("nonce", nonce),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    let query = form_body(&params);
    let separator = if endpoint.contains('?') { '&' } else { '?' };
    format!("{endpoint}{separator}{query}")
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={}", auth::encode_component(value)))
        .collect::<Vec<_>>()
        .join("&")
}

/// What the discovery document says about the provider's endpoints.
#[derive(Debug, Clone, Deserialize)]
pub struct Discovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    #[serde(default)]
    pub code_challenge_methods_supported: Vec<String>,
}

impl Discovery {
    /// Refuses a document for another issuer, or endpoints that aren't
    /// reached over https.
    fn check(&self, issuer: &str) -> Result<(), &'static str> {
        if self.issuer != issuer {
            return Err("the discovery document names another issuer");
        }
        let endpoints = [
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.jwks_uri,
        ];
        if endpoints.iter().any(|url| !transport_allowed(url)) {
            return Err("an endpoint isn't reached over https");
        }
        if !self.code_challenge_methods_supported.is_empty()
            && !self
                .code_challenge_methods_supported
                .iter()
                .any(|method| method == "S256")
        {
            return Err("the provider doesn't support PKCE with S256");
        }
        Ok(())
    }
}

/// Why a sign-in couldn't be finished. The text is for the server's log,
/// never for the visitor and never containing a token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The provider's answer didn't pass a check.
    Refused(&'static str),
    /// The provider couldn't be reached or answered badly.
    Unavailable(String),
}

/// The provider's keys, by key id.
#[derive(Debug, Clone)]
enum Key {
    /// RSA: modulus and exponent.
    Rsa { n: Vec<u8>, e: Vec<u8> },
    /// A P-256 point, uncompressed.
    P256(Vec<u8>),
}

#[derive(Debug, Clone)]
struct StoredKey {
    alg: Option<String>,
    key: Key,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    alg: Option<String>,
    crv: Option<String>,
    n: Option<String>,
    e: Option<String>,
    x: Option<String>,
    y: Option<String>,
}

fn decode_part(part: Option<&String>) -> Option<Vec<u8>> {
    Base64UrlUnpadded::decode_vec(part?).ok()
}

impl Jwk {
    /// The key, when it's one Sideporch can check a signature with.
    fn stored(self) -> Option<(String, StoredKey)> {
        if self.usage.as_deref().is_some_and(|usage| usage != "sig") {
            return None;
        }
        let key = match self.kty.as_str() {
            "RSA" => Key::Rsa {
                n: decode_part(self.n.as_ref())?,
                e: decode_part(self.e.as_ref())?,
            },
            "EC" if self.crv.as_deref() == Some("P-256") => {
                let x = decode_part(self.x.as_ref())?;
                let y = decode_part(self.y.as_ref())?;
                if x.len() != 32 || y.len() != 32 {
                    return None;
                }
                let mut point = vec![0x04];
                point.extend_from_slice(&x);
                point.extend_from_slice(&y);
                Key::P256(point)
            }
            _ => return None,
        };
        Some((self.kid?, StoredKey { alg: self.alg, key }))
    }
}

/// Parses the keys of a provider's key set, dropping any it can't use.
fn parse_keys(body: &[u8]) -> Result<HashMap<String, StoredKey>, String> {
    let set: KeySet = serde_json::from_slice(body)
        .map_err(|error| format!("the provider's keys are unreadable: {error}"))?;
    Ok(set.keys.into_iter().filter_map(Jwk::stored).collect())
}

#[derive(Clone)]
struct Cached<T> {
    key: String,
    at: Instant,
    value: T,
}

/// Talks to the provider: discovery, keys, and the token exchange. Caches the
/// discovery document and the keys, and asks for keys again at most once a
/// minute when a token names an unknown one.
pub struct Oidc {
    client: HttpClient,
    discovery: RwLock<Option<Cached<Discovery>>>,
    keys: RwLock<Option<Cached<HashMap<String, StoredKey>>>>,
    /// When the keys were last asked for; held while asking, so requests
    /// asking together wait for one answer.
    asked: tokio::sync::Mutex<Option<(String, Instant)>>,
}

impl Oidc {
    /// Sets up the client. Whether an address may be used is decided per
    /// request, by [`transport_allowed`].
    ///
    /// # Errors
    ///
    /// Fails when the TLS roots can't be loaded.
    pub fn new() -> AppResult<Self> {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(AppError::internal)?
            .https_or_http()
            .enable_http1()
            .build();
        Ok(Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            discovery: RwLock::new(None),
            keys: RwLock::new(None),
            asked: tokio::sync::Mutex::new(None),
        })
    }

    /// Sends a request and reads its answer, with one deadline and a size limit.
    async fn send(&self, request: Request<Full<Bytes>>) -> Result<Bytes, Failure> {
        let call = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|error| format!("could not reach the provider: {error}"))?;
            let status = response.status();
            let body = Limited::new(response.into_body(), MAX_DOCUMENT_BYTES)
                .collect()
                .await
                .map_err(|_| "the provider's answer was too large or broke off".to_owned())?
                .to_bytes();
            if !status.is_success() {
                return Err(format!("the provider answered {status}"));
            }
            Ok(body)
        };
        tokio::time::timeout(TIMEOUT, call)
            .await
            .map_err(|_| Failure::Unavailable("the provider did not answer in time".to_owned()))?
            .map_err(Failure::Unavailable)
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, url: &str) -> Result<T, Failure> {
        if !transport_allowed(url) {
            return Err(Failure::Refused("an endpoint isn't reached over https"));
        }
        let request = Request::get(url)
            .header(header::ACCEPT, "application/json")
            .body(Full::default())
            .map_err(|error| Failure::Unavailable(error.to_string()))?;
        let body = self.send(request).await?;
        serde_json::from_slice(&body)
            .map_err(|_| Failure::Unavailable("the provider's answer is unreadable".to_owned()))
    }

    /// The provider's discovery document, for `issuer`, kept for an hour.
    ///
    /// # Errors
    ///
    /// Fails when the document can't be fetched or doesn't match the issuer.
    pub async fn discovery(&self, issuer: &str) -> Result<Discovery, Failure> {
        let cached = self
            .discovery
            .read()
            .ok()
            .and_then(|cache| cache.as_ref().cloned());
        if let Some(cached) =
            cached.filter(|cached| cached.key == issuer && cached.at.elapsed() < DISCOVERY_EVERY)
        {
            return Ok(cached.value);
        }
        let url = format!(
            "{}/.well-known/openid-configuration",
            issuer.trim_end_matches('/')
        );
        let document: Discovery = self.get_json(&url).await?;
        document.check(issuer).map_err(Failure::Refused)?;
        if let Ok(mut cache) = self.discovery.write() {
            *cache = Some(Cached {
                key: issuer.to_owned(),
                at: Instant::now(),
                value: document.clone(),
            });
        }
        Ok(document)
    }

    /// The fresh key `kid` names, if it's cached from `jwks_uri`. Another
    /// provider can use the same key id for a different key.
    fn known_key(&self, jwks_uri: &str, kid: &str) -> Option<StoredKey> {
        self.keys.read().ok().and_then(|cache| {
            let cached = cache.as_ref()?;
            if cached.key != jwks_uri || cached.at.elapsed() >= REFRESH_EVERY {
                return None;
            }
            cached.value.get(kid).cloned()
        })
    }

    /// The key `kid` names, asking the provider when it isn't cached.
    async fn key(&self, jwks_uri: &str, kid: &str) -> Result<StoredKey, Failure> {
        if let Some(key) = self.known_key(jwks_uri, kid) {
            return Ok(key);
        }
        let mut asked = self.asked.lock().await;
        if let Some(key) = self.known_key(jwks_uri, kid) {
            return Ok(key);
        }
        if asked
            .as_ref()
            .is_some_and(|(uri, at)| uri == jwks_uri && at.elapsed() < REFETCH_EVERY)
        {
            return Err(Failure::Refused("signed with an unknown key"));
        }
        *asked = Some((jwks_uri.to_owned(), Instant::now()));
        if !transport_allowed(jwks_uri) {
            return Err(Failure::Refused("an endpoint isn't reached over https"));
        }
        let request = Request::get(jwks_uri)
            .body(Full::default())
            .map_err(|error| Failure::Unavailable(error.to_string()))?;
        let body = self.send(request).await?;
        let keys = parse_keys(&body).map_err(Failure::Unavailable)?;
        if let Ok(mut cache) = self.keys.write() {
            *cache = Some(Cached {
                key: jwks_uri.to_owned(),
                at: Instant::now(),
                value: keys,
            });
        }
        drop(asked);
        self.known_key(jwks_uri, kid)
            .ok_or(Failure::Refused("signed with an unknown key"))
    }

    /// Exchanges the code for the provider's ID token, proving the PKCE
    /// verifier and the client's secret.
    ///
    /// # Errors
    ///
    /// Fails when the provider refuses the exchange or answers without a token.
    pub async fn exchange(
        &self,
        discovery: &Discovery,
        provider: &Provider,
        code: &str,
        verifier: &str,
        redirect_uri: &str,
    ) -> Result<String, Failure> {
        let body = form_body(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("code_verifier", verifier),
        ]);
        let credentials = format!(
            "{}:{}",
            auth::encode_component(&provider.client_id),
            auth::encode_component(&provider.client_secret)
        );
        let request = Request::post(&discovery.token_endpoint)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(
                header::AUTHORIZATION,
                format!("Basic {}", Base64::encode_string(credentials.as_bytes())),
            )
            .header(header::ACCEPT, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|error| Failure::Unavailable(error.to_string()))?;
        if !transport_allowed(&discovery.token_endpoint) {
            return Err(Failure::Refused("an endpoint isn't reached over https"));
        }
        let answer = self.send(request).await?;
        if answer.len() > MAX_TOKEN_BYTES {
            return Err(Failure::Unavailable(
                "the token answer is too large".to_owned(),
            ));
        }
        let answer: TokenAnswer = serde_json::from_slice(&answer)
            .map_err(|_| Failure::Unavailable("the token answer is unreadable".to_owned()))?;
        answer
            .id_token
            .ok_or(Failure::Refused("the token answer has no ID token"))
    }

    /// Checks an ID token from the provider: its signature with the key it
    /// names, then its claims.
    ///
    /// # Errors
    ///
    /// Fails with the first check the token doesn't pass.
    pub async fn verify(
        &self,
        discovery: &Discovery,
        token: &str,
        expect: &Expect<'_>,
        now: i64,
    ) -> Result<Identity, Failure> {
        let parsed = parse(token).map_err(Failure::Refused)?;
        let key = self.key(&discovery.jwks_uri, &parsed.kid).await?;
        check_signature(&parsed, &key)?;
        check_claims(parsed.claims, expect, now).map_err(Failure::Refused)
    }
}

/// The token endpoint's answer; only the ID token is read.
#[derive(Deserialize)]
struct TokenAnswer {
    id_token: Option<String>,
}

/// The parts of a compact JWS, once its header allows checking it.
struct Parsed {
    alg: &'static str,
    kid: String,
    signed: String,
    signature: Vec<u8>,
    claims: Claims,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
    crit: Option<Value>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: Option<String>,
    sub: Option<String>,
    aud: Option<Audience>,
    azp: Option<String>,
    exp: Option<i64>,
    iat: Option<i64>,
    nbf: Option<i64>,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<Value>,
    preferred_username: Option<String>,
    name: Option<String>,
}

/// Splits a token and reads its header and claims. Only RS256 and ES256 pass;
/// `none`, HS256 and every other algorithm are refused before anything else
/// is looked at.
fn parse(token: &str) -> Result<Parsed, &'static str> {
    if token.len() > MAX_ID_TOKEN_BYTES {
        return Err("the ID token is too long");
    }
    let (signed, signature) = token.rsplit_once('.').ok_or("not a JWT")?;
    let (header, payload) = signed.split_once('.').ok_or("not a JWT")?;
    if payload.contains('.') {
        return Err("not a JWT");
    }
    let header: Header = serde_json::from_slice(
        &Base64UrlUnpadded::decode_vec(header).map_err(|_| "not base64url")?,
    )
    .map_err(|_| "unreadable header")?;
    let alg = match header.alg.as_str() {
        "RS256" => "RS256",
        "ES256" => "ES256",
        _ => return Err("not signed with RS256 or ES256"),
    };
    if header.crit.is_some() {
        return Err("the header has critical extensions");
    }
    let kid = header
        .kid
        .filter(|kid| !kid.is_empty())
        .ok_or("no key id")?;
    let claims: Claims = serde_json::from_slice(
        &Base64UrlUnpadded::decode_vec(payload).map_err(|_| "not base64url")?,
    )
    .map_err(|_| "unreadable claims")?;
    Ok(Parsed {
        alg,
        kid,
        signed: signed.to_owned(),
        signature: Base64UrlUnpadded::decode_vec(signature).map_err(|_| "not base64url")?,
        claims,
    })
}

fn check_signature(parsed: &Parsed, stored: &StoredKey) -> Result<(), Failure> {
    if stored.alg.as_deref().is_some_and(|alg| alg != parsed.alg) {
        return Err(Failure::Refused("the key is for another algorithm"));
    }
    let message = parsed.signed.as_bytes();
    let checked = match (parsed.alg, &stored.key) {
        ("RS256", Key::Rsa { n, e }) => RsaPublicKeyComponents {
            n: n.as_slice(),
            e: e.as_slice(),
        }
        .verify(&RSA_PKCS1_2048_8192_SHA256, message, &parsed.signature),
        ("ES256", Key::P256(point)) => UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
            .verify(message, &parsed.signature),
        _ => return Err(Failure::Refused("the key is for another algorithm")),
    };
    checked.map_err(|_| Failure::Refused("bad signature"))
}

/// What a token must match: who issued it, for which client, the nonce this
/// sign-in started with, and the email domain if the admin limited it.
#[derive(Debug, Clone, Copy)]
pub struct Expect<'a> {
    pub issuer: &'a str,
    pub client_id: &'a str,
    pub nonce: &'a str,
    pub domain: Option<&'a str>,
}

/// Who a verified token is about. The subject is what identifies them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    /// The email claim, lowercased, when the provider gave one.
    pub email: Option<String>,
    pub preferred_username: Option<String>,
    pub name: Option<String>,
}

fn verified(value: Option<&Value>) -> bool {
    match value {
        Some(Value::Bool(flag)) => *flag,
        Some(Value::String(text)) => text == "true",
        _ => false,
    }
}

/// Checks a verified token's claims at `now`, in seconds since 1970.
fn check_claims(claims: Claims, expect: &Expect<'_>, now: i64) -> Result<Identity, &'static str> {
    if claims.iss.as_deref() != Some(expect.issuer) {
        return Err("issued by another issuer");
    }
    let audiences = match &claims.aud {
        Some(Audience::One(aud)) => vec![aud.as_str()],
        Some(Audience::Many(auds)) => auds.iter().map(String::as_str).collect(),
        None => return Err("no audience"),
    };
    if !audiences.contains(&expect.client_id) {
        return Err("for another client");
    }
    if (audiences.len() > 1 || claims.azp.is_some())
        && claims.azp.as_deref() != Some(expect.client_id)
    {
        return Err("the authorized party is another client");
    }
    let exp = claims.exp.ok_or("no expiry")?;
    if exp.saturating_add(SKEW_SECONDS) <= now {
        return Err("expired");
    }
    let iat = claims.iat.ok_or("no issue time")?;
    if iat.saturating_sub(SKEW_SECONDS) > now {
        return Err("issued in the future");
    }
    if claims
        .nbf
        .is_some_and(|nbf| nbf.saturating_sub(SKEW_SECONDS) > now)
    {
        return Err("not valid yet");
    }
    let nonce = claims.nonce.ok_or("no nonce")?;
    if !auth::same_bytes(nonce.as_bytes(), expect.nonce.as_bytes()) {
        return Err("the nonce doesn't match this sign-in");
    }
    let subject = claims
        .sub
        .filter(|sub| !sub.is_empty() && sub.len() <= 255)
        .ok_or("no subject")?;
    let email = claims
        .email
        .map(|email| email.trim().to_lowercase())
        .filter(|email| !email.is_empty());
    if let Some(domain) = expect.domain {
        let in_domain = email
            .as_deref()
            .and_then(|email| email.rsplit_once('@'))
            .is_some_and(|(_, host)| host == domain);
        if !in_domain || !verified(claims.email_verified.as_ref()) {
            return Err("the email address isn't a verified one in the allowed domain");
        }
    }
    Ok(Identity {
        subject,
        email,
        preferred_username: claims.preferred_username,
        name: claims.name,
    })
}

/// The username a provider's `preferred_username` suggests: lowercase
/// letters, numbers, dots, dashes and underscores; an address keeps its name
/// before the `@`. `None` when nothing usable is left.
pub fn username_from(preferred: &str) -> Option<String> {
    let name = preferred.trim().to_lowercase();
    let name = name.split('@').next().unwrap_or_default();
    let mut username: String = name
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'))
        .take(32)
        .collect();
    while username.starts_with(['.', '-', '_']) {
        username.remove(0);
    }
    (username.chars().count() >= 2).then_some(username)
}

/// The username to create, with `-2`, `-3`… after it while taken, so it
/// stays within 32 characters.
pub fn free_username(base: &str, taken: impl Fn(&str) -> bool) -> Option<String> {
    if !taken(base) {
        return Some(base.to_owned());
    }
    (2..100)
        .map(|n| {
            let suffix = format!("-{n}");
            let stem: String = base
                .chars()
                .take(32_usize.saturating_sub(suffix.len()))
                .collect();
            format!("{stem}{suffix}")
        })
        .find(|candidate| !taken(candidate))
}

/// Whether `user_id` has a sign-in with `issuer` linked.
pub fn linked(conn: &Connection, issuer: &str, user_id: i64) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM oidc_identities WHERE issuer = ?1 AND user_id = ?2)",
        rusqlite::params![issuer, user_id],
        |row| row.get(0),
    )?)
}

/// Whether a stored identity is linked to a user. Rows are looked up by
/// issuer and subject only.
pub fn identity_owner(conn: &Connection, issuer: &str, subject: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT user_id FROM oidc_identities WHERE issuer = ?1 AND subject = ?2",
            [issuer, subject],
            |row| row.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::arithmetic_side_effects,
        clippy::unwrap_used,
        clippy::expect_used
    )]

    use super::*;

    #[test]
    fn a_cached_key_only_answers_for_the_provider_it_came_from() {
        let oidc = Oidc::new().unwrap();
        let key = StoredKey {
            alg: None,
            key: Key::P256(vec![4]),
        };
        *oidc.keys.write().unwrap() = Some(Cached {
            key: "https://one.example/keys".to_owned(),
            at: Instant::now(),
            value: HashMap::from([("shared-id".to_owned(), key)]),
        });
        assert!(
            oidc.known_key("https://one.example/keys", "shared-id")
                .is_some()
        );
        assert!(
            oidc.known_key("https://two.example/keys", "shared-id")
                .is_none()
        );
    }

    fn token(header: &Value, claims: &Value) -> String {
        format!(
            "{}.{}.c2ln",
            Base64UrlUnpadded::encode_string(header.to_string().as_bytes()),
            Base64UrlUnpadded::encode_string(claims.to_string().as_bytes())
        )
    }

    #[test]
    fn only_rs256_and_es256_with_a_key_id_are_read() {
        let claims = serde_json::json!({ "iss": "x" });
        for header in [
            serde_json::json!({ "alg": "none", "kid": "a" }),
            serde_json::json!({ "alg": "HS256", "kid": "a" }),
            serde_json::json!({ "alg": "RS512", "kid": "a" }),
            serde_json::json!({ "alg": "RS256" }),
            serde_json::json!({ "alg": "RS256", "kid": "a", "crit": ["x"] }),
        ] {
            assert!(parse(&token(&header, &claims)).is_err(), "{header}");
        }
        for alg in ["RS256", "ES256"] {
            let parsed = parse(&token(
                &serde_json::json!({ "alg": alg, "kid": "a" }),
                &serde_json::json!({
                    "iss": "x", "aud": "c", "exp": 1, "iat": 1, "nonce": "n", "sub": "s"
                }),
            ));
            assert_eq!(parsed.map(|parsed| parsed.alg).ok(), Some(alg));
        }
        assert!(parse("a.b").is_err());
        assert!(parse("a.b.c.d").is_err());
    }

    #[test]
    fn issuers_must_be_https_or_loopback() {
        assert_eq!(
            parse_issuer(" https://login.example.com/realms/team ").as_deref(),
            Ok("https://login.example.com/realms/team")
        );
        assert!(parse_issuer("http://login.example.com").is_err());
        assert!(parse_issuer("http://127.0.0.1:9000").is_ok());
        assert!(parse_issuer("http://localhost").is_ok());
        assert!(parse_issuer("https://user@login.example.com").is_err());
        assert!(parse_issuer("https://login.example.com/?x=1").is_err());
        assert!(parse_issuer("https://").is_err());
        assert!(parse_issuer("login.example.com").is_err());
    }

    #[test]
    fn pkce_matches_the_rfc_example() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn usernames_come_from_the_preferred_one() {
        assert_eq!(
            username_from(" Ada.Lovelace ").as_deref(),
            Some("ada.lovelace")
        );
        assert_eq!(username_from("ada@example.com").as_deref(), Some("ada"));
        assert_eq!(username_from("ü").as_deref(), None);
        assert_eq!(username_from("-x"), None);
        assert_eq!(
            free_username("ada", |name| name == "ada"),
            Some("ada-2".to_owned())
        );
        assert_eq!(free_username("ada", |_| false), Some("ada".to_owned()));
        let long = "a".repeat(32);
        let taken = format!("{}-2", "a".repeat(30));
        assert_eq!(
            free_username(&long, |name| name == long || name == taken).map(|name| name.len()),
            Some(32)
        );
    }

    fn claims(now: i64) -> Value {
        serde_json::json!({
            "iss": "https://issuer.example",
            "aud": ["client", "other"],
            "azp": "client",
            "sub": "subject-1",
            "exp": now + 300,
            "iat": now,
            "nonce": "n-1",
            "email": "Ada@Example.com",
            "email_verified": true,
        })
    }

    fn check(
        claims_json: &Value,
        domain: Option<&str>,
        now: i64,
    ) -> Result<Identity, &'static str> {
        let claims: Claims = serde_json::from_value(claims_json.clone()).unwrap();
        check_claims(
            claims,
            &Expect {
                issuer: "https://issuer.example",
                client_id: "client",
                nonce: "n-1",
                domain,
            },
            now,
        )
    }

    #[test]
    fn claims_are_checked() {
        let now = 1_700_000_000;
        let ok = check(&claims(now), None, now).unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(ok.subject, "subject-1");
        assert_eq!(ok.email.as_deref(), Some("ada@example.com"));
        let changed = |key: &str, value: Value| {
            let mut json = claims(now);
            json[key] = value;
            check(&json, None, now)
        };
        assert!(changed("iss", "https://evil.example".into()).is_err());
        assert!(changed("aud", "other".into()).is_err());
        assert!(changed("nonce", "n-2".into()).is_err());
        assert!(changed("azp", "other".into()).is_err());
        assert!(changed("exp", (now - 61).into()).is_err());
        assert!(changed("iat", (now + 61).into()).is_err());
        assert!(changed("sub", "".into()).is_err());
        let mut no_nonce = claims(now);
        no_nonce
            .as_object_mut()
            .map(|object| object.remove("nonce"));
        assert!(check(&no_nonce, None, now).is_err());
        // Within the clock skew, the token is still good.
        assert!(check(&claims(now), None, now + 120).is_ok());
    }

    #[test]
    fn a_domain_needs_a_verified_address_in_it() {
        let now = 1_700_000_000;
        assert!(check(&claims(now), Some("example.com"), now).is_ok());
        assert!(check(&claims(now), Some("other.com"), now).is_err());
        let mut unverified = claims(now);
        unverified["email_verified"] = false.into();
        assert!(check(&unverified, Some("example.com"), now).is_err());
        assert!(check(&unverified, None, now).is_ok());
    }
}
