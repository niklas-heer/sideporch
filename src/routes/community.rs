//! Signing up, and how admins and moderators look after the community:
//! who may join, what trust levels and roles allow, reports and time-outs.

use std::collections::HashMap;

use axum::{
    Form, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use super::{AccountInput, redirect_with_cookie, shell_data, validate_account, wants_no_content};
use crate::{
    AppState,
    access::{self, ClientIp, Counted},
    auth,
    auth::CurrentUser,
    automations,
    community::{self, Joining, Permission, Registration, Requirement},
    error::{AppError, AppResult},
    now_ms, store,
    views::{self, AccountForm, Shell, community as pages},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/signup", get(signup_form).post(signup))
        .route("/admin/community", get(community_page).post(save_community))
        .route(
            "/admin/permissions",
            get(permissions_page).post(save_permissions),
        )
        .route("/admin/roles", post(create_role))
        .route("/admin/roles/{role_id}/delete", post(delete_role))
        .route("/admin/roles/{role_id}/badge", post(role_badge))
        .route("/moderation", get(moderation))
        .route("/moderation/signups/{signup_id}/approve", post(approve))
        .route("/moderation/signups/{signup_id}/decline", post(decline))
        .route(
            "/moderation/reports/{message_id}/dismiss",
            post(dismiss_report),
        )
        .route(
            "/moderation/reports/{message_id}/delete",
            post(delete_reported),
        )
        .route("/people/{user_id}/trust", post(set_trust))
        .route("/people/{user_id}/roles", post(set_roles))
        .route("/people/{user_id}/timeout", post(timeout))
        .route(
            "/c/{channel_id}/m/{message_id}/report",
            get(report_form).post(report),
        )
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

const fn require_moderator(user: &CurrentUser) -> AppResult<()> {
    if user.may(Permission::Moderate) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// Signing up

#[derive(Deserialize)]
struct SignupInput {
    display_name: String,
    username: String,
    password: String,
    #[serde(default)]
    note: String,
    /// Checked when the person agrees to the rules.
    rules: Option<String>,
    /// Left empty by people; bots fill in every field.
    #[serde(default)]
    website: String,
    /// The proof-of-work challenge from the form, and app.js's answer.
    #[serde(default)]
    challenge: String,
    #[serde(default)]
    proof: String,
}

/// Refuses sign-ups without the form's proof of work, and too many from
/// one address; otherwise counts this one.
fn bot_check(
    state: &AppState,
    address: Option<std::net::IpAddr>,
    input: &SignupInput,
) -> Result<(), String> {
    if let Some(wait) = address.and_then(|address| {
        state
            .access
            .check(&Counted::SignUp(address), access::SIGN_UP)
            .err()
    }) {
        return Err(format!(
            "Several accounts were just made from your network. Try again in {}.",
            access::wait_text(wait)
        ));
    }
    if !state.access.redeem(&input.challenge, &input.proof) {
        return Err(
            "Your browser didn't finish the check that keeps bots out. Wait a moment, then try again."
                .to_owned(),
        );
    }
    if let Some(address) = address {
        state.access.count(Counted::SignUp(address));
    }
    Ok(())
}

async fn joining(state: &AppState) -> AppResult<Joining> {
    state.db.call(|conn| Joining::load(conn)).await
}

async fn signup_form(State(state): State<AppState>) -> AppResult<Response> {
    let joining = joining(&state).await?;
    if joining.registration == Registration::Invite {
        return Err(AppError::NotFound);
    }
    let ctx = state.db.call(|conn| store::render_context(conn)).await?;
    let challenge = state.access.challenge()?;
    Ok(pages::signup_page(
        &joining,
        &ctx,
        None,
        &AccountForm::default(),
        "",
        &challenge,
    )
    .into_response())
}

async fn signup(
    State(state): State<AppState>,
    ClientIp(address): ClientIp,
    Form(input): Form<SignupInput>,
) -> AppResult<Response> {
    let joining = joining(&state).await?;
    if joining.registration == Registration::Invite {
        return Err(AppError::NotFound);
    }
    let ctx = state.db.call(|conn| store::render_context(conn)).await?;
    let note: String = input.note.trim().chars().take(500).collect();
    let challenge = state.access.challenge()?;
    let refuse = |error: &str, form: &AccountForm| {
        (
            StatusCode::BAD_REQUEST,
            pages::signup_page(&joining, &ctx, Some(error), form, &note, &challenge),
        )
            .into_response()
    };
    let form = AccountForm {
        display_name: input.display_name.trim().to_owned(),
        username: input.username.trim().to_lowercase(),
    };
    if !input.website.is_empty() {
        return Ok(refuse("Something went wrong. Try again.", &form));
    }
    if let Err(error) = bot_check(&state, address, &input) {
        return Ok(refuse(&error, &form));
    }
    if !joining.rules.is_empty() && input.rules.is_none() {
        return Ok(refuse("Agree to the rules to join.", &form));
    }
    let account = match validate_account(AccountInput {
        display_name: input.display_name,
        username: input.username,
        password: input.password,
    }) {
        Ok(account) => account,
        Err((error, form)) => return Ok(refuse(error, &form)),
    };
    let hash = auth::hash_password(account.password).await?;
    let now = now_ms();
    let approval = joining.registration == Registration::Approval;
    let username = account.username.clone();
    let display_name = account.display_name.clone();
    let saved_note = note.clone();
    let created = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if community::too_many_signups(&tx, now)? {
                return Ok(Err(
                    "Lots of people signed up in the last hour. Try again a little later.",
                ));
            }
            if community::username_taken(&tx, &username)? {
                return Ok(Err("That username is taken. Try another one."));
            }
            let id = if approval {
                community::add_signup(&tx, &username, &display_name, &hash, &saved_note, now)?;
                None
            } else {
                let id = store::create_user(&tx, &username, &display_name, &hash, false, now)?;
                community::set_trust(&tx, id, 0, false)?;
                Some(id)
            };
            tx.commit()?;
            Ok(Ok(id))
        })
        .await?;
    match created {
        Err(error) => Ok(refuse(error, &form)),
        Ok(None) => Ok(pages::waiting_page(&account.display_name).into_response()),
        Ok(Some(user_id)) => {
            state
                .automations
                .event(automations::Event::MemberJoined(automations::MemberEvent {
                    user: account.display_name,
                    username: account.username,
                }));
            let cookie = auth::start_session(&state, user_id).await?;
            redirect_with_cookie("/", &cookie)
        }
    }
}

// Admin settings

async fn render_community(state: &AppState, user: &CurrentUser, saved: bool) -> AppResult<Markup> {
    let (joining, requirements, counts) = state
        .db
        .call(|conn| {
            let mut statement = conn.prepare(
                "SELECT trust_level, COUNT(*) FROM users WHERE deactivated_at IS NULL GROUP BY trust_level",
            )?;
            let counts: Vec<(u8, i64)> = statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            Ok((Joining::load(conn)?, community::requirements(conn)?, counts))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(pages::community_page(
        &shell,
        &joining,
        &requirements,
        &counts,
        saved,
    ))
}

async fn community_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_community(&state, &user, false).await
}

fn number(form: &HashMap<String, String>, key: &str) -> AppResult<u32> {
    form.get(key)
        .map_or(Ok(0), |value| value.trim().parse())
        .map_err(|_| AppError::bad_request("Enter whole numbers for the trust levels."))
}

async fn save_community(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    let registration = form
        .get("registration")
        .and_then(|value| Registration::parse(value))
        .ok_or_else(|| AppError::bad_request("Pick how people join."))?;
    let rules: String = form
        .get("rules")
        .map(|rules| rules.chars().take(5000).collect())
        .unwrap_or_default();
    let mut requirements = [Requirement {
        days: 0,
        visits: 0,
        messages: 0,
    }; 3];
    for (level, requirement) in (1_u8..).zip(requirements.iter_mut()) {
        requirement.days = number(&form, &format!("days_{level}"))?.min(3650);
        requirement.visits = number(&form, &format!("visits_{level}"))?.min(3650);
        requirement.messages = number(&form, &format!("messages_{level}"))?.min(1_000_000);
    }
    let joining = Joining {
        registration,
        rules,
        new_member_per_minute: number(&form, "new_member_per_minute")?.min(600),
    };
    state
        .db
        .call(move |conn| {
            joining.save(conn)?;
            community::set_requirements(conn, &requirements)
        })
        .await?;
    render_community(&state, &user, true).await
}

async fn render_permissions(
    state: &AppState,
    user: &CurrentUser,
    error: Option<&str>,
    saved: bool,
) -> AppResult<Markup> {
    let (levels, roles) = state
        .db
        .call(|conn| Ok((community::levels(conn)?, community::roles(conn)?)))
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(pages::permissions_page(
        &shell, &levels, &roles, error, saved,
    ))
}

async fn permissions_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render_permissions(&state, &user, None, false).await
}

/// Saves the permission table: `level_<permission>` holds a level or
/// `none`; a checkbox `role_<id>_<permission>` grants it to a role.
async fn save_permissions(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            for permission in Permission::ALL {
                let level = form
                    .get(&format!("level_{}", permission.key()))
                    .and_then(|value| value.parse::<u8>().ok())
                    .filter(|level| *level <= 4);
                community::set_level(&tx, permission, level)?;
            }
            for role in community::roles(&tx)? {
                let granted: Vec<Permission> = Permission::ALL
                    .into_iter()
                    .filter(|permission| {
                        form.contains_key(&format!("role_{}_{}", role.id, permission.key()))
                    })
                    .collect();
                community::set_role_permissions(&tx, role.id, &granted)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    render_permissions(&state, &user, None, true).await
}

#[derive(Deserialize)]
struct RoleForm {
    name: String,
    #[serde(default)]
    description: String,
}

async fn create_role(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<RoleForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let name: String = form.name.trim().chars().take(40).collect();
    let description: String = form.description.trim().chars().take(200).collect();
    if name.is_empty() {
        return Ok((
            StatusCode::BAD_REQUEST,
            render_permissions(&state, &user, Some("Give the role a name."), false).await?,
        )
            .into_response());
    }
    let now = now_ms();
    let created = state
        .db
        .call(move |conn| {
            if community::role_name_taken(conn, &name)? {
                return Ok(false);
            }
            community::create_role(conn, &name, &description, now)?;
            Ok(true)
        })
        .await?;
    if !created {
        return Ok((
            StatusCode::BAD_REQUEST,
            render_permissions(&state, &user, Some("A role with that name exists."), false).await?,
        )
            .into_response());
    }
    Ok(Redirect::to("/admin/permissions#roles").into_response())
}

#[derive(Deserialize)]
struct BadgeForm {
    badge: Option<String>,
    #[serde(default)]
    color: String,
}

async fn role_badge(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(role_id): Path<i64>,
    Form(form): Form<BadgeForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let badge = form.badge.is_some();
    state
        .db
        .call(move |conn| community::set_role_badge(conn, role_id, badge, &form.color))
        .await?;
    Ok(Redirect::to("/admin/permissions#roles"))
}

async fn delete_role(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(role_id): Path<i64>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| community::delete_role(conn, role_id))
        .await?;
    Ok(Redirect::to("/admin/permissions#roles"))
}

// Moderation

async fn moderation(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_moderator(&user)?;
    let now = now_ms();
    let (signups, reports, timed_out, bans, ctx) = state
        .db
        .call(move |conn| {
            let mut statement = conn.prepare(
                "SELECT id, display_name, muted_until FROM users WHERE muted_until > ?1 ORDER BY muted_until",
            )?;
            let timed_out: Vec<(i64, String, i64)> = statement
                .query_map([now], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<_, _>>()?;
            Ok((
                community::signups(conn)?,
                community::open_reports(conn)?,
                timed_out,
                crate::access::bans(conn)?,
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
    Ok(pages::moderation_page(
        &shell,
        &signups,
        &reports,
        &timed_out,
        &bans,
        &views::Render::for_user(&ctx, user.id),
    ))
}

async fn approve(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(signup_id): Path<i64>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    let now = now_ms();
    let approved = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let approved = community::approve(&tx, signup_id, now)?;
            tx.commit()?;
            Ok(approved)
        })
        .await?;
    if let Some((_, display_name, username)) = approved {
        state
            .automations
            .event(automations::Event::MemberJoined(automations::MemberEvent {
                user: display_name,
                username,
            }));
    }
    Ok(Redirect::to("/moderation"))
}

async fn decline(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(signup_id): Path<i64>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    state
        .db
        .call(move |conn| community::decline(conn, signup_id))
        .await?;
    Ok(Redirect::to("/moderation"))
}

async fn dismiss_report(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(message_id): Path<i64>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    let (by, now) = (user.id, now_ms());
    state
        .db
        .call(move |conn| community::resolve_reports(conn, message_id, by, "dismissed", now))
        .await?;
    Ok(Redirect::to("/moderation"))
}

async fn delete_reported(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(message_id): Path<i64>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    let found = state
        .db
        .call(move |conn| store::message(conn, message_id))
        .await?
        .ok_or(AppError::NotFound)?;
    if !found.deleted {
        crate::messages::change(
            &state,
            &user,
            found.channel_id,
            message_id,
            crate::messages::Change::Delete,
        )
        .await?;
    }
    let (by, now) = (user.id, now_ms());
    state
        .db
        .call(move |conn| community::resolve_reports(conn, message_id, by, "deleted", now))
        .await?;
    Ok(Redirect::to("/moderation"))
}

#[derive(Deserialize)]
struct ReportForm {
    #[serde(default)]
    reason: String,
}

async fn report_form(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let message = state
        .db
        .call(move |conn| store::readable_message(conn, user_id, channel_id, message_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: Some(channel_id),
    };
    Ok(pages::report_page(&shell, &message))
}

async fn report(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ReportForm>,
) -> AppResult<Response> {
    let user_id = user.id;
    let reason: String = form.reason.trim().chars().take(500).collect();
    let now = now_ms();
    let message = state
        .db
        .call(move |conn| {
            let message = store::readable_message(conn, user_id, channel_id, message_id)?
                .filter(|message| !message.deleted)
                .ok_or(AppError::NotFound)?;
            community::report(conn, message_id, user_id, &reason, now)?;
            Ok(message)
        })
        .await?;
    if wants_no_content(&headers) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    Ok(Redirect::to(&super::message_href(&message)).into_response())
}

// People

/// Loads someone else's account for an admin or moderator.
async fn other_person(
    state: &AppState,
    user: &CurrentUser,
    user_id: i64,
) -> AppResult<store::User> {
    if user.id == user_id {
        return Err(AppError::bad_request(
            "Ask someone else to change your own account.",
        ));
    }
    state
        .db
        .call(move |conn| store::user(conn, user_id))
        .await?
        .ok_or(AppError::NotFound)
}

#[derive(Deserialize)]
struct TrustForm {
    level: u8,
    /// A checkbox: present to keep the level from changing on its own.
    locked: Option<String>,
}

async fn set_trust(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Form(form): Form<TrustForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    other_person(&state, &user, user_id).await?;
    let locked = form.locked.is_some();
    let level = form.level.min(4);
    state
        .db
        .call(move |conn| community::set_trust(conn, user_id, level, locked))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}

async fn set_roles(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Form(form): Form<HashMap<String, String>>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    other_person(&state, &user, user_id).await?;
    let roles: Vec<i64> = form
        .keys()
        .filter_map(|key| key.strip_prefix("role_")?.parse().ok())
        .collect();
    state
        .db
        .call(move |conn| community::set_user_roles(conn, user_id, &roles))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}

#[derive(Deserialize)]
struct TimeoutForm {
    /// Milliseconds, or 0 to end a time-out.
    duration: i64,
}

async fn timeout(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    Form(form): Form<TimeoutForm>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    let person = other_person(&state, &user, user_id).await?;
    if person.is_admin {
        return Err(AppError::bad_request("Admins can't be timed out."));
    }
    let until = community::TIMEOUTS
        .iter()
        .find(|(duration, _)| *duration == form.duration)
        .map(|(duration, _)| now_ms().saturating_add(*duration));
    state
        .db
        .call(move |conn| community::set_timeout(conn, user_id, until))
        .await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}
