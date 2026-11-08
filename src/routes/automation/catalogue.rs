//! The automation store: browsing the catalogue, and previewing an entry
//! before it is installed.

use axum::{
    Form, Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use super::{require_admin, sharing};
use crate::{
    AppState,
    auth::CurrentUser,
    automations::{self, bundle, catalogue, http},
    error::AppResult,
    routes::shell_data,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/automations/browse", get(browse))
        .route("/automations/browse/install", post(install))
}

async fn browse(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    page(&state, &user, None).await
}

/// The store page, with `notice` above the list when there is one.
async fn page(state: &AppState, user: &CurrentUser, notice: Option<&str>) -> AppResult<Markup> {
    let settings = state
        .db
        .call(|conn| automations::Settings::load(conn))
        .await?;
    let entries = catalogue::index(state, &settings).await;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::automations::browse_page(
        &shell,
        settings.catalogue(),
        &entries,
        notice,
    ))
}

#[derive(Deserialize)]
struct InstallForm {
    file: String,
}

/// Downloads a listed automation and shows the import preview. The file
/// must be one the store lists, and the checks come from the store's entry.
async fn install(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<InstallForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let settings = state
        .db
        .call(|conn| automations::Settings::load(conn))
        .await?;
    let entries = match catalogue::index(&state, &settings).await {
        Ok(entries) => entries,
        Err(error) => {
            let notice = format!("Nothing was installed. {error}");
            return Ok(page(&state, &user, Some(&notice)).await?.into_response());
        }
    };
    let Some(entry) = entries.iter().find(|entry| entry.file == form.file) else {
        let notice = "Nothing was installed: that automation isn't in the store any more.";
        return Ok(page(&state, &user, Some(notice)).await?.into_response());
    };
    match http::download(
        settings.allow_private_network,
        &entry.file,
        bundle::MAX_BYTES,
    )
    .await
    {
        Ok(body) => {
            let text = String::from_utf8_lossy(&body).into_owned();
            sharing::show_preview(&state, &user, &text, Some(&entry.file)).await
        }
        Err(error) => {
            let notice = format!(
                "Nothing was installed. Couldn't download {}: {error}.",
                entry.name
            );
            Ok(page(&state, &user, Some(&notice)).await?.into_response())
        }
    }
}
