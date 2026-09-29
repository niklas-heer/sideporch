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
pub async fn plans(state: &AppState, bundle: Bundle) -> AppResult<(Bundle, Vec<Plan>)> {
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
    let actions: Vec<Action> = (0..plans.len())
        .map(|index| {
            form.get(&format!("action_{index}"))
                .and_then(|key| Action::from_key(key))
                .unwrap_or(Action::Skip)
        })
        .collect();
    let imported = import(&state, plans, &actions, user.id, "import")
        .await?
        .map_err(AppError::bad_request)?;
    Ok(Redirect::to(&format!("/automations?imported={}", imported.len())).into_response())
}

/// One item an import added or replaced.
pub struct Imported {
    pub id: i64,
    pub name: String,
    pub kind: String,
    pub action: Action,
}

/// Takes `actions[i]` for `plans[i]`, switched off, then restarts the
/// automations once. Refuses an action the item doesn't allow, such as
/// replacing something that isn't there, when the server changed since the
/// preview.
pub async fn import(
    state: &AppState,
    plans: Vec<Plan>,
    actions: &[Action],
    user_id: i64,
    saved_with: &str,
) -> AppResult<Result<Vec<Imported>, String>> {
    for (plan, action) in plans.iter().zip(actions) {
        if !plan.allows(*action) {
            return Ok(Err(format!(
                "{} can't be {}: automations here changed since the preview, or the choice doesn't fit it. Its choices are {}.",
                plan.item.name,
                match action {
                    Action::Import => "imported as new",
                    Action::Replace => "replaced",
                    Action::Copy => "imported as a copy",
                    Action::Skip => "skipped",
                },
                plan.choices()
                    .iter()
                    .map(|choice| choice.key())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    }
    let mut imported = Vec::new();
    for (plan, action) in plans.into_iter().zip(actions.iter().copied()) {
        if action == Action::Skip {
            continue;
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
            name: name.clone(),
            source: plan.item.source,
            enabled: Some(false),
            kind: plan.item.kind.clone(),
            user_id,
            saved_with: saved_with.to_owned(),
        };
        match save_change(state, change).await? {
            Ok(id) => imported.push(Imported {
                id,
                name,
                kind: plan.item.kind,
                action,
            }),
            Err(error) => {
                state.automations.reload(state).await?;
                return Ok(Err(error));
            }
        }
    }
    state.automations.reload(state).await?;
    Ok(Ok(imported))
}
