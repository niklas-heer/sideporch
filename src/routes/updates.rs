//! Admin → Updates: new releases, installing them, and the reminder.

use std::sync::Arc;

use axum::{
    Form, Router,
    extract::State,
    http::HeaderMap,
    response::Redirect,
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    now_ms,
    updates::{AutoInstall, Settings},
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/updates", get(page))
        .route("/admin/updates/check", post(check))
        .route("/admin/updates/install", post(install))
        .route("/admin/updates/settings", post(save_settings))
        .route("/updates/hide", post(hide))
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let settings = state.db.call(|conn| Settings::load(conn)).await?;
    let sidebar = shell_data(&state, user.id).await?;
    let updates = &state.updates;
    let status = updates.status();
    let cannot_install = updates.cannot_install();
    Ok(views::updates::updates_page(
        &Shell {
            user: &user,
            sidebar: &sidebar,
            current: None,
        },
        &views::updates::UpdatesView {
            status: &status,
            settings,
            method: updates.method(),
            program: updates.executable(),
            cannot_install: cannot_install.as_deref(),
            allowed: updates.allowed(),
            restarts: updates.restarts(),
        },
    ))
}

async fn check(user: CurrentUser, State(state): State<AppState>) -> AppResult<Redirect> {
    require_admin(&user)?;
    if state.updates.allowed() {
        // A failed check shows on the page.
        drop(state.updates.check(&state).await);
    }
    Ok(Redirect::to("/admin/updates"))
}

async fn install(user: CurrentUser, State(state): State<AppState>) -> AppResult<Redirect> {
    require_admin(&user)?;
    let updates = Arc::clone(&state.updates);
    if let Some(latest) = updates
        .status()
        .newer
        .first()
        .map(|release| release.version)
    {
        // The outcome shows on the page.
        drop(updates.install(latest).await);
    }
    Ok(Redirect::to("/admin/updates"))
}

#[derive(Deserialize)]
struct SettingsForm {
    check: Option<String>,
    install: String,
}

async fn save_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<SettingsForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let settings = Settings {
        check: form.check.is_some(),
        install: AutoInstall::parse(&form.install)
            .ok_or_else(|| AppError::bad_request("Choose what installs by itself."))?,
    };
    state.db.call(move |conn| settings.save(conn)).await?;
    Ok(Redirect::to("/admin/updates"))
}

/// Hides the reminder, then goes back to the page it was on.
async fn hide(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let updates = Arc::clone(&state.updates);
    let user_id = user.id;
    state
        .db
        .call(move |conn| updates.hide(conn, user_id, now_ms()))
        .await?;
    let back = headers
        .get(axum::http::header::REFERER)
        .and_then(|value| value.to_str().ok())
        .and_then(|referer| referer.split_once("://").map(|(_, rest)| rest))
        .and_then(|rest| rest.find('/').and_then(|at| rest.get(at..)))
        .and_then(crate::auth::safe_next)
        .unwrap_or("/")
        .to_owned();
    Ok(Redirect::to(&back))
}
