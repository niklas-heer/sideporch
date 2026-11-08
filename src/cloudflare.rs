//! Signing in through Cloudflare Access.
//!
//! Cloudflare Access is a proxy in front of the server that signs people in
//! with the team's identity provider. Every request it lets through carries
//! `Cf-Access-Jwt-Assertion`, a JWT signed with RS256 by the team's keys,
//! which Cloudflare publishes at `https://<team>/cdn-cgi/access/certs`.
//!
//! Sideporch checks the assertion only where someone signs in: `/login`
//! (where every page without a session leads) and first-run setup. An
//! assertion counts as the first step of signing in, so an account's second
//! step and the passkey policy still apply. Webhooks, federation and API
//! tokens never look at it. With [`CloudflareAccess::required`], pages also
//! refuse requests that didn't come through Access, so nobody can go around
//! it by reaching the server directly.
//!
//! When Cloudflare can't be reached for its keys, Access sign-in fails and
//! everything else keeps working.

use std::{
    collections::HashMap,
    sync::RwLock,
    time::{Duration, Instant},
};

use axum::{
    Form, Router,
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use http_body_util::{BodyExt as _, Empty, Limited};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{Client, connect::HttpConnector},
    rt::TokioExecutor,
};
use maud::{Markup, html};
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use rusqlite::{Connection, OptionalExtension as _};
use serde::Deserialize;

use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    security::{self, Policy, Requirement},
    views,
};

/// The header Cloudflare Access adds to requests it lets through.
pub const HEADER: &str = "cf-access-jwt-assertion";
/// Where Cloudflare Access signs people out, on every protected host.
pub const LOGOUT: &str = "/cdn-cgi/access/logout";
/// How far clocks may disagree.
const SKEW_SECONDS: i64 = 60;
/// How often unknown key ids may make Sideporch ask Cloudflare again.
const REFETCH_EVERY: Duration = Duration::from_mins(1);
/// How often the keys are refreshed anyway, to follow Cloudflare's rotation.
const REFRESH_EVERY: Duration = Duration::from_hours(1);
const TIMEOUT: Duration = Duration::from_secs(10);
const MAX_KEYS_BYTES: usize = 64 * 1024;
const MAX_TOKEN_BYTES: usize = 16 * 1024;

type HttpClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;

/// Turns what the operator wrote into the team's host name: `myteam`,
/// `myteam.cloudflareaccess.com` and `https://myteam.cloudflareaccess.com/`
/// all mean `myteam.cloudflareaccess.com`.
///
/// # Errors
///
/// Says what's wrong with a value that isn't a host name.
pub fn team_domain(input: &str) -> Result<String, String> {
    let trimmed = input.trim().to_lowercase();
    let host = trimmed
        .strip_prefix("https://")
        .unwrap_or(&trimmed)
        .trim_end_matches('/');
    let valid = !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        });
    if !valid {
        return Err(format!(
            "the Cloudflare Access team domain {input:?} isn't a host name like myteam.cloudflareaccess.com"
        ));
    }
    Ok(if host.contains('.') {
        host.to_owned()
    } else {
        format!("{host}.cloudflareaccess.com")
    })
}

/// What a valid assertion says about who is visiting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Assertion {
    /// A person the identity provider signed in, by their address.
    Person { email: String },
    /// A service token: a program, which never signs into an account.
    Service,
}

/// Why an assertion wasn't accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    /// The request didn't carry one.
    Missing,
    /// It isn't a valid assertion for this application.
    Invalid(&'static str),
    /// Cloudflare's keys couldn't be fetched to check it.
    Unavailable,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    aud: Audience,
    exp: i64,
    nbf: Option<i64>,
    iat: Option<i64>,
    #[serde(rename = "type")]
    kind: Option<String>,
    email: Option<String>,
    common_name: Option<String>,
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Jwk>,
}

#[derive(Deserialize)]
struct Jwk {
    kty: String,
    kid: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

/// A public key's modulus and exponent.
type RsaKey = (Vec<u8>, Vec<u8>);

/// The parts of a token, once its header allows checking it.
struct Parsed<'a> {
    kid: String,
    signed: &'a str,
    signature: Vec<u8>,
    claims: Claims,
}

fn decode(part: &str) -> Result<Vec<u8>, Failure> {
    Base64UrlUnpadded::decode_vec(part).map_err(|_| Failure::Invalid("not base64url"))
}

/// Splits a token and reads its header and claims, refusing every
/// algorithm but RS256 before anything else happens.
fn parse(token: &str) -> Result<Parsed<'_>, Failure> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err(Failure::Invalid("too long"));
    }
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(Failure::Invalid("not a JWT"));
    };
    let header: Header = serde_json::from_slice(&decode(header)?)
        .map_err(|_| Failure::Invalid("unreadable header"))?;
    if header.alg != "RS256" {
        return Err(Failure::Invalid("not signed with RS256"));
    }
    let kid = header
        .kid
        .filter(|kid| !kid.is_empty())
        .ok_or(Failure::Invalid("no key id"))?;
    let claims: Claims = serde_json::from_slice(&decode(payload)?)
        .map_err(|_| Failure::Invalid("unreadable claims"))?;
    let signed_len = token
        .len()
        .saturating_sub(signature.len())
        .saturating_sub(1);
    let signed = token
        .get(..signed_len)
        .ok_or(Failure::Invalid("not a JWT"))?;
    Ok(Parsed {
        kid,
        signed,
        signature: decode(signature)?,
        claims,
    })
}

/// Checks who issued the claims, for whom, and when, and says who they are
/// about.
fn check_claims(
    claims: Claims,
    team: &str,
    audience: &str,
    now: i64,
) -> Result<Assertion, Failure> {
    if claims.iss != format!("https://{team}") {
        return Err(Failure::Invalid("issued by another team"));
    }
    let for_us = match &claims.aud {
        Audience::One(aud) => aud == audience,
        Audience::Many(auds) => auds.iter().any(|aud| aud == audience),
    };
    if !for_us {
        return Err(Failure::Invalid("for another application"));
    }
    if claims.exp.saturating_add(SKEW_SECONDS) <= now {
        return Err(Failure::Invalid("expired"));
    }
    let early = |at: Option<i64>| at.is_some_and(|at| at.saturating_sub(SKEW_SECONDS) > now);
    if early(claims.nbf) || early(claims.iat) {
        return Err(Failure::Invalid("not valid yet"));
    }
    if claims.kind.as_deref().is_some_and(|kind| kind != "app") {
        return Err(Failure::Invalid("not an application token"));
    }
    if claims
        .common_name
        .as_deref()
        .is_some_and(|name| !name.is_empty())
    {
        return Ok(Assertion::Service);
    }
    let email = claims
        .email
        .map(|email| email.trim().to_lowercase())
        .filter(|email| !email.is_empty())
        .ok_or(Failure::Invalid("no email address"))?;
    Ok(Assertion::Person { email })
}

/// The keys Cloudflare signs with, and when they were last asked for.
#[derive(Default)]
struct Keys {
    by_id: HashMap<String, RsaKey>,
}

/// Checks Cloudflare Access assertions for one application.
pub struct CloudflareAccess {
    /// The team's host, such as `myteam.cloudflareaccess.com`.
    team: String,
    /// The application's audience tag.
    audience: String,
    /// Refuse pages to requests without a valid assertion.
    pub required: bool,
    certs_url: String,
    client: HttpClient,
    keys: RwLock<Keys>,
    /// When the keys were last asked for; held while asking, so one request
    /// asks and the others wait for its answer.
    fetched: tokio::sync::Mutex<Option<Instant>>,
}

impl CloudflareAccess {
    /// Sets up checking for the team and audience tag the operator gave.
    /// `certs_url` replaces Cloudflare's key address in tests.
    ///
    /// # Errors
    ///
    /// Fails when the team domain or audience tag is unusable.
    pub fn new(
        team: &str,
        audience: &str,
        required: bool,
        certs_url: Option<String>,
    ) -> AppResult<Self> {
        let team = team_domain(team).map_err(AppError::Internal)?;
        let audience = audience.trim().to_owned();
        if audience.is_empty() {
            return Err(AppError::Internal(
                "the Cloudflare Access audience (AUD) tag is empty".to_owned(),
            ));
        }
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_provider_and_webpki_roots(rustls::crypto::ring::default_provider())
            .map_err(AppError::internal)?;
        let connector = if certs_url.is_some() {
            connector.https_or_http()
        } else {
            connector.https_only()
        }
        .enable_http1()
        .enable_http2()
        .build();
        Ok(Self {
            certs_url: certs_url.unwrap_or_else(|| format!("https://{team}/cdn-cgi/access/certs")),
            team,
            audience,
            required,
            client: Client::builder(TokioExecutor::new()).build(connector),
            keys: RwLock::default(),
            fetched: tokio::sync::Mutex::new(None),
        })
    }

    fn known_key(&self, kid: &str) -> Option<RsaKey> {
        self.keys
            .read()
            .ok()
            .and_then(|keys| keys.by_id.get(kid).cloned())
    }

    async fn fetch_keys(&self) -> Result<HashMap<String, RsaKey>, String> {
        let request = axum::http::Request::get(&self.certs_url)
            .body(Empty::new())
            .map_err(|error| error.to_string())?;
        let response = tokio::time::timeout(TIMEOUT, self.client.request(request))
            .await
            .map_err(|_| "Cloudflare did not answer in time".to_owned())?
            .map_err(|error| format!("could not reach Cloudflare: {error}"))?;
        let status = response.status();
        let body = Limited::new(response.into_body(), MAX_KEYS_BYTES)
            .collect()
            .await
            .map_err(|_| "Cloudflare's answer was too large".to_owned())?
            .to_bytes();
        if !status.is_success() {
            return Err(format!("Cloudflare answered {status}"));
        }
        let set: KeySet = serde_json::from_slice(&body)
            .map_err(|error| format!("Cloudflare's keys are unreadable: {error}"))?;
        let keys: HashMap<String, RsaKey> = set
            .keys
            .into_iter()
            .filter(|key| key.kty == "RSA")
            .filter_map(|key| {
                let n = Base64UrlUnpadded::decode_vec(key.n.as_deref()?).ok()?;
                let e = Base64UrlUnpadded::decode_vec(key.e.as_deref()?).ok()?;
                Some((key.kid?, (n, e)))
            })
            .collect();
        if keys.is_empty() {
            return Err("Cloudflare listed no RSA keys".to_owned());
        }
        Ok(keys)
    }

    /// Asks Cloudflare for its keys and keeps them. When `only_if_missing`
    /// names a key that arrived in the meantime, nothing is asked.
    async fn refresh(&self, only_if_missing: Option<&str>) -> Result<(), String> {
        let mut fetched = self.fetched.lock().await;
        if let Some(kid) = only_if_missing {
            if self.known_key(kid).is_some() {
                return Ok(());
            }
            if fetched.is_some_and(|at| at.elapsed() < REFETCH_EVERY) {
                return Err("asked Cloudflare less than a minute ago".to_owned());
            }
        }
        *fetched = Some(Instant::now());
        let keys = self.fetch_keys().await?;
        if let Ok(mut known) = self.keys.write() {
            known.by_id = keys;
        }
        // Others waiting for keys go on once these are in.
        drop(fetched);
        Ok(())
    }

    /// The key `kid` names, asking Cloudflare when it's new to us.
    async fn key(&self, kid: &str) -> Result<RsaKey, Failure> {
        if let Some(key) = self.known_key(kid) {
            return Ok(key);
        }
        let had_keys = self.keys.read().is_ok_and(|keys| !keys.by_id.is_empty());
        if let Err(error) = self.refresh(Some(kid)).await {
            tracing::warn!(%error, "could not fetch Cloudflare Access keys");
            return Err(if had_keys {
                Failure::Invalid("signed with an unknown key")
            } else {
                Failure::Unavailable
            });
        }
        self.known_key(kid)
            .ok_or(Failure::Invalid("signed with an unknown key"))
    }

    /// Checks an assertion at `now`, in seconds since 1970.
    pub async fn verify(&self, token: &str, now: i64) -> Result<Assertion, Failure> {
        let parsed = parse(token.trim())?;
        let (n, e) = self.key(&parsed.kid).await?;
        RsaPublicKeyComponents { n, e }
            .verify(
                &RSA_PKCS1_2048_8192_SHA256,
                parsed.signed.as_bytes(),
                &parsed.signature,
            )
            .map_err(|_| Failure::Invalid("bad signature"))?;
        check_claims(parsed.claims, &self.team, &self.audience, now)
    }

    /// Checks the assertion a request carries.
    pub async fn check_request(&self, headers: &HeaderMap) -> Result<Assertion, Failure> {
        let token = headers
            .get(HEADER)
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or(Failure::Missing)?;
        self.verify(token, jiff::Timestamp::now().as_second()).await
    }
}

/// Keeps Cloudflare's keys fresh: once at start, then hourly, sooner again
/// after a failure.
pub fn start(state: &AppState) {
    let Some(access) = state.cloudflare.clone() else {
        return;
    };
    tokio::spawn(async move {
        loop {
            let wait = match access.refresh(None).await {
                Ok(()) => REFRESH_EVERY,
                Err(error) => {
                    tracing::warn!(%error, "could not fetch Cloudflare Access keys");
                    Duration::from_mins(5)
                }
            };
            tokio::time::sleep(wait).await;
        }
    });
}

/// What the request's assertion says, when Access is set up.
async fn assertion(state: &AppState, headers: &HeaderMap) -> Option<Result<Assertion, Failure>> {
    Some(state.cloudflare.as_ref()?.check_request(headers).await)
}

/// The local, active account that confirmed `email`.
fn account_for(conn: &Connection, email: &str) -> AppResult<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM users
             WHERE lower(email) = ?1 AND instance_id IS NULL AND deactivated_at IS NULL",
            [email],
            |row| row.get(0),
        )
        .optional()?)
}

/// What `/login` does about Cloudflare Access.
pub enum AtLogin {
    /// Nothing: Access isn't set up, or the request has no assertion.
    Nothing,
    /// Sign in as this account; its second step still applies.
    SignIn(i64),
    /// Show the sign-in form with this explanation, and go to `next`
    /// afterwards if nothing else was asked for.
    Explain {
        notice: String,
        next: Option<&'static str>,
    },
}

/// Decides whether the visitor's Access assertion signs them in.
pub async fn at_login(state: &AppState, headers: &HeaderMap) -> AppResult<AtLogin> {
    let email = match assertion(state, headers).await {
        None | Some(Ok(Assertion::Service) | Err(Failure::Missing)) => {
            return Ok(AtLogin::Nothing);
        }
        Some(Err(Failure::Unavailable)) => {
            return Ok(AtLogin::Explain {
                notice: "Sideporch couldn't reach Cloudflare to check your Access sign-in, so sign in below.".to_owned(),
                next: None,
            });
        }
        Some(Err(Failure::Invalid(reason))) => {
            tracing::warn!(reason, "refused a Cloudflare Access assertion");
            return Ok(AtLogin::Explain {
                notice:
                    "Your Cloudflare Access sign-in couldn't be verified here, so sign in below."
                        .to_owned(),
                next: None,
            });
        }
        Some(Ok(Assertion::Person { email })) => email,
    };
    let lookup = email.clone();
    let found = state
        .db
        .call(move |conn| {
            let Some(user_id) = account_for(conn, &lookup)? else {
                return Ok(None);
            };
            // With passkeys required, Access works like a password: only
            // until the account has a passkey.
            let passkey_only = Policy::load(conn)?.require == Requirement::Passkeys
                && security::factors(conn, user_id)?.passkeys > 0;
            Ok(Some((user_id, passkey_only)))
        })
        .await?;
    Ok(match found {
        Some((_, true)) => AtLogin::Explain {
            notice: "This Sideporch signs in with passkeys. Use the passkey button below."
                .to_owned(),
            next: None,
        },
        Some((user_id, false)) => AtLogin::SignIn(user_id),
        None => AtLogin::Explain {
            notice: format!(
                "Cloudflare Access signed you in as {email}, but no account here uses that address. If you have an account, sign in below to link the address to it."
            ),
            next: Some("/settings/cloudflare-access"),
        },
    })
}

/// Before the first account exists: the Access address of whoever may set
/// up the server, `None` when Access isn't set up, or the page that refuses
/// them.
pub async fn setup_gate(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<String>, Box<Response>> {
    let refuse = |status: StatusCode, message: &str| {
        Box::new((status, views::error_page(status, message)).into_response())
    };
    match assertion(state, headers).await {
        None => Ok(None),
        Some(Ok(Assertion::Person { email })) => Ok(Some(email)),
        Some(Err(Failure::Unavailable)) => Err(refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "Sideporch couldn't reach Cloudflare to check your Access sign-in. Try again in a moment.",
        )),
        Some(failure) => {
            if let Err(Failure::Invalid(reason)) = failure {
                tracing::warn!(reason, "refused a Cloudflare Access assertion at setup");
            }
            Err(refuse(
                StatusCode::FORBIDDEN,
                "This server signs people in through Cloudflare Access, so setup does too. Open it at the address Cloudflare Access protects and sign in there as a person, not with a service token.",
            ))
        }
    }
}

/// Paths that stay reachable without Access in required mode: the health
/// check, files the browser loads for any page, and what programs and
/// other servers call with their own credentials.
fn open_without_access(path: &str) -> bool {
    const PATHS: &[&str] = &[
        "/healthz",
        "/mcp",
        "/sw.js",
        "/manifest.webmanifest",
        "/favicon.ico",
        "/apple-touch-icon.png",
        "/offline",
        "/.well-known/sideporch",
    ];
    const PREFIXES: &[&str] = &["/assets/", "/hooks/", "/federation/"];
    PATHS.contains(&path) || PREFIXES.iter().any(|prefix| path.starts_with(prefix))
}

/// In required mode, refuses pages to requests that didn't come through
/// Cloudflare Access. A service token's assertion passes too: Access let it
/// in, though it never signs into an account.
pub async fn require(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(access) = state.cloudflare.as_ref().filter(|access| access.required) else {
        return next.run(request).await;
    };
    if open_without_access(request.uri().path()) {
        return next.run(request).await;
    }
    match access.check_request(request.headers()).await {
        Ok(_) => next.run(request).await,
        Err(Failure::Unavailable) => (
            StatusCode::SERVICE_UNAVAILABLE,
            views::error_page(
                StatusCode::SERVICE_UNAVAILABLE,
                "Sideporch couldn't reach Cloudflare to check your Access sign-in. Try again in a moment.",
            ),
        )
            .into_response(),
        Err(_) => (
            StatusCode::FORBIDDEN,
            views::error_page(
                StatusCode::FORBIDDEN,
                "This server is only open through Cloudflare Access. Open it at its usual address.",
            ),
        )
            .into_response(),
    }
}

pub fn router() -> Router<AppState> {
    Router::new().route("/settings/cloudflare-access", get(link_page).post(link))
}

/// What the page that links an Access address to an account can do.
enum Linking {
    /// Nothing to link: Access isn't set up or vouches for nobody.
    Nothing,
    AlreadyLinked(String),
    /// Another account uses the address.
    Taken(String),
    Banned(String),
    Ready(String),
}

async fn linking(state: &AppState, user: &CurrentUser, headers: &HeaderMap) -> AppResult<Linking> {
    let Some(Ok(Assertion::Person { email })) = assertion(state, headers).await else {
        return Ok(Linking::Nothing);
    };
    if state.access.email_banned(&email) {
        return Ok(Linking::Banned(email));
    }
    let (user_id, lookup) = (user.id, email.clone());
    let owner = state
        .db
        .call(move |conn| account_for(conn, &lookup))
        .await?;
    Ok(match owner {
        Some(owner) if owner == user_id => Linking::AlreadyLinked(email),
        Some(_) => Linking::Taken(email),
        None => Linking::Ready(email),
    })
}

fn link_view(linking: &Linking) -> Markup {
    views::auth_page(
        "Cloudflare Access",
        &html! {
            h1 class="mb-2 text-xl font-bold" { "Sign in with Cloudflare Access" }
            @match linking {
                Linking::Nothing => {
                    p class="mb-5 text-muted dark:text-haint" {
                        "Open Sideporch through Cloudflare Access to link the address it signs you in with."
                    }
                }
                Linking::AlreadyLinked(email) => {
                    p class="mb-5 text-muted dark:text-haint" {
                        "Cloudflare Access already signs you in to this account as " strong { (email) } "."
                    }
                }
                Linking::Taken(email) => {
                    p class="mb-5 text-muted dark:text-haint" {
                        "Another account here uses " strong { (email) } ", so Cloudflare Access signs in to that one."
                    }
                }
                Linking::Banned(email) => {
                    p class="mb-5 text-muted dark:text-haint" {
                        (email) " can't be used here."
                    }
                }
                Linking::Ready(email) => {
                    p class="mb-5 text-muted dark:text-haint" {
                        "Cloudflare Access signed you in as " strong { (email) } ". Use this address for your account, and next time Cloudflare Access signs you in here directly. It also becomes your account's email address."
                    }
                    form method="post" action="/settings/cloudflare-access" {
                        input type="hidden" name="email" value=(email);
                        button type="submit" class="btn w-full" { "Use " (email) }
                    }
                }
            }
            a href="/" class="btn-quiet mt-4" { "Not now" }
        },
    )
}

async fn link_page(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    let linking = linking(&state, &user, &headers).await?;
    Ok(link_view(&linking))
}

#[derive(Deserialize)]
struct LinkForm {
    /// The address the page offered, so a different assertion arriving in
    /// the meantime doesn't link something else.
    email: String,
}

async fn link(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<LinkForm>,
) -> AppResult<Response> {
    let linking = linking(&state, &user, &headers).await?;
    let email = match &linking {
        Linking::Ready(email) if *email == form.email.trim().to_lowercase() => email.clone(),
        _ => return Ok((StatusCode::CONFLICT, link_view(&linking)).into_response()),
    };
    let user_id = user.id;
    state
        .db
        .call(move |conn| security::set_email(conn, user_id, Some(&email)))
        .await?;
    Ok(Redirect::to("/settings/security?notice=email").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_domains_are_normalized() {
        for input in [
            "myteam",
            "MyTeam.cloudflareaccess.com",
            "https://myteam.cloudflareaccess.com/",
            " myteam.cloudflareaccess.com ",
        ] {
            assert_eq!(
                team_domain(input).as_deref(),
                Ok("myteam.cloudflareaccess.com"),
                "{input}"
            );
        }
        assert_eq!(
            team_domain("auth.example.com").as_deref(),
            Ok("auth.example.com")
        );
        for input in [
            "",
            "http://myteam.cloudflareaccess.com",
            "myteam.cloudflareaccess.com/cdn-cgi",
            "my team",
            "-team",
        ] {
            assert!(team_domain(input).is_err(), "{input}");
        }
    }

    fn claims(json: &serde_json::Value) -> Claims {
        serde_json::from_value(json.clone()).unwrap_or_else(|error| panic!("{error}"))
    }

    fn base() -> serde_json::Value {
        serde_json::json!({
            "iss": "https://team.cloudflareaccess.com",
            "aud": ["other", "aud-tag"],
            "exp": 2_000,
            "iat": 1_000,
            "nbf": 1_000,
            "type": "app",
            "email": "Ada@Example.com",
        })
    }

    fn check(json: &serde_json::Value, now: i64) -> Result<Assertion, Failure> {
        check_claims(claims(json), "team.cloudflareaccess.com", "aud-tag", now)
    }

    #[test]
    fn claims_are_checked() {
        let person = Ok(Assertion::Person {
            email: "ada@example.com".to_owned(),
        });
        assert_eq!(check(&base(), 1_500), person);
        // Clocks may be a minute apart either way.
        assert_eq!(check(&base(), 2_059), person);
        assert_eq!(check(&base(), 941), person);
        assert_eq!(check(&base(), 2_060), Err(Failure::Invalid("expired")));
        assert_eq!(check(&base(), 900), Err(Failure::Invalid("not valid yet")));
        let mut one = base();
        one["aud"] = "aud-tag".into();
        assert_eq!(check(&one, 1_500), person);
        let changed = |key: &str, value: serde_json::Value| {
            let mut json = base();
            json[key] = value;
            check(&json, 1_500)
        };
        assert!(changed("aud", "other".into()).is_err());
        assert!(changed("iss", "https://evil.cloudflareaccess.com".into()).is_err());
        assert!(changed("type", "org".into()).is_err());
        assert!(changed("email", "".into()).is_err());
        assert_eq!(
            changed("common_name", "robot.access".into()),
            Ok(Assertion::Service)
        );
    }

    #[test]
    fn only_rs256_with_a_key_id_is_read() {
        let part = |value: &serde_json::Value| {
            Base64UrlUnpadded::encode_string(value.to_string().as_bytes())
        };
        let claims = part(&base());
        for header in [
            serde_json::json!({ "alg": "none", "kid": "a" }),
            serde_json::json!({ "alg": "HS256", "kid": "a" }),
            serde_json::json!({ "alg": "RS256" }),
        ] {
            let token = format!("{}.{claims}.", part(&header));
            assert!(parse(&token).is_err(), "{header}");
        }
        let token = format!(
            "{}.{claims}.c2ln",
            part(&serde_json::json!({ "alg": "RS256", "kid": "a" }))
        );
        let parsed = parse(&token).unwrap_or_else(|_| panic!("parses"));
        assert_eq!(parsed.kid, "a");
        assert_eq!(parsed.signature, b"sig");
        assert_eq!(
            Some(parsed.signed),
            token.rsplit_once('.').map(|(signed, _)| signed)
        );
        assert!(parse("a.b").is_err());
        assert!(parse("a.b.c.d").is_err());
    }
}
