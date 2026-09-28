//! Admin → Backups: download a backup, back up on a schedule, and fetch the
//! stored ones.

use axum::{
    Form, Router,
    body::{Body, Bytes},
    extract::{Path, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use tokio::io::AsyncReadExt as _;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    backup::{self, Settings, Status},
    error::{AppError, AppResult},
    now_ms,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/backups", get(page).post(save))
        .route("/admin/backups/run", post(run))
        .route("/admin/backups/download", get(download))
        .route("/admin/backups/files/{name}", get(stored))
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn render(state: &AppState, user: &CurrentUser, error: Option<&str>) -> AppResult<Markup> {
    let (settings, status) = state
        .db
        .call(|conn| Ok((Settings::load(conn)?, Status::load(conn)?)))
        .await?;
    let dir = settings.directory(&state.data_dir);
    let listed = dir.clone();
    let stored = tokio::task::spawn_blocking(move || backup::list(&listed))
        .await
        .map_err(AppError::internal)?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::admin::backups_page(
        &shell,
        &views::admin::BackupsView {
            settings: &settings,
            status: &status,
            stored: &stored,
            dir: &dir,
            error,
        },
    ))
}

async fn page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render(&state, &user, None).await
}

#[derive(Deserialize)]
struct ScheduleForm {
    every_hours: u32,
    keep: u32,
    dir: String,
    include_key: Option<String>,
}

async fn save(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<ScheduleForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let settings = Settings {
        every_hours: form.every_hours,
        keep: form.keep,
        dir: form.dir,
        include_key: form.include_key.is_some(),
    };
    match state.db.call(move |conn| settings.save(conn)).await {
        Ok(()) => Ok(Redirect::to("/admin/backups").into_response()),
        Err(AppError::BadRequest(message)) => Ok((
            StatusCode::BAD_REQUEST,
            render(&state, &user, Some(&message)).await?,
        )
            .into_response()),
        Err(other) => Err(other),
    }
}

async fn run(user: CurrentUser, State(state): State<AppState>) -> AppResult<Redirect> {
    require_admin(&user)?;
    // The outcome shows on the page as the last backup.
    drop(backup::run_scheduled(&state).await);
    Ok(Redirect::to("/admin/backups"))
}

/// Streams a file as a download and, with `unlink`, deletes its name first
/// so the space returns once the download ends.
async fn send_file(path: std::path::PathBuf, name: &str, unlink: bool) -> AppResult<Response> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let size = file.metadata().await.map_err(AppError::internal)?.len();
    if unlink {
        drop(std::fs::remove_file(&path));
    }
    let chunks = futures_util::stream::unfold(Some(file), |file| async move {
        let mut file = file?;
        let mut buffer = vec![0_u8; 64 * 1024];
        match file.read(&mut buffer).await {
            Ok(0) => None,
            Ok(read) => {
                buffer.truncate(read);
                Some((Ok::<_, std::io::Error>(Bytes::from(buffer)), Some(file)))
            }
            Err(error) => Some((Err(error), None)),
        }
    });
    let mut response = Body::from_stream(chunks).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/gzip"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(size));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{name}\""))
            .map_err(AppError::internal)?,
    );
    Ok(response)
}

#[derive(Deserialize)]
struct DownloadQuery {
    key: Option<String>,
}

async fn download(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<DownloadQuery>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let name = backup::file_name(now_ms());
    let path = state
        .data_dir
        .join(format!(".download-{}", crate::auth::random_token()?));
    backup::write_to(&state, path.clone(), query.key.is_some()).await?;
    send_file(path, &name, true).await
}

async fn stored(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Response> {
    require_admin(&user)?;
    if !backup::is_backup_name(&name) {
        return Err(AppError::NotFound);
    }
    let settings = state.db.call(|conn| Settings::load(conn)).await?;
    let path = settings.directory(&state.data_dir).join(&name);
    send_file(path, &name, false).await
}
