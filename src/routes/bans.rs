//! Banning people, addresses, emails and email domains, and lifting bans.

use axum::{
    Form, Router,
    extract::{Path, State},
    response::Redirect,
    routing::post,
};
use serde::Deserialize;

use crate::{
    AppState,
    access::{self, ClientIp, Range, Target},
    auth::CurrentUser,
    community::Permission,
    error::{AppError, AppResult},
    now_ms, store,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/moderation/bans", post(add))
        .route("/moderation/bans/{ban_id}/lift", post(lift))
        .route("/people/{user_id}/ban", post(ban_person))
}

/// How long bans last: milliseconds, with `0` for until lifted.
pub const DURATIONS: [(i64, &str); 4] = [
    (86_400_000, "1 day"),
    (604_800_000, "1 week"),
    (2_592_000_000, "30 days"),
    (0, "until lifted"),
];

fn ends(duration: i64) -> AppResult<Option<i64>> {
    match DURATIONS.iter().find(|(known, _)| *known == duration) {
        Some((0, _)) => Ok(None),
        Some((duration, _)) => Ok(Some(now_ms().saturating_add(*duration))),
        None => Err(AppError::bad_request("Choose how long the ban lasts.")),
    }
}

const fn require_moderator(user: &CurrentUser) -> AppResult<()> {
    if user.may(Permission::Moderate) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

#[derive(Deserialize)]
struct BanForm {
    target: String,
    #[serde(default)]
    reason: String,
    duration: i64,
}

async fn add(
    user: CurrentUser,
    State(state): State<AppState>,
    ClientIp(own): ClientIp,
    Form(form): Form<BanForm>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    let target = Target::parse(&form.target).ok_or_else(|| {
        AppError::bad_request(
            "Ban an address like 203.0.113.7, a range like 203.0.113.0/24, an email address, or a domain like @spam.example.",
        )
    })?;
    if let (Target::Addresses(range), Some(own)) = (&target, own)
        && range.contains(own)
    {
        return Err(AppError::bad_request(
            "That would ban your own address. Ask another moderator if it has to be.",
        ));
    }
    let expires_at = ends(form.duration)?;
    let by = user.id;
    state
        .db
        .call(move |conn| access::add_ban(conn, &target, &form.reason, by, expires_at))
        .await?;
    state.access.reload(&state).await?;
    Ok(Redirect::to("/moderation#bans"))
}

async fn lift(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(ban_id): Path<i64>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    state
        .db
        .call(move |conn| access::lift_ban(conn, ban_id))
        .await?;
    state.access.reload(&state).await?;
    Ok(Redirect::to("/moderation#bans"))
}

#[derive(Deserialize)]
struct BanPersonForm {
    #[serde(default)]
    reason: String,
    duration: i64,
    /// Also ban the addresses they used lately.
    addresses: Option<String>,
    /// Also ban their email address.
    email: Option<String>,
    /// Remove every message and reaction of theirs.
    remove: Option<String>,
}

/// Deactivates someone, and bans and cleans up after them as chosen.
async fn ban_person(
    user: CurrentUser,
    State(state): State<AppState>,
    ClientIp(own): ClientIp,
    Path(user_id): Path<i64>,
    Form(form): Form<BanPersonForm>,
) -> AppResult<Redirect> {
    require_moderator(&user)?;
    if user_id == user.id {
        return Err(AppError::bad_request("You can't ban yourself."));
    }
    let expires_at = ends(form.duration)?;
    let by = user.id;
    let reason = form.reason.trim().to_owned();
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let person = store::user(conn, user_id)?.ok_or(AppError::NotFound)?;
            if person.is_admin {
                return Err(AppError::bad_request("Admins can't be banned."));
            }
            if person.server.is_some() {
                return Err(AppError::bad_request(
                    "People from other servers are banned by their own server. Stop sharing channels with it under Admin → Connections instead.",
                ));
            }
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let reason = if reason.is_empty() {
                format!("Banned with {}", person.username)
            } else {
                format!("{reason} ({})", person.username)
            };
            store::set_deactivated(&tx, user_id, Some(now))?;
            if form.addresses.is_some() {
                for seen in access::addresses(&tx, user_id)? {
                    let Some(range) = Range::parse(&seen.ip) else {
                        continue;
                    };
                    // Never the moderator's own address, shared or not.
                    if own.is_some_and(|own| range.contains(own)) {
                        continue;
                    }
                    access::add_ban(&tx, &Target::Addresses(range), &reason, by, expires_at)?;
                }
            }
            if form.email.is_some()
                && let Some(email) = crate::security::email(&tx, user_id)?
            {
                access::add_ban(&tx, &Target::Email(email), &reason, by, expires_at)?;
            }
            if form.remove.is_some() {
                store::remove_everything_by(&tx, user_id, now)?;
            }
            tx.commit()?;
            Ok(())
        })
        .await?;
    state.access.reload(&state).await?;
    Ok(Redirect::to(&format!("/people/{user_id}")))
}
