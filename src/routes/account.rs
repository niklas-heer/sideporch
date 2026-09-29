//! Accounts: changing your password, reset links an admin hands out, and
//! admins deactivating people and granting admin rights.

use axum::{
    Form, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use super::{base_url, shell_data};
use crate::{
    AppState,
    auth::{self, CurrentUser},
    error::{AppError, AppResult},
    now_ms, store,
    views::{self, Shell},
};

/// How long a password reset link works.
const RESET_HOURS: i64 = 24;
const MIN_PASSWORD: usize = 8;
const MAX_PASSWORD: usize = 256;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/account", get(account).post(change_password))
        .route("/reset/{token}", get(reset_form).post(reset))
        .route("/people/{user_id}/reset-link", post(reset_link))
        .route("/people/{user_id}/deactivate", post(deactivate))
        .route("/people/{user_id}/reactivate", post(reactivate))
        .route("/people/{user_id}/admin", post(set_admin))
}

fn password_problem(password: &str) -> Option<&'static str> {
    (!(MIN_PASSWORD..=MAX_PASSWORD).contains(&password.chars().count()))
        .then_some("Choose a password with at least 8 characters.")
}

async fn render_account(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::account::account_page(&shell, error, saved))
}

async fn account(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    render_account(&state, &user, None, false).await
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    password: String,
}

/// Changes the password and signs out every other session.
async fn change_password(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<PasswordForm>,
) -> AppResult<Response> {
    let user_id = user.id;
    let hash = state
        .db
        .call(move |conn| store::password_hash(conn, user_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let problem = if auth::verify_password(form.current, hash).await? {
        password_problem(&form.password)
    } else {
        Some("Your current password isn't right.")
    };
    if let Some(problem) = problem {
        let page = render_account(&state, &user, Some(problem), false).await?;
        return Ok((StatusCode::BAD_REQUEST, page).into_response());
    }
    let hash = auth::hash_password(form.password).await?;
    let keep = auth::session_token(&headers).map(|token| auth::hash_token(&token));
    state
        .db
        .call(move |conn| {
            store::set_password(conn, user_id, &hash)?;
            store::end_sessions(conn, user_id, keep.as_deref())
        })
        .await?;
    Ok(render_account(&state, &user, None, true)
        .await?
        .into_response())
}

async fn reset_user(state: &AppState, token: &str) -> AppResult<store::User> {
    let token_hash = auth::hash_token(token);
    let now = now_ms();
    state
        .db
        .call(move |conn| store::password_reset_user(conn, &token_hash, now))
        .await?
        .ok_or_else(|| {
            AppError::Gone(
                "This reset link has expired or was already used. Ask for a new one.".to_owned(),
            )
        })
}

async fn reset_form(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Markup> {
    let person = reset_user(&state, &token).await?;
    Ok(views::account::reset_page(&token, &person, None))
}

#[derive(Deserialize)]
struct ResetForm {
    password: String,
}

/// Sets a new password from a reset link, signs out everywhere else, and
/// signs in here.
async fn reset(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(form): Form<ResetForm>,
) -> AppResult<Response> {
    let person = reset_user(&state, &token).await?;
    if let Some(problem) = password_problem(&form.password) {
        return Ok((
            StatusCode::BAD_REQUEST,
            views::account::reset_page(&token, &person, Some(problem)),
        )
            .into_response());
    }
    let hash = auth::hash_password(form.password).await?;
    let user_id = person.id;
    state
        .db
        .call(move |conn| {
            store::set_password(conn, user_id, &hash)?;
            store::end_sessions(conn, user_id, None)
        })
        .await?;
    // A reset replaces the password, not a passkey or authenticator app.
    super::security::after_first_step(&state, user_id, None).await
}

/// Loads someone an admin manages. Admins can't manage themselves here, so
/// they can't lock themselves out.
async fn managed_person(
    state: &AppState,
    admin: &CurrentUser,
    user_id: i64,
) -> AppResult<store::User> {
    if !admin.is_admin {
        return Err(AppError::Forbidden);
    }
    if admin.id == user_id {
        return Err(AppError::bad_request(
            "Ask another admin to change your own account.",
        ));
    }
    state
        .db
        .call(move |conn| store::user(conn, user_id))
        .await?
        .ok_or(AppError::NotFound)
}

async fn reset_link(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    let person = managed_person(&state, &user, user_id).await?;
    if person.deactivated {
        return Err(AppError::bad_request("Reactivate the account first."));
    }
    let token = auth::random_token()?;
    let token_hash = auth::hash_token(&token);
    let now = now_ms();
    let expires = now.saturating_add(RESET_HOURS.saturating_mul(60 * 60 * 1000));
    let admin_id = user.id;
    state
        .db
        .call(move |conn| {
            store::create_password_reset(conn, &token_hash, user_id, admin_id, now, expires)
        })
        .await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    let link = format!("{}/reset/{token}", base_url(&state, &headers));
    Ok(views::account::reset_link_page(&shell, &person, &link))
}

async fn deactivate(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> AppResult<Redirect> {
    managed_person(&state, &user, user_id).await?;
    let now = now_ms();
    state
        .db
        .call(move |conn| store::set_deactivated(conn, user_id, Some(now)))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}

async fn reactivate(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> AppResult<Redirect> {
    managed_person(&state, &user, user_id).await?;
    state
        .db
        .call(move |conn| store::set_deactivated(conn, user_id, None))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}

#[derive(Deserialize)]
struct AdminForm {
    admin: String,
}

async fn set_admin(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Form(form): Form<AdminForm>,
) -> AppResult<Redirect> {
    managed_person(&state, &user, user_id).await?;
    let admin = form.admin == "true";
    state
        .db
        .call(move |conn| store::set_admin(conn, user_id, admin))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}
