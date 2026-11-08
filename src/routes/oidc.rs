//! Signing in with an `OpenID Connect` provider, and linking a provider
//! identity to an account from Sign-in and security. The checks are in
//! [`crate::oidc`]; this module drives the browser through them.
//!
//! A sign-in starts at `/login/oidc`, which records it server-side and sets
//! a cookie only this browser has. The provider sends the browser back to
//! `/login/oidc/callback`, where the login must match that cookie and is
//! used up. Whatever the answer, a sign-in ends in
//! [`super::security::after_first_step`], so a second step still applies.

use std::sync::Arc;

use axum::{
    Form, Router,
    extract::{Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use rusqlite::{Connection, OptionalExtension as _, params};
use serde::Deserialize;

use super::{base_url, redirect_with_cookie};
use crate::{
    AppState,
    auth::{self, CurrentUser},
    automations,
    community::{self, Registration},
    error::{AppError, AppResult},
    now_ms, oidc,
    security::{self as factors, Policy, Requirement},
    store,
};

/// What every failed sign-in says, whatever the reason.
const FAILED: &str = "Sideporch couldn't finish signing you in. Try again, or sign in another way.";
const UNAVAILABLE: &str = "Sideporch couldn't reach your sign-in provider. Try again in a moment.";
const NOT_LINKED: &str = "That sign-in isn't linked to an account here. Sign in another way, then link it under Sign-in and security.";
const NO_USERNAME: &str = "Your sign-in provider didn't give a username that Sideporch can use here. Ask an admin to set one.";
const CANT_USE: &str = "This account can't use a sign-in with a provider here.";
const SECURITY_PAGE: &str = "/settings/security";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/login/oidc", get(sign_in))
        .route(oidc::CALLBACK_PATH, get(callback))
        .route("/settings/security/oidc", post(link))
        .route("/settings/security/oidc/unlink", post(unlink))
        .route("/admin/sign-in/oidc", post(save))
}

/// Logs why a sign-in was refused, and says the generic thing to the visitor.
fn refused(reason: &str) -> AppError {
    tracing::warn!(reason, "could not finish an OpenID Connect sign-in");
    AppError::bad_request(FAILED)
}

fn failed(failure: oidc::Failure) -> AppError {
    match failure {
        oidc::Failure::Refused(reason) => refused(reason),
        oidc::Failure::Unavailable(reason) => {
            tracing::warn!(reason, "could not reach the sign-in provider");
            AppError::bad_request(UNAVAILABLE)
        }
    }
}

async fn provider(state: &AppState) -> AppResult<Option<oidc::Provider>> {
    let vault = Arc::clone(&state.vault);
    state
        .db
        .call(move |conn| oidc::provider(conn, &vault))
        .await
}

fn redirect_uri(state: &AppState, headers: &HeaderMap) -> String {
    format!("{}{}", base_url(state, headers), oidc::CALLBACK_PATH)
}

fn browser_cookie(state: &AppState, value: &str, max_age: i64) -> String {
    let secure = if state.secure_cookies { "; Secure" } else { "" };
    format!(
        "{}={value}; Path=/login/oidc; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}",
        oidc::BROWSER_COOKIE
    )
}

/// The browser's proof, from its cookie.
fn browser_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == oidc::BROWSER_COOKIE && !value.is_empty()).then(|| value.to_owned())
        })
}

/// Ends the browser's sign-in, whatever it led to.
fn end_browser(state: &AppState, response: &mut Response) -> AppResult<()> {
    let cookie = browser_cookie(state, "", 0);
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(AppError::internal)?,
    );
    Ok(())
}

/// What a sign-in started at the provider is for.
struct Start {
    /// `sign-in`, or `link` when a signed-in person links an identity.
    purpose: &'static str,
    user_id: Option<i64>,
    next: String,
}

/// Records a sign-in and sends the browser to the provider.
async fn begin(
    state: &AppState,
    headers: &HeaderMap,
    provider: &oidc::Provider,
    start: Start,
) -> AppResult<Response> {
    let discovery = state
        .oidc
        .discovery(&provider.issuer)
        .await
        .map_err(failed)?;
    let sign_in_state = auth::random_token()?;
    let browser = auth::random_token()?;
    let nonce = auth::random_token()?;
    let verifier = oidc::random_verifier()?;
    let now = now_ms();
    let row = (
        auth::hash_token(&sign_in_state),
        auth::hash_token(&browser),
        nonce.clone(),
        verifier.clone(),
        provider.issuer.clone(),
        start.purpose,
        start.user_id,
        start.next,
    );
    let expires = now.saturating_add(oidc::LOGIN_MINUTES.saturating_mul(60_000));
    state
        .db
        .call(move |conn| {
            conn.execute("DELETE FROM oidc_logins WHERE expires_at < ?1", [now])?;
            conn.execute(
                "INSERT INTO oidc_logins (state_hash, browser_hash, nonce, verifier, issuer, purpose, user_id, next, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![row.0, row.1, row.2, row.3, row.4, row.5, row.6, row.7, now, expires],
            )?;
            Ok(())
        })
        .await?;
    let location = oidc::authorize_url(
        &discovery.authorization_endpoint,
        &provider.client_id,
        &redirect_uri(state, headers),
        &sign_in_state,
        &nonce,
        &oidc::pkce_challenge(&verifier),
    );
    let cookie = browser_cookie(state, &browser, oidc::LOGIN_MINUTES * 60);
    redirect_with_cookie(&location, &cookie)
}

#[derive(Deserialize, Default)]
struct StartQuery {
    next: Option<String>,
}

/// `/login/oidc`: the sign-in button. Missing when no provider is set up.
async fn sign_in(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> AppResult<Response> {
    let provider = provider(&state).await?.ok_or(AppError::NotFound)?;
    let next = query
        .next
        .as_deref()
        .and_then(auth::safe_next)
        .unwrap_or("/")
        .to_owned();
    begin(
        &state,
        &headers,
        &provider,
        Start {
            purpose: "sign-in",
            user_id: None,
            next,
        },
    )
    .await
}

#[derive(Deserialize)]
struct Answer {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    iss: Option<String>,
}

/// A sign-in as it was started, read back and used up.
struct Login {
    issuer: String,
    verifier: String,
    nonce: String,
    purpose: String,
    user_id: Option<i64>,
    next: String,
}

/// Uses up the login that `state_hash` names, when this browser started it
/// and it hasn't expired. A login another browser presents is left alone.
async fn take_login(
    state: &AppState,
    state_hash: Vec<u8>,
    browser_hash: Vec<u8>,
    now: i64,
) -> AppResult<Option<Login>> {
    state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let row = tx
                .query_row(
                    "SELECT browser_hash, nonce, verifier, issuer, purpose, user_id, next, expires_at
                     FROM oidc_logins WHERE state_hash = ?1",
                    params![state_hash],
                    |row| {
                        Ok((
                            row.get::<_, Vec<u8>>(0)?,
                            Login {
                                nonce: row.get(1)?,
                                verifier: row.get(2)?,
                                issuer: row.get(3)?,
                                purpose: row.get(4)?,
                                user_id: row.get(5)?,
                                next: row.get(6)?,
                            },
                            row.get::<_, i64>(7)?,
                        ))
                    },
                )
                .optional()?;
            let Some((browser, login, expires)) = row else {
                return Ok(None);
            };
            if !auth::same_bytes(&browser, &browser_hash) {
                return Ok(None);
            }
            tx.execute("DELETE FROM oidc_logins WHERE state_hash = ?1", params![state_hash])?;
            tx.commit()?;
            Ok((expires >= now).then_some(login))
        })
        .await
}

/// `/login/oidc/callback`: the provider sends the browser back here.
async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(answer): Query<Answer>,
) -> AppResult<Response> {
    let browser = browser_token(&headers).ok_or_else(|| refused("no browser cookie"))?;
    if let Some(error) = answer.error.as_deref() {
        // The provider's own error code, such as `access_denied`; no tokens.
        let error: String = error.chars().take(40).collect();
        tracing::warn!(error, "the sign-in provider refused a sign-in");
        return Err(AppError::bad_request(FAILED));
    }
    let (Some(code), Some(sign_in_state)) = (answer.code.as_deref(), answer.state.as_deref())
    else {
        return Err(refused("no code or state"));
    };
    let login = take_login(
        &state,
        auth::hash_token(sign_in_state),
        auth::hash_token(&browser),
        now_ms(),
    )
    .await?
    .ok_or_else(|| refused("no sign-in matches this browser, or it was used or expired"))?;
    let provider = provider(&state)
        .await?
        .filter(|provider| provider.issuer == login.issuer)
        .ok_or_else(|| refused("the provider changed while the sign-in was under way"))?;
    if answer
        .iss
        .as_deref()
        .is_some_and(|iss| iss != provider.issuer)
    {
        return Err(refused("the provider named another issuer"));
    }
    let discovery = state
        .oidc
        .discovery(&provider.issuer)
        .await
        .map_err(failed)?;
    let id_token = state
        .oidc
        .exchange(
            &discovery,
            &provider,
            code,
            &login.verifier,
            &redirect_uri(&state, &headers),
        )
        .await
        .map_err(failed)?;
    let identity = state
        .oidc
        .verify(
            &discovery,
            &id_token,
            &oidc::Expect {
                issuer: &provider.issuer,
                client_id: &provider.client_id,
                nonce: &login.nonce,
                domain: provider.domain.as_deref(),
            },
            jiff::Timestamp::now().as_second(),
        )
        .await
        .map_err(failed)?;
    if identity
        .email
        .as_deref()
        .is_some_and(|email| state.access.email_banned(email))
    {
        return Err(refused("the provider's email address is banned here"));
    }
    let mut response = match (login.purpose.as_str(), login.user_id) {
        ("link", Some(user_id)) => {
            link_identity(&state, &headers, user_id, &provider, &identity).await?
        }
        ("sign-in", _) => sign_in_identity(&state, &provider, &identity, &login.next).await?,
        _ => return Err(refused("the sign-in has an unknown purpose")),
    };
    end_browser(&state, &mut response)?;
    Ok(response)
}

/// Whether an account can use a sign-in: local, and not deactivated.
fn usable(conn: &Connection, user_id: i64) -> AppResult<bool> {
    Ok(conn
        .query_row(
            "SELECT instance_id IS NULL AND deactivated_at IS NULL FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(false))
}

/// Signs in the account the identity is linked to, or creates one for a
/// first sign-in when the admin allowed it and sign-up is open.
async fn sign_in_identity(
    state: &AppState,
    provider: &oidc::Provider,
    identity: &oidc::Identity,
    next: &str,
) -> AppResult<Response> {
    let (issuer, subject) = (provider.issuer.clone(), identity.subject.clone());
    let (create, preferred, name) = (
        provider.create_accounts,
        identity.preferred_username.clone(),
        identity.name.clone(),
    );
    let now = now_ms();
    let (user_id, created) = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let found = oidc::identity_owner(&tx, &issuer, &subject)?;
            let (user_id, created) = if let Some(user_id) = found {
                (user_id, None)
            } else {
                {
                    if !create || community::Joining::load(&tx)?.registration != Registration::Open
                    {
                        return Err(AppError::bad_request(NOT_LINKED));
                    }
                    if community::too_many_signups(&tx, now)? {
                        return Err(AppError::bad_request(
                            "Lots of people signed up in the last hour. Try again a little later.",
                        ));
                    }
                    let base = preferred
                        .as_deref()
                        .and_then(oidc::username_from)
                        .ok_or_else(|| AppError::bad_request(NO_USERNAME))?;
                    let username = oidc::free_username(&base, |candidate| {
                        community::username_taken(&tx, candidate).unwrap_or(true)
                    })
                    .ok_or_else(|| AppError::bad_request(NO_USERNAME))?;
                    let display: String = name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(&username)
                        .chars()
                        .take(64)
                        .collect();
                    let user_id = store::create_user(&tx, &username, &display, "", false, now)?;
                    community::set_trust(&tx, user_id, 0, false)?;
                    (user_id, Some((display, username)))
                }
            };
            if !usable(&tx, user_id)? {
                return Err(AppError::bad_request(CANT_USE));
            }
            tx.execute(
                "INSERT INTO oidc_identities (issuer, subject, user_id, created_at, last_used_at)
                 VALUES (?1, ?2, ?3, ?4, ?4)
                 ON CONFLICT (issuer, subject) DO UPDATE SET last_used_at = excluded.last_used_at",
                params![issuer, subject, user_id, now],
            )?;
            tx.commit()?;
            Ok((user_id, created))
        })
        .await?;
    if let Some((user, username)) = created {
        state
            .automations
            .event(automations::Event::MemberJoined(automations::MemberEvent {
                user,
                username,
            }));
    }
    // With passkeys required, a sign-in with a provider works like a password.
    let blocked = state
        .db
        .call(move |conn| {
            Ok(Policy::load(conn)?.require == Requirement::Passkeys
                && factors::factors(conn, user_id)?.passkeys > 0)
        })
        .await?;
    if blocked {
        return Err(AppError::bad_request(
            "This Sideporch signs in with passkeys. Use the passkey button below.",
        ));
    }
    super::security::after_first_step(state, user_id, Some(next)).await
}

/// Links the identity to a signed-in account. The session must still exist
/// and the account not be recovering, checked in the same transaction.
async fn link_identity(
    state: &AppState,
    headers: &HeaderMap,
    user_id: i64,
    provider: &oidc::Provider,
    identity: &oidc::Identity,
) -> AppResult<Response> {
    let session = auth::request_session(headers);
    let (issuer, subject) = (provider.issuer.clone(), identity.subject.clone());
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if !auth::still_signed_in(&tx, session.as_deref(), user_id)? {
                return Err(AppError::Forbidden);
            }
            if !usable(&tx, user_id)? {
                return Err(AppError::bad_request(CANT_USE));
            }
            match oidc::identity_owner(&tx, &issuer, &subject)? {
                Some(owner) if owner == user_id => {}
                Some(_) => {
                    return Err(AppError::bad_request(
                        "That sign-in is already linked to another account here.",
                    ));
                }
                None => {
                    tx.execute(
                        "INSERT INTO oidc_identities (issuer, subject, user_id, created_at, last_used_at)
                         VALUES (?1, ?2, ?3, ?4, NULL)",
                        params![issuer, subject, user_id, now],
                    )?;
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(Redirect::to(&format!("{SECURITY_PAGE}?notice=oidc")).into_response())
}

/// `/settings/security/oidc`: a signed-in person links the provider's
/// sign-in to their account.
async fn link(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let provider = provider(&state).await?.ok_or(AppError::NotFound)?;
    let user_id = user.id;
    let ready = state
        .db
        .call(
            move |conn| Ok(usable(conn, user_id)? && !crate::security::recovering(conn, user_id)?),
        )
        .await?;
    if !ready {
        return Err(AppError::bad_request(CANT_USE));
    }
    begin(
        &state,
        &headers,
        &provider,
        Start {
            purpose: "link",
            user_id: Some(user_id),
            next: SECURITY_PAGE.to_owned(),
        },
    )
    .await
}

/// `/settings/security/oidc/unlink`: removes the provider's sign-in, unless
/// it is the only way into the account.
async fn unlink(user: CurrentUser, State(state): State<AppState>) -> AppResult<Redirect> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| {
            let hash = store::password_hash(conn, user_id)?.unwrap_or_default();
            let other_way = auth::has_password(&hash)
                || factors::factors(conn, user_id)?.passkeys > 0
                || factors::email(conn, user_id)?.is_some();
            if !other_way {
                return Err(AppError::bad_request(
                    "Add a password, a passkey or a confirmed email address before you unlink this sign-in.",
                ));
            }
            conn.execute("DELETE FROM oidc_identities WHERE user_id = ?1", [user_id])?;
            Ok(())
        })
        .await?;
    Ok(Redirect::to(&format!(
        "{SECURITY_PAGE}?notice=oidc-unlinked"
    )))
}

/// The admin's form for the provider. An empty secret keeps the stored one.
#[derive(Deserialize, Default)]
pub struct ProviderForm {
    #[serde(default)]
    issuer: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    domain: String,
    #[serde(default)]
    secret: String,
    create_accounts: Option<String>,
    clear: Option<String>,
}

/// `/admin/sign-in/oidc`: sets the provider up, or turns it off.
async fn save(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<ProviderForm>,
) -> AppResult<Response> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    let vault = Arc::clone(&state.vault);
    let result = state
        .db
        .call(move |conn| {
            if form.clear.is_some() {
                return oidc::clear(conn);
            }
            oidc::save(
                conn,
                &vault,
                &oidc::Input {
                    issuer: form.issuer,
                    client_id: form.client_id,
                    label: form.label,
                    domain: form.domain,
                    create_accounts: form.create_accounts.is_some(),
                    secret: form.secret,
                },
            )
        })
        .await;
    match result {
        Ok(()) => Ok(
            super::security::render_policy(&state, &user, None, Some("Saved."))
                .await?
                .into_response(),
        ),
        Err(AppError::BadRequest(message)) => Ok((
            StatusCode::BAD_REQUEST,
            super::security::render_policy(&state, &user, Some(&message), None).await?,
        )
            .into_response()),
        Err(error) => Err(error),
    }
}
