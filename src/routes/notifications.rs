//! Pauses and quiet hours, from the person's own notification settings.

use axum::{
    Form, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::AppResult,
    later, notifications, now_ms, store,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings/notifications", get(page))
        .route("/settings/notifications/pause", post(pause))
        .route("/settings/notifications/quiet-hours", post(quiet_hours))
}

#[derive(Deserialize)]
struct SavedQuery {
    saved: Option<String>,
}

async fn render(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (schedule, zone, paused) = state
        .db
        .call(move |conn| {
            let schedule = store::notification_schedule(conn, user_id)?;
            let zone = store::user_timezone(conn, user_id)?;
            let paused = notifications::paused_label(conn, user_id, now_ms())?;
            Ok((schedule, zone, paused))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::notifications::page(
        &shell,
        &views::notifications::Status {
            quiet: schedule.quiet,
            paused: paused.as_deref(),
            zone: &zone,
        },
        error,
        saved,
    ))
}

async fn page(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<SavedQuery>,
) -> AppResult<Markup> {
    render(&state, &user, None, query.saved.is_some()).await
}

#[derive(Deserialize)]
struct PauseForm {
    duration: String,
}

async fn pause(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<PauseForm>,
) -> AppResult<Response> {
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let zone = later::zone(&store::user_timezone(conn, user_id)?);
            let until = notifications::pause_until(&form.duration, now, &zone)?;
            store::set_notifications_paused_until(conn, user_id, until)
        })
        .await?;
    Ok(Redirect::to("/settings/notifications?saved=1").into_response())
}

async fn quiet_hours(
    user: CurrentUser,
    State(state): State<AppState>,
    // Read as pairs: a single `days` value doesn't deserialize into a list.
    Form(fields): Form<Vec<(String, String)>>,
) -> AppResult<Response> {
    let value = |name: &str| {
        fields
            .iter()
            .rev()
            .find(|(key, _)| key == name)
            .map_or("", |(_, value)| value.as_str())
    };
    let days: Vec<String> = fields
        .iter()
        .filter(|(key, _)| key == "days")
        .map(|(_, day)| day.clone())
        .collect();
    let enabled = fields.iter().any(|(key, _)| key == "enabled");
    let quiet = match notifications::quiet_from_form(enabled, value("start"), value("end"), &days) {
        Ok(quiet) => quiet,
        Err(message) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                render(&state, &user, Some(message), false).await?,
            )
                .into_response());
        }
    };
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::set_quiet_hours(conn, user_id, quiet))
        .await?;
    Ok(Redirect::to("/settings/notifications?saved=1").into_response())
}
