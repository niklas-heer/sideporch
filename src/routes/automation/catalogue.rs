//! The automation store: browsing the catalogue, and previewing an entry
//! before it is installed.

use axum::{
    Form, Router,
    extract::State,
    http::StatusCode,
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
        return refuse(
            &state,
            &user,
            "Nothing was installed: that automation isn't in the store any more.",
        )
        .await;
    };
    let allow = settings.allow_private_network;
    let body = match http::download(allow, &entry.file, bundle::MAX_BYTES).await {
        Ok(body) => body,
        Err(error) => {
            let notice = format!(
                "Nothing was installed. Couldn't download {}: {error}.",
                entry.name
            );
            return refuse(&state, &user, &notice).await;
        }
    };
    // The checksum is the store's; a file that doesn't match it is refused.
    if catalogue::sha256_hex(&body) != entry.sha256 {
        let notice = format!(
            "Nothing was installed. The file for {} doesn't match the checksum the store lists for it, so it may have changed on the way.",
            entry.name
        );
        return refuse(&state, &user, &notice).await;
    }
    let signed = match &entry.signature {
        None => false,
        Some(address) => {
            let signature = match http::download(allow, address, catalogue::MAX_SIGNATURE_BYTES)
                .await
            {
                Ok(signature) => String::from_utf8_lossy(&signature).into_owned(),
                Err(error) => {
                    let notice = format!(
                        "Nothing was installed. Couldn't download the signature for {}: {error}.",
                        entry.name
                    );
                    return refuse(&state, &user, &notice).await;
                }
            };
            if let Err(error) = catalogue::check_signature(
                state.automations.signing_key(),
                &body,
                &entry.sha256,
                &signature,
            ) {
                let notice = format!(
                    "Nothing was installed. The signature for {} doesn't check out: {error}.",
                    entry.name
                );
                return refuse(&state, &user, &notice).await;
            }
            true
        }
    };
    let text = String::from_utf8_lossy(&body).into_owned();
    let origin = views::automations::Origin {
        url: &entry.file,
        signed,
    };
    sharing::show_preview(&state, &user, &text, Some(&origin)).await
}

/// The store page with `notice`, and a 400 status: nothing was installed.
async fn refuse(state: &AppState, user: &CurrentUser, notice: &str) -> AppResult<Response> {
    let page = page(state, user, Some(notice)).await?;
    Ok((StatusCode::BAD_REQUEST, page).into_response())
}
