//! Profiles: a page for each person, and settings for your own.

use axum::{
    Form, Router,
    extract::{Multipart, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect, Response},
    routing::get,
};
use maud::Markup;
use rusqlite::OptionalExtension as _;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    files::{self, Upload},
    markup,
    store::{self, ProfileEdit, ProfileLink},
    views::{self, Shell},
};

/// Largest profile picture.
const MAX_AVATAR_BYTES: usize = 2 * 1024 * 1024;
const MAX_LINKS: usize = 5;
pub const MAX_FAVORITES: usize = 12;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/people/{user_id}", get(profile))
        .route("/settings/profile", get(edit).post(save))
        .route(
            "/settings/appearance",
            get(appearance).post(save_appearance),
        )
}

async fn render_appearance(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let user_id = user.id;
    let ((theme, mode), (default_theme, default_mode), reading) = state
        .db
        .call(move |conn| {
            let reading: (String, f64) = conn.query_row(
                "SELECT speech_voice, speech_speed FROM users WHERE id = ?1",
                [user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((
                store::user_appearance(conn, user_id)?,
                crate::themes::defaults(conn)?,
                reading,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::appearance::appearance_page(
        &shell,
        &views::appearance::Picker {
            action: "/settings/appearance",
            theme: &theme,
            appearance: &mode,
            default: Some((
                &default_theme,
                crate::themes::Appearance::parse(&default_mode).unwrap_or_default(),
            )),
        },
        user.speech.voice.then_some((reading.0.as_str(), reading.1)),
        error,
        saved,
    ))
}

async fn appearance(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<EditQuery>,
) -> AppResult<Markup> {
    render_appearance(&state, &user, None, query.saved.is_some()).await
}

#[derive(serde::Deserialize)]
struct AppearanceForm {
    #[serde(default)]
    theme: String,
    #[serde(default)]
    appearance: String,
}

async fn save_appearance(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<AppearanceForm>,
) -> AppResult<Response> {
    crate::themes::validate(&form.theme, &form.appearance)?;
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::set_user_appearance(conn, user_id, &form.theme, &form.appearance))
        .await?;
    // Reload, so the page shows the new theme.
    Ok(Redirect::to("/settings/appearance?saved=1").into_response())
}

async fn profile(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> AppResult<Markup> {
    let now = crate::now_ms();
    let (person, ctx, standing) = state
        .db
        .call(move |conn| {
            let standing = views::profile::Standing {
                roles: crate::community::role_names(conn, user_id)?,
                role_ids: crate::community::user_roles(conn, user_id)?,
                all_roles: crate::community::roles(conn)?,
                progress: crate::community::progress(conn, user_id, now)?.unwrap_or_default(),
                timed_out_until: conn
                    .query_row(
                        "SELECT muted_until FROM users WHERE id = ?1 AND muted_until > ?2",
                        rusqlite::params![user_id, now],
                        |row| row.get(0),
                    )
                    .optional()?,
            };
            Ok((
                store::user(conn, user_id)?,
                store::render_context(conn)?,
                standing,
            ))
        })
        .await?;
    let person = person.ok_or(AppError::NotFound)?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::profile::profile_page(
        &shell, &person, &ctx, &standing,
    ))
}

async fn render_edit(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (person, ctx, most_used) = state
        .db
        .call(move |conn| {
            Ok((
                store::user(conn, user_id)?.ok_or(AppError::NotFound)?,
                store::render_context(conn)?,
                store::most_used_emoji(conn, user_id, 12)?,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::profile::edit_page(
        &shell, &person, &ctx, &most_used, error, saved,
    ))
}

#[derive(serde::Deserialize)]
struct EditQuery {
    saved: Option<String>,
}

async fn edit(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<EditQuery>,
) -> AppResult<Markup> {
    render_edit(&state, &user, None, query.saved.is_some()).await
}

/// What the profile form sent.
#[derive(Default)]
struct ProfileForm {
    display_name: String,
    status_emoji: String,
    status_text: String,
    bio: String,
    link_labels: Vec<String>,
    link_urls: Vec<String>,
    favorite_emoji: String,
    avatar: Option<Upload>,
    remove_avatar: bool,
}

async fn read_form(mut form: Multipart) -> AppResult<ProfileForm> {
    let mut input = ProfileForm::default();
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|error| AppError::bad_request(error.body_text()))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "avatar" {
            let file_name = field.file_name().unwrap_or("avatar").to_owned();
            let data = field
                .bytes()
                .await
                .map_err(|error| AppError::bad_request(error.body_text()))?;
            if !data.is_empty() {
                input.avatar = Some(Upload {
                    name: file_name,
                    data: data.to_vec(),
                });
            }
            continue;
        }
        let text = field
            .text()
            .await
            .map_err(|error| AppError::bad_request(error.body_text()))?;
        match name.as_str() {
            "display_name" => input.display_name = text,
            "status_emoji" => input.status_emoji = text,
            "status_text" => input.status_text = text,
            "bio" => input.bio = text,
            "link_label" => input.link_labels.push(text),
            "link_url" => input.link_urls.push(text),
            "favorite_emoji" => input.favorite_emoji = text,
            "remove_avatar" => input.remove_avatar = true,
            _ => {}
        }
    }
    Ok(input)
}

/// A status emoji as a `:shortcode:`, from a shortcode with or without colons.
fn status_emoji(raw: &str, ctx: &markup::Context) -> Result<String, &'static str> {
    let name = raw.trim().trim_matches(':');
    if name.is_empty() {
        return Ok(String::new());
    }
    if ctx.has_emoji(name) {
        Ok(format!(":{name}:"))
    } else {
        Err("Pick the status emoji by its name, such as coffee or palm_tree.")
    }
}

/// Checks the form and turns it into an edit.
fn validate(form: &ProfileForm, ctx: &markup::Context) -> Result<ProfileEdit, String> {
    let display_name = form.display_name.trim();
    if display_name.is_empty() || display_name.chars().count() > 60 {
        return Err("Use a display name of 1 to 60 characters.".to_owned());
    }
    let status_text: String = form.status_text.trim().to_owned();
    if status_text.chars().count() > 100 {
        return Err("Keep the status under 100 characters.".to_owned());
    }
    let bio = form.bio.trim().to_owned();
    if bio.chars().count() > 1_000 {
        return Err("Keep the bio under 1,000 characters.".to_owned());
    }
    let mut links = Vec::new();
    for (label, url) in form.link_labels.iter().zip(&form.link_urls) {
        let url = url.trim();
        if url.is_empty() {
            continue;
        }
        if !markup::is_safe_url(url) {
            return Err(format!(
                "{url} is not a web address. Links start with https://."
            ));
        }
        let label = label.trim();
        let label = if label.is_empty() {
            url.split("://").nth(1).unwrap_or(url).trim_end_matches('/')
        } else {
            label
        };
        links.push(ProfileLink {
            label: label.chars().take(40).collect(),
            url: url.to_owned(),
        });
    }
    if links.len() > MAX_LINKS {
        return Err("Add at most five links.".to_owned());
    }
    let mut favorite_emoji: Vec<String> = Vec::new();
    for name in form
        .favorite_emoji
        .split(|c: char| c.is_whitespace() || c == ',')
        .map(|name| name.trim_matches(':'))
        .filter(|name| !name.is_empty())
    {
        if !ctx.has_emoji(name) {
            return Err(format!("There is no emoji named {name}."));
        }
        if !favorite_emoji.iter().any(|known| known == name) {
            favorite_emoji.push(name.to_owned());
        }
    }
    if favorite_emoji.len() > MAX_FAVORITES {
        return Err(format!("Pick at most {MAX_FAVORITES} favorite emoji."));
    }
    Ok(ProfileEdit {
        display_name: display_name.to_owned(),
        status_emoji: status_emoji(&form.status_emoji, ctx).map_err(ToOwned::to_owned)?,
        status_text,
        bio,
        links,
        favorite_emoji,
    })
}

async fn save(
    user: CurrentUser,
    State(state): State<AppState>,
    form: Multipart,
) -> AppResult<Response> {
    let form = read_form(form).await?;
    let ctx = state.db.call(|conn| store::render_context(conn)).await?;
    let edit = match validate(&form, &ctx) {
        Ok(edit) => edit,
        Err(error) => {
            let page = render_edit(&state, &user, Some(&error), false).await?;
            return Ok((StatusCode::BAD_REQUEST, page).into_response());
        }
    };
    let avatar = match form.avatar {
        Some(upload) if upload.image_type().is_none() => {
            let page = render_edit(
                &state,
                &user,
                Some("Choose a PNG, JPEG, GIF or WebP picture."),
                false,
            )
            .await?;
            return Ok((StatusCode::BAD_REQUEST, page).into_response());
        }
        Some(upload) if upload.data.len() > MAX_AVATAR_BYTES => {
            let page = render_edit(
                &state,
                &user,
                Some("Profile pictures can be at most 2 MB."),
                false,
            )
            .await?;
            return Ok((StatusCode::BAD_REQUEST, page).into_response());
        }
        Some(upload) => Some(files::store_uploads(&state, user.id, vec![upload]).await?),
        None => None,
    };
    let user_id = user.id;
    let remove = form.remove_avatar;
    state
        .db
        .call(move |conn| {
            store::update_profile(conn, user_id, &edit)?;
            if let Some(file_id) = avatar.and_then(|ids| ids.first().copied()) {
                store::set_avatar(conn, user_id, Some(file_id))?;
            } else if remove {
                store::set_avatar(conn, user_id, None)?;
            }
            Ok(())
        })
        .await?;
    Ok(Redirect::to("/settings/profile?saved=1").into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_profiles() {
        let ctx = markup::Context::default();
        let form = ProfileForm {
            display_name: " Ada ".to_owned(),
            status_emoji: ":coffee:".to_owned(),
            status_text: "Brewing".to_owned(),
            link_labels: vec![String::new(), "Blog".to_owned()],
            link_urls: vec![
                "https://example.com/".to_owned(),
                "https://blog.example.com".to_owned(),
            ],
            favorite_emoji: "tada, :eyes: tada".to_owned(),
            ..ProfileForm::default()
        };
        let edit = validate(&form, &ctx).unwrap();
        assert_eq!(edit.display_name, "Ada");
        assert_eq!(edit.status_emoji, ":coffee:");
        assert_eq!(edit.links[0].label, "example.com");
        assert_eq!(edit.favorite_emoji, ["tada", "eyes"]);
        let bad = ProfileForm {
            display_name: "Ada".to_owned(),
            link_urls: vec!["javascript:alert(1)".to_owned()],
            link_labels: vec![String::new()],
            ..ProfileForm::default()
        };
        assert!(validate(&bad, &ctx).is_err());
    }
}
