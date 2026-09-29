//! Signing in with passkeys, authenticator codes and email links, and the
//! page where people set them up. Admins choose what sign-in requires.

use axum::{
    Form, Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use serde_json::json;

use super::{NextQuery, base_url, redirect_with_cookie, shell_data};
use crate::{
    AppState,
    auth::{self, CurrentUser},
    error::{AppError, AppResult},
    mail, now_ms,
    passkeys::{self, Purpose, Site},
    security::{self, LinkPurpose, Policy, Requirement},
    store, totp,
    views::{Shell, security as pages},
};

pub const PENDING_COOKIE: &str = "sideporch_pending";
const LINK_MINUTES: i64 = 15;
const CONFIRM_HOURS: i64 = 24;
const MINUTE_MS: i64 = 60 * 1000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/security", get(settings))
        .route(
            "/settings/security/totp",
            get(totp_setup).post(totp_confirm),
        )
        .route("/settings/security/totp/delete", post(totp_delete))
        .route("/settings/security/recovery", post(new_recovery_codes))
        .route(
            "/settings/security/passkeys/{passkey_id}/delete",
            post(delete_passkey),
        )
        .route("/settings/security/email", post(add_email))
        .route("/settings/security/email/remove", post(remove_email))
        .route("/settings/security/email/{token}", get(confirm_email))
        .route("/webauthn/register/options", post(register_options))
        .route("/webauthn/register", post(register))
        .route("/webauthn/login/options", post(login_options))
        .route("/webauthn/login", post(login))
        .route("/login/verify", get(verify_form).post(verify))
        .route("/login/email", get(email_form).post(send_link))
        .route("/login/link/{token}", get(link_form).post(use_link))
        .route("/admin/sign-in", get(policy_page).post(save_policy))
        .route("/admin/sign-in/test-email", post(test_email))
        .route("/people/{user_id}/reset-security", post(reset_security))
}

fn site(state: &AppState, headers: &HeaderMap) -> Site {
    Site::from_base_url(&base_url(state, headers))
}

/// Browsers only use passkeys for domain names, not IP addresses.
fn passkey_site(state: &AppState, headers: &HeaderMap) -> AppResult<Site> {
    let site = site(state, headers);
    if site.rp_id.parse::<std::net::IpAddr>().is_ok() {
        return Err(AppError::bad_request(
            "Passkeys only work when Sideporch is opened by a name, like chat.example.com or localhost, not an IP address.",
        ));
    }
    Ok(site)
}

fn pending_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == PENDING_COOKIE && !value.is_empty()).then(|| value.to_owned())
        })
}

fn pending_cookie(state: &AppState, token: &str, max_age: i64) -> String {
    let secure = if state.secure_cookies { "; Secure" } else { "" };
    format!("{PENDING_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

/// Who is between the first and second step of signing in, if anyone.
async fn pending_user(state: &AppState, headers: &HeaderMap) -> AppResult<Option<i64>> {
    let Some(token) = pending_token(headers) else {
        return Ok(None);
    };
    let hash = auth::hash_token(&token);
    let now = now_ms();
    state
        .db
        .call(move |conn| security::pending(conn, &hash, now))
        .await
}

/// After a password, email link or reset: signs in, or asks for the second
/// step when the account has one.
pub async fn after_first_step(
    state: &AppState,
    user_id: i64,
    next: Option<&str>,
) -> AppResult<Response> {
    let factors = state
        .db
        .call(move |conn| security::factors(conn, user_id))
        .await?;
    let next = next.and_then(auth::safe_next).unwrap_or("/");
    if !factors.second_step() {
        let cookie = auth::start_session(state, user_id).await?;
        return redirect_with_cookie(next, &cookie);
    }
    let token = auth::random_token()?;
    let hash = auth::hash_token(&token);
    let now = now_ms();
    state
        .db
        .call(move |conn| security::start_pending(conn, &hash, user_id, now))
        .await?;
    let target = if next == "/" {
        "/login/verify".to_owned()
    } else {
        format!("/login/verify?next={}", auth::encode_component(next))
    };
    redirect_with_cookie(&target, &pending_cookie(state, &token, 600))
}

/// Signs in after the second step, clearing the pending sign-in.
async fn finish(
    state: &AppState,
    headers: &HeaderMap,
    user_id: i64,
    next: &str,
) -> AppResult<Response> {
    if let Some(token) = pending_token(headers) {
        let hash = auth::hash_token(&token);
        state
            .db
            .call(move |conn| security::end_pending(conn, &hash))
            .await?;
    }
    let cookie = auth::start_session(state, user_id).await?;
    let mut response = redirect_with_cookie(auth::safe_next(next).unwrap_or("/"), &cookie)?;
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(&pending_cookie(state, "", 0)).map_err(AppError::internal)?,
    );
    Ok(response)
}

// The settings page

#[derive(Deserialize, Default)]
struct Notice {
    #[serde(default)]
    notice: String,
}

async fn render_settings(
    state: &AppState,
    user: &CurrentUser,
    notice: Option<&str>,
    codes: Option<&[String]>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (passkeys, factors, policy, email, mail_ready) = state
        .db
        .call(move |conn| {
            Ok((
                security::passkeys(conn, user_id)?,
                security::factors(conn, user_id)?,
                Policy::load(conn)?,
                security::email(conn, user_id)?,
                mail::configured(conn)?,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(pages::settings_page(
        &shell,
        &pages::SecurityView {
            passkeys: &passkeys,
            factors,
            policy,
            email: email.as_deref(),
            mail_ready,
            notice,
            codes,
        },
    ))
}

async fn settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(notice): Query<Notice>,
) -> AppResult<Markup> {
    let notice = match notice.notice.as_str() {
        "passkey" => Some("Passkey added. Use it to sign in on any device that has it."),
        "email" => Some("Address confirmed."),
        "sent" => Some("We sent a link to that address. Open it to confirm the address."),
        _ => None,
    };
    render_settings(&state, &user, notice, None).await
}

async fn delete_passkey(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(passkey_id): Path<i64>,
) -> AppResult<Redirect> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| security::delete_passkey(conn, user_id, passkey_id))
        .await?;
    Ok(Redirect::to("/settings/security"))
}

fn totp_name(user_id: i64) -> String {
    format!("totp.{user_id}")
}

async fn totp_setup(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let secret = totp::new_secret()?;
    let user_id = user.id;
    let username = state
        .db
        .call(move |conn| Ok(store::user(conn, user_id)?.map(|user| user.username)))
        .await?
        .unwrap_or_default();
    let sealed = state
        .vault
        .seal_text(&totp_name(user.id), &totp::base32(&secret))?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(pages::totp_page(
        &shell,
        &totp::uri(&secret, &username),
        &totp::base32(&secret),
        &sealed,
        None,
    ))
}

#[derive(Deserialize)]
struct TotpForm {
    /// The new secret, sealed, so nobody can swap it on the way.
    secret: String,
    code: String,
}

async fn totp_confirm(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<TotpForm>,
) -> AppResult<Response> {
    let encoded = state.vault.open_text(&totp_name(user.id), &form.secret)?;
    let secret =
        totp::from_base32(&encoded).ok_or_else(|| AppError::bad_request("Start again."))?;
    let Some(step) = totp::verify(&secret, &form.code, now_ms(), 0) else {
        let sidebar = shell_data(&state, user.id).await?;
        let shell = Shell {
            user: &user,
            sidebar: &sidebar,
            current: None,
        };
        let username = user.display_name.clone();
        return Ok((
            StatusCode::BAD_REQUEST,
            pages::totp_page(
                &shell,
                &totp::uri(&secret, &username),
                &encoded,
                &form.secret,
                Some(
                    "That code didn't match. Check the time on your phone and try the newest code.",
                ),
            ),
        )
            .into_response());
    };
    let user_id = user.id;
    let codes = totp::recovery_codes()?;
    let stored = codes.clone();
    let sealed = form.secret;
    let had_codes = state
        .db
        .call(move |conn| {
            security::set_totp(conn, user_id, Some(&sealed), step)?;
            let had = security::factors(conn, user_id)?.recovery_codes > 0;
            if !had {
                security::replace_recovery_codes(conn, user_id, &stored)?;
            }
            Ok(had)
        })
        .await?;
    let notice =
        "Authenticator app set up. From now on, signing in with your password asks for a code.";
    Ok(render_settings(
        &state,
        &user,
        Some(notice),
        (!had_codes).then_some(codes.as_slice()),
    )
    .await?
    .into_response())
}

async fn totp_delete(user: CurrentUser, State(state): State<AppState>) -> AppResult<Redirect> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| security::set_totp(conn, user_id, None, 0))
        .await?;
    Ok(Redirect::to("/settings/security"))
}

async fn new_recovery_codes(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let codes = totp::recovery_codes()?;
    let stored = codes.clone();
    let user_id = user.id;
    state
        .db
        .call(move |conn| security::replace_recovery_codes(conn, user_id, &stored))
        .await?;
    render_settings(
        &state,
        &user,
        Some("New recovery codes. The old ones no longer work."),
        Some(&codes),
    )
    .await
}

#[derive(Deserialize)]
struct EmailForm {
    email: String,
}

async fn add_email(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<EmailForm>,
) -> AppResult<Response> {
    let email = form.email.trim().to_lowercase();
    if !mail::valid_address(&email) {
        return Ok((
            StatusCode::BAD_REQUEST,
            render_settings(
                &state,
                &user,
                Some("That email address doesn't look right."),
                None,
            )
            .await?,
        )
            .into_response());
    }
    let token = auth::random_token()?;
    let hash = auth::hash_token(&token);
    let (user_id, now) = (user.id, now_ms());
    let vault = std::sync::Arc::clone(&state.vault);
    let stored = email.clone();
    let settings = state
        .db
        .call(move |conn| {
            if security::recent_links(conn, user_id, now)? >= 5 {
                return Err(AppError::bad_request(
                    "Too many emails in the last hour. Try again later.",
                ));
            }
            security::create_link(
                conn,
                &hash,
                user_id,
                LinkPurpose::ConfirmEmail,
                &stored,
                now,
                CONFIRM_HOURS.saturating_mul(60).saturating_mul(MINUTE_MS),
            )?;
            mail::Settings::load(conn, &vault)
        })
        .await?;
    let link = format!(
        "{}/settings/security/email/{token}",
        base_url(&state, &headers)
    );
    mail::send(
        &settings,
        &email,
        "Confirm your email address for Sideporch",
        &format!(
            "Hi {},\n\nOpen this link to use {email} with your Sideporch account:\n\n{link}\n\nThe link works for {CONFIRM_HOURS} hours. If you didn't ask for this, ignore this email.\n",
            user.display_name
        ),
    )
    .await?;
    Ok(Redirect::to("/settings/security?notice=sent").into_response())
}

async fn confirm_email(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> AppResult<Redirect> {
    let hash = auth::hash_token(&token);
    let (user_id, now) = (user.id, now_ms());
    state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            match security::take_link(&tx, &hash, LinkPurpose::ConfirmEmail, now)? {
                Some((owner, email)) if owner == user_id => {
                    security::set_email(&tx, user_id, Some(&email))?;
                }
                _ => {
                    return Err(AppError::Gone(
                        "This link expired or was used already. Send a new one from Sign-in and security.".to_owned(),
                    ));
                }
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    Ok(Redirect::to("/settings/security?notice=email"))
}

async fn remove_email(user: CurrentUser, State(state): State<AppState>) -> AppResult<Redirect> {
    let user_id = user.id;
    state
        .db
        .call(move |conn| security::set_email(conn, user_id, None))
        .await?;
    Ok(Redirect::to("/settings/security"))
}

// Passkey ceremonies

fn credential_list(ids: &[(Vec<u8>, Vec<String>)]) -> Vec<serde_json::Value> {
    ids.iter()
        .map(|(id, transports)| json!({ "type": "public-key", "id": passkeys::encode(id), "transports": transports }))
        .collect()
}

async fn register_options(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let user_id = user.id;
    let (handle, existing, username) = state
        .db
        .call(move |conn| {
            Ok((
                security::user_handle(conn, user_id)?,
                security::credential_ids(conn, user_id)?,
                store::user(conn, user_id)?
                    .map(|user| user.username)
                    .unwrap_or_default(),
            ))
        })
        .await?;
    let site = passkey_site(&state, &headers)?;
    let (ceremony, challenge) = state
        .ceremonies
        .start(Purpose::Register(user.id), now_ms())?;
    Ok(Json(json!({
        "ceremony": ceremony,
        "publicKey": {
            "challenge": passkeys::encode(&challenge),
            "rp": { "name": "Sideporch", "id": site.rp_id },
            "user": { "id": passkeys::encode(&handle), "name": username, "displayName": user.display_name },
            "pubKeyCredParams": passkeys::ALGORITHMS.iter().map(|alg| json!({ "type": "public-key", "alg": alg })).collect::<Vec<_>>(),
            "timeout": 120_000,
            "attestation": "none",
            "excludeCredentials": credential_list(&existing),
            "authenticatorSelection": { "residentKey": "required", "requireResidentKey": true, "userVerification": "required" },
        },
    })))
}

async fn register(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(registration): Json<passkeys::Registration>,
) -> AppResult<Json<serde_json::Value>> {
    let now = now_ms();
    let challenge =
        state
            .ceremonies
            .finish(&registration.ceremony, &Purpose::Register(user.id), now)?;
    let passkey =
        passkeys::verify_registration(&registration, &challenge, &site(&state, &headers))?;
    let name: String = registration.name.trim().chars().take(60).collect();
    let name = if name.is_empty() {
        passkeys::default_name(&passkey.transports)
    } else {
        name
    };
    let user_id = user.id;
    state
        .db
        .call(move |conn| security::add_passkey(conn, user_id, &passkey, &name, now))
        .await?;
    Ok(Json(
        json!({ "redirect": "/settings/security?notice=passkey" }),
    ))
}

async fn login_options(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Json<serde_json::Value>> {
    let pending = pending_user(&state, &headers).await?;
    let (purpose, allowed) = match pending {
        Some(user_id) => (
            Purpose::Confirm(user_id),
            state
                .db
                .call(move |conn| security::credential_ids(conn, user_id))
                .await?,
        ),
        None => (Purpose::SignIn, Vec::new()),
    };
    let (ceremony, challenge) = state.ceremonies.start(purpose, now_ms())?;
    Ok(Json(json!({
        "ceremony": ceremony,
        "publicKey": {
            "challenge": passkeys::encode(&challenge),
            "rpId": passkey_site(&state, &headers)?.rp_id,
            "timeout": 120_000,
            "userVerification": "required",
            "allowCredentials": credential_list(&allowed),
        },
    })))
}

#[derive(Deserialize)]
struct PasskeyLogin {
    #[serde(flatten)]
    assertion: passkeys::Assertion,
    #[serde(default)]
    next: String,
}

/// Signs in with a passkey: on its own, or as the second step.
async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(form): Json<PasskeyLogin>,
) -> AppResult<Response> {
    let now = now_ms();
    let credential = passkeys::decode(&form.assertion.id)?;
    let stored = state
        .db
        .call(move |conn| security::passkey_by_credential(conn, &credential))
        .await?
        .ok_or_else(|| {
            AppError::bad_request("This passkey isn't known here. It may have been removed.")
        })?;
    let pending = pending_user(&state, &headers).await?;
    let purpose = match pending {
        Some(user_id) if user_id == stored.user_id => Purpose::Confirm(user_id),
        Some(_) => {
            return Err(AppError::bad_request(
                "That passkey belongs to another account.",
            ));
        }
        None => Purpose::SignIn,
    };
    let challenge = state
        .ceremonies
        .finish(&form.assertion.ceremony, &purpose, now)?;
    let count = passkeys::verify_assertion(
        &form.assertion,
        &stored,
        &challenge,
        &site(&state, &headers),
    )?;
    let passkey_id = stored.id;
    state
        .db
        .call(move |conn| security::passkey_used(conn, passkey_id, count, now))
        .await?;
    let response = finish(&state, &headers, stored.user_id, &form.next).await?;
    // app.js follows the redirect itself.
    let location = response
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("/")
        .to_owned();
    let mut json = Json(json!({ "redirect": location })).into_response();
    for cookie in response.headers().get_all(header::SET_COOKIE) {
        json.headers_mut()
            .append(header::SET_COOKIE, cookie.clone());
    }
    Ok(json)
}

// The second step

async fn verify_form(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<NextQuery>,
) -> AppResult<Response> {
    let Some(user_id) = pending_user(&state, &headers).await? else {
        return Ok(Redirect::to("/login").into_response());
    };
    let factors = state
        .db
        .call(move |conn| security::factors(conn, user_id))
        .await?;
    Ok(pages::verify_page(
        factors,
        query.next.as_deref().and_then(auth::safe_next),
        None,
    )
    .into_response())
}

#[derive(Deserialize)]
struct VerifyForm {
    #[serde(default)]
    code: String,
    #[serde(default)]
    recovery: String,
    #[serde(default)]
    next: String,
}

async fn verify(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<VerifyForm>,
) -> AppResult<Response> {
    let Some(token) = pending_token(&headers) else {
        return Ok(Redirect::to("/login").into_response());
    };
    let hash = auth::hash_token(&token);
    let now = now_ms();
    let vault = std::sync::Arc::clone(&state.vault);
    let lookup = hash.clone();
    let result = state
        .db
        .call(move |conn| {
            let Some(user_id) = security::pending(conn, &lookup, now)? else {
                return Ok(Err(None));
            };
            if !form.recovery.trim().is_empty() {
                if security::use_recovery_code(conn, user_id, &form.recovery, now)? {
                    return Ok(Ok(user_id));
                }
            } else if let Some((sealed, last)) = security::totp(conn, user_id)? {
                let encoded = vault.open_text(&totp_name(user_id), &sealed)?;
                let secret = totp::from_base32(&encoded).unwrap_or_default();
                if let Some(step) = totp::verify(&secret, &form.code, now, last) {
                    security::totp_used(conn, user_id, step)?;
                    return Ok(Ok(user_id));
                }
            }
            security::failed_attempt(conn, &lookup)?;
            Ok(Err(Some(security::factors(conn, user_id)?)))
        })
        .await?;
    match result {
        Ok(user_id) => finish(&state, &headers, user_id, &form.next).await,
        Err(None) => Ok(Redirect::to("/login").into_response()),
        Err(Some(factors)) => Ok((
            StatusCode::UNAUTHORIZED,
            pages::verify_page(
                factors,
                auth::safe_next(&form.next),
                Some(
                    "That code didn't work. Try the newest one from your app, or a recovery code.",
                ),
            ),
        )
            .into_response()),
    }
}

// Email links

async fn email_ready(state: &AppState) -> AppResult<bool> {
    state.db.call(|conn| mail::configured(conn)).await
}

#[derive(Deserialize, Default)]
struct EmailQuery {
    /// `reset` for a password reset link.
    #[serde(default)]
    purpose: String,
}

async fn email_form(
    State(state): State<AppState>,
    Query(query): Query<EmailQuery>,
) -> AppResult<Markup> {
    let policy = state.db.call(|conn| Policy::load(conn)).await?;
    let reset = query.purpose == "reset";
    if !email_ready(&state).await? || (!reset && !policy.email_links) {
        return Err(AppError::NotFound);
    }
    Ok(pages::email_link_page(reset, false))
}

#[derive(Deserialize)]
struct SendLinkForm {
    email: String,
    #[serde(default)]
    purpose: String,
}

/// Emails a sign-in or password reset link. The answer is the same whether
/// or not an account has the address, so nobody learns who is here.
async fn send_link(
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SendLinkForm>,
) -> AppResult<Markup> {
    let reset = form.purpose == "reset";
    let policy = state.db.call(|conn| Policy::load(conn)).await?;
    if !email_ready(&state).await? || (!reset && !policy.email_links) {
        return Err(AppError::NotFound);
    }
    let email = form.email.trim().to_lowercase();
    let token = auth::random_token()?;
    let hash = auth::hash_token(&token);
    let now = now_ms();
    let vault = std::sync::Arc::clone(&state.vault);
    let lookup = email.clone();
    let found = state
        .db
        .call(move |conn| {
            let Some(user_id) = security::user_by_email(conn, &lookup)? else {
                return Ok(None);
            };
            if security::recent_links(conn, user_id, now)? >= 5 {
                return Ok(None);
            }
            let valid = LINK_MINUTES.saturating_mul(MINUTE_MS);
            let purpose = if reset {
                LinkPurpose::ResetPassword
            } else {
                LinkPurpose::SignIn
            };
            security::create_link(conn, &hash, user_id, purpose, &lookup, now, valid)?;
            let name = store::user(conn, user_id)?
                .map(|user| user.display_name)
                .unwrap_or_default();
            Ok(Some((name, mail::Settings::load(conn, &vault)?)))
        })
        .await?;
    if let Some((name, settings)) = found {
        let base = base_url(&state, &headers);
        let (subject, body) = if reset {
            (
                "Reset your Sideporch password",
                format!(
                    "Hi {name},\n\nOpen this link to choose a new password:\n\n{base}/reset/{token}\n\nThe link works for {LINK_MINUTES} minutes. If you didn't ask for this, ignore this email; your password stays as it is.\n"
                ),
            )
        } else {
            (
                "Your Sideporch sign-in link",
                format!(
                    "Hi {name},\n\nOpen this link to sign in:\n\n{base}/login/link/{token}\n\nThe link works once, for {LINK_MINUTES} minutes. If you didn't ask for this, ignore this email.\n"
                ),
            )
        };
        if let Err(error) = mail::send(&settings, &email, subject, &body).await {
            tracing::warn!(%error, "could not send a sign-in email");
        }
    }
    Ok(pages::email_link_page(reset, true))
}

/// Shows a button rather than signing in on GET, since mail scanners open
/// links before people do.
async fn link_form(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Markup> {
    let hash = auth::hash_token(&token);
    let now = now_ms();
    let valid = state
        .db
        .call(move |conn| security::link_valid(conn, &hash, LinkPurpose::SignIn, now))
        .await?;
    if !valid {
        return Err(AppError::Gone(
            "This sign-in link expired or was used already. Ask for a new one.".to_owned(),
        ));
    }
    Ok(pages::use_link_page(&token))
}

async fn use_link(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Response> {
    let hash = auth::hash_token(&token);
    let now = now_ms();
    let found = state
        .db
        .call(move |conn| security::take_link(conn, &hash, LinkPurpose::SignIn, now))
        .await?;
    let Some((user_id, _)) = found else {
        return Err(AppError::Gone(
            "This sign-in link expired or was used already. Ask for a new one.".to_owned(),
        ));
    };
    let requirement = state.db.call(|conn| Policy::load(conn)).await?.require;
    let factors = state
        .db
        .call(move |conn| security::factors(conn, user_id))
        .await?;
    if requirement == Requirement::Passkeys && factors.passkeys > 0 {
        return Err(AppError::bad_request("Sign in with your passkey."));
    }
    after_first_step(&state, user_id, None).await
}

// Admins

async fn render_policy(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: Option<&str>,
) -> AppResult<Markup> {
    let vault = std::sync::Arc::clone(&state.vault);
    let (policy, mail_settings, counts) = state
        .db
        .call(move |conn| {
            let counts = conn.query_row(
                "SELECT (SELECT COUNT(*) FROM users WHERE deactivated_at IS NULL),
                        (SELECT COUNT(DISTINCT user_id) FROM passkeys),
                        (SELECT COUNT(*) FROM users WHERE totp_secret IS NOT NULL)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?;
            Ok((
                Policy::load(conn)?,
                mail::Settings::load(conn, &vault)?,
                counts,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(pages::policy_page(
        &shell,
        policy,
        &mail_settings,
        counts,
        error,
        saved,
    ))
}

async fn policy_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    render_policy(&state, &user, None, None).await
}

#[derive(Deserialize)]
struct PolicyForm {
    require: String,
    email_links: Option<String>,
    #[serde(default)]
    mail_host: String,
    #[serde(default)]
    mail_port: String,
    #[serde(default)]
    mail_security: String,
    #[serde(default)]
    mail_username: String,
    /// Empty keeps the stored password.
    #[serde(default)]
    mail_password: String,
    mail_password_clear: Option<String>,
    #[serde(default)]
    mail_from: String,
}

async fn save_policy(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<PolicyForm>,
) -> AppResult<Response> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    let require = Requirement::parse(&form.require)
        .ok_or_else(|| AppError::bad_request("Pick what signing in requires."))?;
    let mail_settings = mail::Settings {
        host: form.mail_host.trim().to_owned(),
        port: form.mail_port.trim().parse().unwrap_or(587),
        security: mail::Security::parse(&form.mail_security).unwrap_or_default(),
        username: form.mail_username.trim().to_owned(),
        password: None,
        from: form.mail_from.trim().to_owned(),
    };
    let password = if form.mail_password_clear.is_some() {
        Some(String::new())
    } else if form.mail_password.is_empty() {
        None
    } else {
        Some(form.mail_password)
    };
    let email_links = form.email_links.is_some();
    let (user_id, is_admin) = (user.id, user.is_admin);
    let vault = std::sync::Arc::clone(&state.vault);
    let result = state
        .db
        .call(move |conn| {
            // Admins can't lock themselves out: they must meet the policy first.
            let policy = Policy { require, email_links };
            if policy.needs_more(is_admin, security::factors(conn, user_id)?) {
                return Ok(Err(match require {
                    Requirement::Passkeys => "Add a passkey to your own account first, under Sign-in and security.",
                    _ => "Set up a passkey or an authenticator app for your own account first, under Sign-in and security.",
                }));
            }
            mail_settings.save(conn, &vault, password.as_deref())?;
            if email_links && !mail::configured(conn)? {
                return Ok(Err("Set up email before turning on sign-in links."));
            }
            policy.save(conn)?;
            Ok(Ok(()))
        })
        .await?;
    match result {
        Ok(()) => Ok(render_policy(&state, &user, None, Some("Saved."))
            .await?
            .into_response()),
        Err(error) => Ok((
            StatusCode::BAD_REQUEST,
            render_policy(&state, &user, Some(error), None).await?,
        )
            .into_response()),
    }
}

#[derive(Deserialize)]
struct TestForm {
    to: String,
}

async fn test_email(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<TestForm>,
) -> AppResult<Response> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    let vault = std::sync::Arc::clone(&state.vault);
    let settings = state
        .db
        .call(move |conn| mail::Settings::load(conn, &vault))
        .await?;
    let email = form.to.trim().to_owned();
    match mail::send(
        &settings,
        &email,
        "Sideporch can send email",
        "This is a test from your Sideporch's email settings. It works.\n",
    )
    .await
    {
        Ok(()) => Ok(render_policy(
            &state,
            &user,
            None,
            Some(&format!("Sent a test email to {email}.")),
        )
        .await?
        .into_response()),
        Err(error) => Ok((
            StatusCode::BAD_REQUEST,
            render_policy(&state, &user, Some(&error.to_string()), None).await?,
        )
            .into_response()),
    }
}

async fn reset_security(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> AppResult<Redirect> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    if user.id == user_id {
        return Err(AppError::bad_request(
            "Ask another admin to reset your own sign-in.",
        ));
    }
    state
        .db
        .call(move |conn| security::reset_factors(conn, user_id))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}
