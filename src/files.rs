//! File uploads, downloads and custom emoji.
//!
//! File contents live on disk ([`crate::blobs`]), their metadata in the
//! database. Only PNG, JPEG, GIF and WebP images,
//! recognised by their first bytes rather than the browser's claim, are
//! shown inline; everything else downloads as an attachment.

use axum::{
    Form,
    extract::{FromRequest, Multipart, Path, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use maud::Markup;
use serde::Deserialize;

use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    now_ms,
    routes::shell_data,
    store, views,
};

pub const MAX_FILE_BYTES: usize = 25 * 1024 * 1024;
pub const MAX_FILES: usize = 10;
pub const MAX_EMOJI_BYTES: usize = 256 * 1024;
/// Request body limit for routes that accept uploads.
pub const UPLOAD_BODY_LIMIT: usize = 64 * 1024 * 1024;

pub struct Upload {
    pub name: String,
    pub data: Vec<u8>,
}

impl Upload {
    /// The image type from the file's first bytes, if it is a safe image.
    pub fn image_type(&self) -> Option<&'static str> {
        let data = self.data.as_slice();
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some("image/png")
        } else if data.starts_with(b"\xff\xd8\xff") {
            Some("image/jpeg")
        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            Some("image/gif")
        } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            Some("image/webp")
        } else {
            None
        }
    }

    pub fn mime(&self) -> &'static str {
        self.image_type().unwrap_or("application/octet-stream")
    }
}

/// Keeps the last path component, drops control characters, and caps the length.
fn clean_file_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let name: String = base.chars().filter(|c| !c.is_control()).take(200).collect();
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        "file".to_owned()
    } else {
        name.to_owned()
    }
}

/// A message form, sent either as a plain form or as multipart with files.
pub struct MessageInput {
    pub body: String,
    pub parent_id: Option<i64>,
    pub files: Vec<Upload>,
    /// A GIF id from the GIF picker.
    pub gif: Option<String>,
}

#[derive(Deserialize)]
struct PlainMessage {
    #[serde(default)]
    body: String,
    parent_id: Option<i64>,
    gif: Option<String>,
}

impl FromRequest<AppState> for MessageInput {
    type Rejection = AppError;

    async fn from_request(request: Request, state: &AppState) -> Result<Self, Self::Rejection> {
        let multipart = request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("multipart/form-data"));
        if !multipart {
            let Form(plain) = Form::<PlainMessage>::from_request(request, state)
                .await
                .map_err(|rejection| AppError::bad_request(rejection.body_text()))?;
            return Ok(Self {
                body: plain.body,
                parent_id: plain.parent_id,
                files: Vec::new(),
                gif: plain.gif.filter(|gif| !gif.is_empty()),
            });
        }
        let mut form = Multipart::from_request(request, state)
            .await
            .map_err(|rejection| AppError::bad_request(rejection.body_text()))?;
        let mut input = Self {
            body: String::new(),
            parent_id: None,
            files: Vec::new(),
            gif: None,
        };
        while let Some(field) = form.next_field().await.map_err(bad_upload)? {
            match field.name().unwrap_or_default() {
                "body" => input.body = field.text().await.map_err(bad_upload)?,
                "parent_id" => {
                    let text = field.text().await.map_err(bad_upload)?;
                    input.parent_id = text.trim().parse().ok();
                }
                "files" => {
                    let name = clean_file_name(field.file_name().unwrap_or("file"));
                    let data = field.bytes().await.map_err(bad_upload)?;
                    if data.is_empty() {
                        continue;
                    }
                    if data.len() > MAX_FILE_BYTES {
                        return Err(AppError::bad_request("Files can be at most 25 MB."));
                    }
                    if input.files.len() >= MAX_FILES {
                        return Err(AppError::bad_request("Attach at most 10 files at a time."));
                    }
                    input.files.push(Upload {
                        name,
                        data: data.to_vec(),
                    });
                }
                _ => {}
            }
        }
        Ok(input)
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "used as a `map_err` callback"
)]
fn bad_upload(error: axum::extract::multipart::MultipartError) -> AppError {
    AppError::bad_request(format!("The upload failed: {}", error.body_text()))
}

/// Stores uploads for `user_id` and returns their ids, ready to attach.
pub async fn store_uploads(
    state: &AppState,
    user_id: i64,
    uploads: Vec<Upload>,
) -> AppResult<Vec<i64>> {
    if uploads.is_empty() {
        return Ok(Vec::new());
    }
    let now = now_ms();
    let stored = write_blobs(state, uploads).await?;
    state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let ids = stored
                .iter()
                .map(|(upload, sha256)| {
                    store::insert_file(
                        &tx,
                        &store::NewFile {
                            name: &upload.name,
                            mime: upload.mime(),
                            sha256,
                            size: upload.data.len(),
                        },
                        user_id,
                        now,
                    )
                })
                .collect::<AppResult<Vec<_>>>()?;
            tx.commit()?;
            Ok(ids)
        })
        .await
}

/// Writes uploads to disk and pairs each with its hash.
pub async fn write_blobs(
    state: &AppState,
    uploads: Vec<Upload>,
) -> AppResult<Vec<(Upload, String)>> {
    let blobs = state.blobs.clone();
    tokio::task::spawn_blocking(move || {
        uploads
            .into_iter()
            .map(|upload| {
                let hash = blobs.put(&upload.data)?;
                Ok((upload, hash))
            })
            .collect::<AppResult<Vec<_>>>()
    })
    .await
    .map_err(AppError::internal)?
}

/// Serves a file to someone who may see it.
pub async fn download(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(file_id): Path<i64>,
) -> AppResult<Response> {
    let user_id = user.id;
    let file = state
        .db
        .call(move |conn| store::readable_file(conn, file_id, user_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let inline = file.mime.starts_with("image/");
    let disposition = format!(
        "{}; filename*=UTF-8''{}",
        if inline { "inline" } else { "attachment" },
        percent_encode(&file.name)
    );
    let data = state.blobs.read(&file.sha256).await?;
    let mut response = (StatusCode::OK, data).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&file.mime)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition).map_err(AppError::internal)?,
    );
    // A file's bytes never change, but it must stay private to this browser.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    Ok(response)
}

fn percent_encode(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

// Custom emoji

pub async fn emoji_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    render_emoji_page(&state, &user, None).await
}

async fn render_emoji_page(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
) -> AppResult<Markup> {
    let emoji = state.db.call(|conn| store::custom_emoji(conn)).await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = views::Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::emoji::page(&shell, &emoji, error))
}

fn valid_emoji_name(name: &str) -> bool {
    (1..=32).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-' | '+'))
}

pub async fn add_emoji(
    user: CurrentUser,
    State(state): State<AppState>,
    mut form: Multipart,
) -> AppResult<Response> {
    let mut name = String::new();
    let mut image: Option<Upload> = None;
    while let Some(field) = form.next_field().await.map_err(bad_upload)? {
        match field.name().unwrap_or_default() {
            "name" => {
                name = field
                    .text()
                    .await
                    .map_err(bad_upload)?
                    .trim()
                    .trim_matches(':')
                    .to_lowercase();
            }
            "image" => {
                let file_name = clean_file_name(field.file_name().unwrap_or("emoji"));
                let data = field.bytes().await.map_err(bad_upload)?;
                image = Some(Upload {
                    name: file_name,
                    data: data.to_vec(),
                });
            }
            _ => {}
        }
    }
    let error = if !valid_emoji_name(&name) {
        Some("Use 1 to 32 lowercase letters, numbers, dashes, underscores or plus signs.")
    } else if image
        .as_ref()
        .is_none_or(|image| image.image_type().is_none())
    {
        Some("Choose a PNG, JPEG, GIF or WebP image.")
    } else if image
        .as_ref()
        .is_some_and(|image| image.data.len() > MAX_EMOJI_BYTES)
    {
        Some("Emoji images can be at most 256 kB.")
    } else {
        None
    };
    if let Some(error) = error {
        return Ok((
            StatusCode::BAD_REQUEST,
            render_emoji_page(&state, &user, Some(error)).await?,
        )
            .into_response());
    }
    let Some(image) = image else {
        return Err(AppError::bad_request("Choose an image."));
    };
    let user_id = user.id;
    let now = now_ms();
    let mut written = write_blobs(&state, vec![image]).await?;
    let (image, sha256) = written
        .pop()
        .ok_or_else(|| AppError::internal("the emoji image was not stored"))?;
    let added = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let ctx = store::render_context(&tx)?;
            if ctx.has_emoji(&name) {
                return Ok(false);
            }
            let file_id = store::insert_file(
                &tx,
                &store::NewFile {
                    name: &image.name,
                    mime: image.mime(),
                    sha256: &sha256,
                    size: image.data.len(),
                },
                user_id,
                now,
            )?;
            store::add_custom_emoji(&tx, &name, file_id, user_id, now)?;
            tx.commit()?;
            Ok(true)
        })
        .await?;
    if !added {
        let page = render_emoji_page(
            &state,
            &user,
            Some("An emoji with that name already exists."),
        )
        .await?;
        return Ok((StatusCode::BAD_REQUEST, page).into_response());
    }
    Ok(Redirect::to("/emoji").into_response())
}

pub async fn delete_emoji(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> AppResult<Response> {
    let user_id = user.id;
    let is_admin = user.is_admin;
    state
        .db
        .call(move |conn| {
            let allowed = store::custom_emoji(conn)?
                .into_iter()
                .any(|emoji| emoji.name == name && (is_admin || emoji.created_by == Some(user_id)));
            if !allowed {
                return Err(AppError::Forbidden);
            }
            store::delete_custom_emoji(conn, &name)
        })
        .await?;
    Ok(Redirect::to("/emoji").into_response())
}

#[cfg(test)]
mod tests {
    use super::{Upload, clean_file_name, percent_encode};

    #[test]
    fn recognises_images_by_content_only() {
        let png = Upload {
            name: "x.txt".into(),
            data: b"\x89PNG\r\n\x1a\nrest".to_vec(),
        };
        assert_eq!(png.mime(), "image/png");
        let svg = Upload {
            name: "x.png".into(),
            data: b"<svg onload=alert(1)>".to_vec(),
        };
        assert_eq!(svg.mime(), "application/octet-stream");
    }

    #[test]
    fn cleans_file_names() {
        assert_eq!(clean_file_name("C:\\Users\\me\\report.pdf"), "report.pdf");
        assert_eq!(clean_file_name("../../etc/passwd"), "passwd");
        assert_eq!(clean_file_name("..\n"), "file");
        assert_eq!(percent_encode("a b ü.txt"), "a%20b%20%C3%BC.txt");
    }
}
