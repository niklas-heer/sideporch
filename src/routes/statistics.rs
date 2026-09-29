//! The statistics page.

use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use maud::Markup;
use serde::Deserialize;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    community::Permission,
    error::AppResult,
    later,
    statistics::{self, Period},
    store,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/statistics", get(page))
}

#[derive(Deserialize)]
struct PeriodQuery {
    period: Option<String>,
}

async fn page(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<PeriodQuery>,
) -> AppResult<Markup> {
    user.require(Permission::ViewStatistics)?;
    let period = query
        .period
        .as_deref()
        .and_then(Period::from_key)
        .unwrap_or_default();
    let viewer = user.id;
    let (report, ctx) = state
        .db
        .call(move |conn| {
            let zone = later::now_for(conn, viewer)?.time_zone().clone();
            Ok((
                statistics::report(conn, period, &zone, viewer)?,
                store::render_context(conn)?,
            ))
        })
        .await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::statistics::page(&shell, &report, &ctx))
}
