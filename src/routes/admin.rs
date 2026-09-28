//! The admin's system page.

use axum::{Router, extract::State, routing::get};
use maud::Markup;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    store, system,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/admin/system", get(system_page))
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
