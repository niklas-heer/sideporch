//! The statistics page.

use std::{sync::Arc, time::Instant};

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
    let zone = state
        .db
        .call(move |conn| Ok(later::now_for(conn, viewer)?.time_zone().clone()))
        .await?;
    let zone_name = zone.iana_name().unwrap_or("UTC").to_owned();
    let summary = if let Some(summary) = state.statistics.get(period, &zone_name) {
        summary
    } else {
        let started = Instant::now();
        let summary = Arc::new(
            state
                .db
                .read(move |conn| statistics::summary(conn, period, &zone))
                .await?,
        );
        state
            .statistics
            .put(period, &zone_name, Arc::clone(&summary), started.elapsed());
        summary
    };
    let shared = Arc::clone(&summary);
    let (you, ctx) = state
        .db
        .read(move |conn| {
            Ok((
                statistics::standing(conn, &shared, viewer)?,
                store::render_context(conn)?,
            ))
        })
        .await?;
    let report = statistics::Report { summary, you };
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::statistics::page(&shell, &report, &ctx))
}
