//! The admin's system page and message settings.

use axum::{Form, Router, extract::State, routing::get};
use maud::Markup;
use serde::Deserialize;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    store, system,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/system", get(system_page))
        .route("/admin/messages", get(messages_page).post(save_messages))
        .route(
            "/admin/appearance",
            get(appearance_page).post(save_appearance),
        )
        .route("/admin/demo", get(demo_page).post(save_demo))
        .route("/admin/demo/reset", axum::routing::post(reset_demo))
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

async fn render_messages(state: &AppState, user: &CurrentUser, saved: bool) -> AppResult<Markup> {
    let (previews, edit_window) = state
        .db
        .call(|conn| {
            Ok((
                crate::previews::enabled(conn)?,
                crate::messages::edit_window(conn)?,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::admin::messages_page(
        &shell,
        previews,
        edit_window.unwrap_or(0),
        saved,
    ))
}

async fn messages_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_messages(&state, &user, false).await
}

#[derive(Deserialize)]
struct MessagesForm {
    previews: Option<String>,
    #[serde(default)]
    edit_minutes: i64,
}

async fn save_messages(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<MessagesForm>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    let previews = form.previews.is_some();
    state
        .db
        .call(move |conn| {
            crate::messages::set_edit_window(conn, form.edit_minutes)?;
            crate::previews::set_enabled(conn, previews)
        })
        .await?;
    render_messages(&state, &user, true).await
}

async fn render_appearance(state: &AppState, user: &CurrentUser, saved: bool) -> AppResult<Markup> {
    let (theme, mode) = state.db.call(|conn| crate::themes::defaults(conn)).await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::appearance::default_page(
        &shell,
        &views::admin::tabs("/admin/appearance"),
        &views::appearance::Picker {
            action: "/admin/appearance",
            theme: &theme,
            appearance: &mode,
            default: None,
        },
        saved,
    ))
}

#[derive(serde::Deserialize, Default)]
struct SavedQuery {
    saved: Option<String>,
}

async fn appearance_page(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<SavedQuery>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    render_appearance(&state, &user, query.saved.is_some()).await
}

#[derive(Deserialize)]
struct AppearanceForm {
    theme: String,
    appearance: String,
}

async fn save_appearance(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<AppearanceForm>,
) -> AppResult<axum::response::Redirect> {
    require_admin(&user)?;
    if form.theme.is_empty() || form.appearance.is_empty() {
        return Err(AppError::bad_request("Pick a theme and a mode."));
    }
    state
        .db
        .call(move |conn| crate::themes::set_defaults(conn, &form.theme, &form.appearance))
        .await?;
    Ok(axum::response::Redirect::to("/admin/appearance?saved=1"))
}

async fn render_demo(
    state: &AppState,
    user: &CurrentUser,
    notice: Option<&str>,
) -> AppResult<Markup> {
    let (demo, channels, last_reset) = state
        .db
        .call(|conn| {
            let mut statement = conn.prepare(
                "SELECT id, name, private, kept FROM channels WHERE kind = 'public' ORDER BY name COLLATE NOCASE",
            )?;
            let channels = statement
                .query_map([], |row| {
                    Ok(views::admin::DemoChannel {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        private: row.get(2)?,
                        kept: row.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                crate::demo::Demo::load(conn)?,
                channels,
                store::setting(conn, "demo.last_reset")?,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::admin::demo_page(
        &shell,
        demo,
        &channels,
        last_reset.as_deref(),
        notice,
    ))
}

async fn demo_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_demo(&state, &user, None).await
}

async fn save_demo(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::RawForm(form): axum::extract::RawForm,
) -> AppResult<Markup> {
    require_admin(&user)?;
    // `keep` repeats, once per ticked channel.
    let pairs: Vec<(String, String)> = form
        .split(|byte| *byte == b'&')
        .filter_map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            let (key, value) = pair.split_once('=')?;
            Some((key.to_owned(), value.to_owned()))
        })
        .collect();
    let demo = crate::demo::Demo {
        enabled: pairs.iter().any(|(key, _)| key == "enabled"),
        hour: pairs
            .iter()
            .find(|(key, _)| key == "hour")
            .and_then(|(_, hour)| hour.parse().ok())
            .filter(|hour| *hour < 24)
            .unwrap_or(4),
    };
    let kept: Vec<i64> = pairs
        .iter()
        .filter(|(key, _)| key == "keep")
        .filter_map(|(_, id)| id.parse().ok())
        .collect();
    state
        .db
        .call(move |conn| {
            demo.save(conn)?;
            crate::demo::set_kept(conn, &kept)
        })
        .await?;
    render_demo(&state, &user, Some("Saved.")).await
}

async fn reset_demo(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let removed = crate::demo::reset_now(&state).await?;
    let notice = format!(
        "Started over: {} people, {} channels and {} messages are gone.",
        removed.people, removed.channels, removed.messages
    );
    render_demo(&state, &user, Some(&notice)).await
}
