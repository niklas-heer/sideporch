//! The admin area: system resources and GIF search settings, plus the GIF
//! search endpoint the composer uses.

use axum::{
    Form, Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use serde_json::{Value, json};

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    gifs, store, system,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/system", get(system_page))
        .route("/admin/gifs", get(gifs_page).post(save_gifs))
        .route("/admin/gifs/remove", post(remove_gifs))
        .route("/gifs", get(search_gifs))
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn system_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let data_dir = state.data_dir.clone();
    let blobs = state.blobs.clone();
    let snapshot = tokio::task::spawn_blocking(move || system::snapshot(&data_dir, &blobs))
        .await
        .map_err(AppError::internal)?;
    let counts = state.db.call(|conn| store::counts(conn)).await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    let running = state.automations.triggers().len();
    Ok(views::admin::system_page(
        &shell,
        &views::admin::SystemView {
            samples: &state.monitor.samples(),
            snapshot: &snapshot,
            counts: &counts,
            started_at: state.monitor.started_at(),
            running_automations: running,
            online: state.hub.online_count(),
        },
    ))
}

async fn render_gifs(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let vault = std::sync::Arc::clone(&state.vault);
    let (sidebar, settings) = tokio::try_join!(
        shell_data(state, user.id),
        state.db.call(move |conn| gifs::settings(conn, &vault)),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::admin::gifs_page(
        &shell,
        settings.as_ref(),
        error,
        saved,
    ))
}

async fn gifs_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_gifs(&state, &user, None, false).await
}

#[derive(Deserialize)]
struct GifsForm {
    #[serde(default)]
    api_key: String,
    rating: String,
}

async fn save_gifs(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<GifsForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let vault = std::sync::Arc::clone(&state.vault);
    let key = form.api_key.trim().to_owned();
    let result = state
        .db
        .call(move |conn| {
            let key = (!key.is_empty()).then_some(key.as_str());
            match gifs::save(conn, &vault, key, &form.rating) {
                Ok(()) => Ok(Ok(())),
                Err(AppError::BadRequest(message)) => Ok(Err(message)),
                Err(other) => Err(other),
            }
        })
        .await?;
    match result {
        Ok(()) => Ok(render_gifs(&state, &user, None, true)
            .await?
            .into_response()),
        Err(error) => Ok((
            StatusCode::BAD_REQUEST,
            render_gifs(&state, &user, Some(&error), false).await?,
        )
            .into_response()),
    }
}

async fn remove_gifs(user: CurrentUser, State(state): State<AppState>) -> AppResult<Response> {
    require_admin(&user)?;
    state.db.call(|conn| gifs::remove(conn)).await?;
    Ok(Redirect::to("/admin/gifs").into_response())
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: u32,
}

/// GIF search for the composer's picker. An empty query shows trending GIFs.
async fn search_gifs(
    _: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Value>> {
    let vault = std::sync::Arc::clone(&state.vault);
    let settings = state
        .db
        .call(move |conn| gifs::settings(conn, &vault))
        .await?
        .ok_or_else(|| AppError::bad_request("GIF search is not set up here."))?;
    let results = state.gifs.search(&settings, &query.q, query.offset).await?;
    Ok(Json(
        json!({ "results": results, "attribution": "Powered by GIPHY" }),
    ))
}
