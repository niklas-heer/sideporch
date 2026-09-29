//! GIFs: the picker's search, the team's library, and the admin's choice
//! of source.

use axum::{
    Form, Json, Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
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
    files::{self, Upload},
    gifs::{self, Found, Provider},
    now_ms,
    store::{self, Gif, NewLibraryGif},
    views::{self, Shell},
};

/// Largest GIF in the library.
const MAX_GIF_BYTES: usize = 10 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/gifs", get(search))
        .route(
            "/gifs/library",
            get(library)
                .post(add)
                .layer(DefaultBodyLimit::max(files::UPLOAD_BODY_LIMIT)),
        )
        .route("/gifs/library/{gif_id}/delete", post(delete))
        .route("/admin/gifs", get(settings_page).post(save_settings))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    offset: u32,
}

/// The picker's search for the local library and GIPHY. KLIPY is searched
/// from the browser, as its terms require.
async fn search(
    _: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> AppResult<Json<Value>> {
    let vault = std::sync::Arc::clone(&state.vault);
    let settings = state
        .db
        .call(move |conn| gifs::settings(conn, &vault))
        .await?;
    let results: Vec<Found> = match settings.provider {
        Provider::Local => {
            let q = query.q.clone();
            state
                .db
                .call(move |conn| store::library_gifs(conn, &q, 48))
                .await?
                .into_iter()
                .map(|gif| Found {
                    id: gif.id.to_string(),
                    title: gif.title,
                    preview: format!("/files/{}", gif.file_id),
                    width: gif.width,
                    height: gif.height,
                })
                .collect()
        }
        Provider::Giphy => state.gifs.search(&settings, &query.q, query.offset).await?,
        Provider::Klipy => {
            return Err(AppError::bad_request("KLIPY is searched from the browser."));
        }
    };
    Ok(Json(json!({
        "results": results,
        "attribution": settings.provider.attribution(),
    })))
}

/// What the composer sends for a GIF.
pub struct Posted {
    pub id: String,
    /// KLIPY only: the browser found the GIF, so it sends the media too.
    pub url: Option<String>,
    pub title: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Turns a posted GIF into what the message keeps, checking it against the
/// active source.
pub async fn resolve(state: &AppState, posted: Posted) -> AppResult<Gif> {
    let vault = std::sync::Arc::clone(&state.vault);
    let settings = state
        .db
        .call(move |conn| gifs::settings(conn, &vault))
        .await?;
    match settings.provider {
        Provider::Local => {
            let id: i64 = posted
                .id
                .parse()
                .map_err(|_| AppError::bad_request("That GIF is not in the library."))?;
            let gif = state
                .db
                .call(move |conn| {
                    let gif = store::library_gif(conn, id)?
                        .ok_or_else(|| AppError::bad_request("That GIF is not in the library."))?;
                    store::count_gif_use(conn, id)?;
                    Ok(gif)
                })
                .await?;
            Ok(Gif {
                provider: "local".to_owned(),
                id: gif.id.to_string(),
                title: gif.title,
                url: format!("/files/{}", gif.file_id),
                width: gif.width,
                height: gif.height,
            })
        }
        Provider::Giphy => state.gifs.gif(&settings, &posted.id).await,
        Provider::Klipy => {
            let url = posted.url.unwrap_or_default();
            if !gifs::is_klipy_media(&url) {
                return Err(AppError::bad_request("That GIF is not from KLIPY."));
            }
            Ok(Gif {
                provider: "klipy".to_owned(),
                id: posted.id.chars().take(64).collect(),
                title: posted
                    .title
                    .unwrap_or_else(|| "GIF".to_owned())
                    .chars()
                    .take(200)
                    .collect(),
                url,
                width: posted.width.unwrap_or(0).min(4_096),
                height: posted.height.unwrap_or(0).min(4_096),
            })
        }
    }
}

async fn render_library(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
) -> AppResult<Markup> {
    let (gifs, sidebar) = tokio::try_join!(
        state.db.call(|conn| store::library_gifs(conn, "", 500)),
        shell_data(state, user.id),
    )?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::gifs::library_page(&shell, &gifs, error))
}

async fn library(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    render_library(&state, &user, None).await
}

async fn add(
    user: CurrentUser,
    State(state): State<AppState>,
    mut form: Multipart,
) -> AppResult<Response> {
    user.require(crate::community::Permission::UploadFiles)?;
    let mut title = String::new();
    let mut tags = String::new();
    let mut upload: Option<Upload> = None;
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|error| AppError::bad_request(error.body_text()))?
    {
        match field.name().unwrap_or_default() {
            "title" => {
                title = field
                    .text()
                    .await
                    .map_err(|error| AppError::bad_request(error.body_text()))?;
            }
            "tags" => {
                tags = field
                    .text()
                    .await
                    .map_err(|error| AppError::bad_request(error.body_text()))?;
            }
            "file" => {
                let name = files::clean_file_name(field.file_name().unwrap_or("gif"));
                let data = field
                    .bytes()
                    .await
                    .map_err(|error| AppError::bad_request(error.body_text()))?;
                if !data.is_empty() {
                    upload = Some(Upload {
                        name,
                        data: data.to_vec(),
                    });
                }
            }
            _ => {}
        }
    }
    let title = title.trim().chars().take(100).collect::<String>();
    let tags = tags
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|tag| !tag.is_empty())
        .map(|tag| tag.trim_start_matches('#').to_lowercase())
        .take(20)
        .collect::<Vec<_>>()
        .join(" ");
    let problem = match &upload {
        None => Some("Choose a GIF to add."),
        Some(upload) if upload.image_type().is_none() => {
            Some("Add a GIF, WebP, PNG or JPEG image.")
        }
        Some(upload) if upload.data.len() > MAX_GIF_BYTES => Some("GIFs can be at most 10 MB."),
        Some(_) if title.is_empty() => Some("Give the GIF a title, so people can find it."),
        Some(_) => None,
    };
    let (Some(upload), None) = (upload, problem) else {
        let page = render_library(&state, &user, problem).await?;
        return Ok((StatusCode::BAD_REQUEST, page).into_response());
    };
    let (width, height) = upload.image_size().unwrap_or((0, 0));
    let file_ids = files::store_uploads(&state, user.id, vec![upload]).await?;
    let file_id = *file_ids
        .first()
        .ok_or_else(|| AppError::internal("the GIF was not stored"))?;
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            store::add_library_gif(
                conn,
                &NewLibraryGif {
                    file_id,
                    title: &title,
                    tags: &tags,
                    width,
                    height,
                },
                user_id,
                now,
            )
        })
        .await?;
    Ok(Redirect::to("/gifs/library").into_response())
}

async fn delete(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(gif_id): Path<i64>,
) -> AppResult<Response> {
    let user_id = user.id;
    let is_admin = user.is_admin;
    state
        .db
        .call(move |conn| {
            let gif = store::library_gif(conn, gif_id)?.ok_or(AppError::NotFound)?;
            if !is_admin && gif.added_by != Some(user_id) {
                return Err(AppError::Forbidden);
            }
            store::delete_library_gif(conn, gif_id)
        })
        .await?;
    Ok(Redirect::to("/gifs/library").into_response())
}

async fn render_settings(
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
    Ok(views::admin::gifs_page(&shell, &settings, error, saved))
}

async fn settings_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    render_settings(&state, &user, None, false).await
}

#[derive(Deserialize)]
struct SettingsForm {
    provider: String,
    #[serde(default)]
    giphy_key: String,
    #[serde(default)]
    klipy_key: String,
    rating: String,
}

async fn save_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<SettingsForm>,
) -> AppResult<Response> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    let vault = std::sync::Arc::clone(&state.vault);
    let result = state
        .db
        .call(move |conn| {
            let provider = Provider::from_key(&form.provider)
                .ok_or_else(|| AppError::bad_request("Pick where GIFs come from."))?;
            let key = |value: &str| {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            };
            let (giphy_key, klipy_key) = (key(&form.giphy_key), key(&form.klipy_key));
            match gifs::save(
                conn,
                &vault,
                &gifs::Change {
                    provider,
                    giphy_key: giphy_key.as_deref(),
                    klipy_key: klipy_key.as_deref(),
                    rating: &form.rating,
                },
            ) {
                Ok(()) => Ok(Ok(())),
                Err(AppError::BadRequest(message)) => Ok(Err(message)),
                Err(other) => Err(other),
            }
        })
        .await;
    let result = match result {
        Err(AppError::BadRequest(message)) => Err(message),
        other => other?,
    };
    match result {
        Ok(()) => Ok(render_settings(&state, &user, None, true)
            .await?
            .into_response()),
        Err(error) => Ok((
            StatusCode::BAD_REQUEST,
            render_settings(&state, &user, Some(&error), false).await?,
        )
            .into_response()),
    }
}
