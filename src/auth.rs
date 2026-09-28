use std::fmt::Write as _;

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};
use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, Method, header, request::Parts},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
};
use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    error::{AppError, AppResult},
    now_ms,
};

pub const SESSION_COOKIE: &str = "sideporch_session";
const SESSION_DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// A random, URL-safe token with 256 bits of entropy.
pub fn random_token() -> AppResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(AppError::internal)?;
    Ok(bytes
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        }))
}

pub fn hash_token(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Hashes a password with Argon2id on the blocking pool.
pub async fn hash_password(password: String) -> AppResult<String> {
    tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(AppError::internal)
    })
    .await
    .map_err(AppError::internal)?
}

pub async fn verify_password(password: String, hash: String) -> AppResult<bool> {
    tokio::task::spawn_blocking(move || {
        let parsed = PasswordHash::new(&hash).map_err(AppError::internal)?;
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(AppError::internal)?
}

/// Creates a session and returns the `Set-Cookie` value for it.
pub async fn start_session(state: &AppState, user_id: i64) -> AppResult<String> {
    let token = random_token()?;
    let token_hash = hash_token(&token);
    let now = now_ms();
    let expires = now.saturating_add(SESSION_DAYS.saturating_mul(DAY_MS));
    state
        .db
        .call(move |conn| {
            conn.execute(
                "INSERT INTO sessions (token_hash, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
                params![token_hash, user_id, now, expires],
            )?;
            Ok(())
        })
        .await?;
    Ok(session_cookie(
        state,
        &token,
        SESSION_DAYS.saturating_mul(24 * 60 * 60),
    ))
}

pub fn clear_cookie(state: &AppState) -> String {
    session_cookie(state, "", 0)
}

fn session_cookie(state: &AppState, token: &str, max_age: i64) -> String {
    let secure = if state.secure_cookies { "; Secure" } else { "" };
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}")
}

pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == SESSION_COOKIE && !value.is_empty()).then(|| value.to_owned())
        })
}

/// The signed-in user. Pages that need one redirect to the login form.
#[derive(Debug, Clone)]
pub struct CurrentUser {
    pub id: i64,
    pub display_name: String,
    pub is_admin: bool,
}

pub async fn lookup_session(state: &AppState, token: String) -> AppResult<Option<CurrentUser>> {
    let token_hash = hash_token(&token);
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            Ok(conn
                .query_row(
                    "SELECT u.id, u.display_name, u.is_admin
                     FROM sessions s JOIN users u ON u.id = s.user_id
                     WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.deactivated_at IS NULL",
                    params![token_hash, now],
                    |row| {
                        Ok(CurrentUser {
                            id: row.get(0)?,
                            display_name: row.get(1)?,
                            is_admin: row.get(2)?,
                        })
                    },
                )
                .optional()?)
        })
        .await
}

pub async fn end_session(state: &AppState, token: String) -> AppResult<()> {
    let token_hash = hash_token(&token);
    state
        .db
        .call(move |conn| {
            conn.execute(
                "DELETE FROM sessions WHERE token_hash = ?1",
                params![token_hash],
            )?;
            Ok(())
        })
        .await
}

/// Rejection for [`CurrentUser`]: a redirect to the login page.
pub enum AuthRejection {
    Login,
    Error(AppError),
}

impl IntoResponse for AuthRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Login => Redirect::to("/login").into_response(),
            Self::Error(error) => error.into_response(),
        }
    }
}

impl FromRequestParts<AppState> for CurrentUser {
    type Rejection = AuthRejection;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let token = session_token(&parts.headers).ok_or(AuthRejection::Login)?;
        lookup_session(state, token)
            .await
            .map_err(AuthRejection::Error)?
            .ok_or(AuthRejection::Login)
    }
}

/// Rejects state-changing requests that a browser sent from another site.
/// Session cookies are `SameSite=Lax` too; this is the second layer.
/// Webhooks authenticate with their URL token and are exempt.
pub async fn same_origin(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let safe = matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if safe
        || request.uri().path().starts_with("/hooks/")
        || is_same_origin(request.headers(), state.public_url.as_deref())
    {
        next.run(request).await
    } else {
        AppError::Forbidden.into_response()
    }
}

fn is_same_origin(headers: &HeaderMap, public_url: Option<&str>) -> bool {
    if headers
        .get("sec-fetch-site")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|site| site == "cross-site")
    {
        return false;
    }
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
    else {
        return true;
    };
    if public_url.is_some_and(|url| url == origin) {
        return true;
    }
    let origin_host = origin.split_once("://").map(|(_, rest)| rest);
    ["x-forwarded-host", header::HOST.as_str()]
        .iter()
        .filter_map(|name| headers.get(*name).and_then(|value| value.to_str().ok()))
        .any(|host| origin_host == Some(host))
}
