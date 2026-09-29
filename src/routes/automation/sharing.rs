//! Exporting automations to a file, and importing them from one after a
//! preview. Imported automations start switched off.

use std::collections::{BTreeSet, HashMap};

use axum::{
    Form, Router,
    extract::{Multipart, RawQuery, State},
    http::{StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};

use super::{Change, require_admin, save_change};
use crate::{
    AppState,
    auth::CurrentUser,
    automations::bundle::{self, Action, Bundle, Plan},
    error::{AppError, AppResult},
    routes::shell_data,
    secrets, store,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/automations/export", get(export))
        .route("/automations/import", get(import_form).post(preview))
        .route("/automations/import/confirm", post(confirm))
}

/// `/automations/export?id=1&id=2` exports those; without ids, everything.
async fn export(
    user: CurrentUser,
    State(state): State<AppState>,
    RawQuery(query): RawQuery,
) -> AppResult<Response> {
    require_admin(&user)?;
    let chosen: Vec<i64> = query
        .unwrap_or_default()
        .split('&')
        .filter_map(|pair| pair.strip_prefix("id="))
        .filter_map(|id| id.parse().ok())
        .collect();
    let all = state.db.call(|conn| store::automations(conn)).await?;
    let bundle = bundle::export(&all, &chosen);
    if bundle.items.is_empty() {
        return Err(AppError::bad_request(
            "There are no automations to export yet.",
        ));
    }
    let json = serde_json::to_string_pretty(&bundle).map_err(AppError::internal)?;
    let disposition = format!(
        "attachment; filename=\"{}\"",
        bundle::file_name(&bundle.items)
    );
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        json,
    )
        .into_response())
}

async fn render_import(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
) -> AppResult<maud::Markup> {
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::automations::import_page(&shell, error))
}

async fn import_form(user: CurrentUser, State(state): State<AppState>) -> AppResult<maud::Markup> {
    require_admin(&user)?;
    render_import(&state, &user, None).await
}

/// The file's items, checked against the automations and secrets here.
async fn plans(state: &AppState, bundle: Bundle) -> AppResult<(Bundle, Vec<Plan>)> {
    state
        .db
        .call(move |conn| {
            let here = store::automations(conn)?;
            let secrets: BTreeSet<String> = secrets::list(conn)?
                .into_iter()
                .map(|secret| secret.name)
                .collect();
            let plans = bundle::plan(&bundle, &here, &secrets);
            Ok((bundle, plans))
        })
        .await
}

/// Reads an uploaded or pasted file and shows what importing it would do.
async fn preview(
    user: CurrentUser,
    State(state): State<AppState>,
    mut form: Multipart,
) -> AppResult<Response> {
    require_admin(&user)?;
    let mut text = String::new();
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|error| AppError::bad_request(error.body_text()))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        let value = field
            .bytes()
            .await
            .map_err(|error| AppError::bad_request(error.body_text()))?;
        // An uploaded file wins over pasted text.
        if (name == "file" || (name == "bundle" && text.trim().is_empty())) && !value.is_empty() {
            text = String::from_utf8_lossy(&value).into_owned();
        }
    }
    let bundle = match bundle::parse(&text) {
        Ok(bundle) => bundle,
        Err(error) => {
            let page = render_import(&state, &user, Some(&error)).await?;
            return Ok((StatusCode::BAD_REQUEST, page).into_response());
        }
    };
    let (bundle, plans) = plans(&state, bundle).await?;
    let json = serde_json::to_string(&bundle).map_err(AppError::internal)?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::automations::import_preview(&shell, &json, &bundle, &plans).into_response())
}

/// Imports the items as chosen in the preview, switched off.
async fn confirm(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let bundle = bundle::parse(form.get("bundle").map_or("", String::as_str))
        .map_err(AppError::bad_request)?;
    let (_, plans) = plans(&state, bundle).await?;
    let mut imported = 0_usize;
    for (index, plan) in plans.into_iter().enumerate() {
        let action = form
            .get(&format!("action_{index}"))
            .and_then(|key| Action::from_key(key))
            .unwrap_or(Action::Skip);
        if action == Action::Skip {
            continue;
        }
        if !plan.allows(action) {
            return Err(AppError::bad_request(format!(
                "Automations here changed since the preview. Import the file again to choose what to do with {}.",
                plan.item.name
            )));
        }
        let name = if action == Action::Copy {
            let here = state.db.call(|conn| store::automations(conn)).await?;
            bundle::copy_name(&plan.item.name, &here)
        } else {
            plan.item.name.clone()
        };
        let change = Change {
            id: if action == Action::Replace {
                plan.existing
            } else {
                None
            },
            name,
            source: plan.item.source,
            enabled: Some(false),
            kind: plan.item.kind,
            user_id: user.id,
            saved_with: "import".to_owned(),
        };
        if let Err(error) = save_change(&state, change).await? {
            state.automations.reload(&state).await?;
            return Err(AppError::bad_request(error));
        }
        imported = imported.saturating_add(1);
    }
    state.automations.reload(&state).await?;
    Ok(Redirect::to(&format!("/automations?imported={imported}")).into_response())
}
