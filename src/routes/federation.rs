//! Connections with other Sideporch servers: what servers send each other,
//! and Admin → Connections.

use std::sync::Arc;

use axum::{
    Form, Json, Router,
    body::Bytes,
    extract::{OriginalUri, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use base64ct::{Base64, Encoding as _};
use maud::Markup;
use serde::{Deserialize, Serialize};

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    federation::{
        self,
        data::{self, Status},
    },
    now_ms,
    views::{self, Shell},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/.well-known/sideporch", get(well_known))
        .route("/federation/requests", post(receive_request))
        .route("/federation/accept", post(receive_accept))
        .route("/federation/decline", post(receive_decline))
        .route("/federation/disconnect", post(receive_disconnect))
        .route("/federation/inbox", post(inbox))
        .route("/federation/files/{file_id}", get(serve_file))
        .route("/federation/people", get(directory))
        .route("/c/{channel_id}/share", post(share_channel))
        .route("/c/{channel_id}/unshare", post(unshare_channel))
        .route("/admin/connections/offers/{id}/take", post(take_offer))
        .route(
            "/admin/connections/offers/{id}/decline",
            post(decline_offer),
        )
        .route("/people/elsewhere", get(people_elsewhere))
        .route("/people/elsewhere/message", post(message_elsewhere))
        .route("/admin/connections", get(page).post(connect))
        .route("/admin/connections/name", post(save_name))
        .route("/admin/connections/{id}/accept", post(accept))
        .route("/admin/connections/{id}/decline", post(decline))
        .route("/admin/connections/{id}/disconnect", post(disconnect))
        .route("/admin/connections/{id}/direct", post(allow_direct))
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// A refused server-to-server request: why, for the other server's log.
pub fn refused(status: StatusCode, reason: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": reason.into() }))).into_response()
}

async fn team_name(state: &AppState) -> AppResult<String> {
    let handle = state.federation.handle().unwrap_or_default();
    Ok(state
        .db
        .call(|conn| data::team_name(conn))
        .await?
        .unwrap_or(handle))
}

async fn well_known(State(state): State<AppState>) -> AppResult<Response> {
    let name = team_name(&state).await?;
    Ok(state.federation.description(&name).map_or_else(
        || {
            refused(
                StatusCode::NOT_FOUND,
                "this server has no public URL, so it can't connect to others",
            )
        },
        |description| Json(description).into_response(),
    ))
}

/// What a server sends when asking to connect.
#[derive(Serialize, Deserialize)]
struct ConnectRequest {
    url: String,
    name: String,
    public_key: String,
    note: String,
}

/// At most this many requests wait for admins, so nobody can pile them up.
const MAX_PENDING: i64 = 50;

async fn receive_request(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let Ok(request) = serde_json::from_slice::<ConnectRequest>(&body) else {
        return Ok(refused(StatusCode::BAD_REQUEST, "not a request to connect"));
    };
    let Ok(key) = Base64::decode_vec(&request.public_key) else {
        return Ok(refused(StatusCode::BAD_REQUEST, "the key isn't valid"));
    };
    let path = uri
        .path_and_query()
        .map_or("/federation/requests", |path| path.as_str());
    let signer = match state
        .federation
        .verify_with(&headers, &key, "POST", path, &body)
    {
        Ok(signer) => signer,
        Err(reason) => return Ok(refused(StatusCode::UNAUTHORIZED, reason)),
    };
    let url = request.url.trim_end_matches('/').to_owned();
    if signer != url {
        return Ok(refused(
            StatusCode::UNAUTHORIZED,
            "signed by another server",
        ));
    }
    // Whoever controls the address publishes the same key.
    let description = match state.federation.describe(&url).await {
        Ok(description) => description,
        Err(reason) => return Ok(refused(StatusCode::BAD_REQUEST, reason)),
    };
    if description.key().ok().as_deref() != Some(key.as_slice()) {
        return Ok(refused(
            StatusCode::UNAUTHORIZED,
            "the key doesn't match the one the server publishes",
        ));
    }
    let Some(handle) = federation::handle_of(&url) else {
        return Ok(refused(StatusCode::BAD_REQUEST, "the address isn't valid"));
    };
    let note: String = request.note.chars().take(500).collect();
    let name: String = description.name.chars().take(80).collect();
    let now = now_ms();
    let outcome = state
        .db
        .call(move |conn| {
            let known = data::instance_by_url(conn, &url)?;
            match known.as_ref().map(|instance| instance.status) {
                Some(Status::Connected) => return Ok(Err("already connected")),
                // Both sides asked: that's agreement.
                Some(Status::Requested) => {
                    data::save_instance(
                        conn,
                        &data::Seen {
                            url: &url,
                            handle: &handle,
                            name: &name,
                            public_key: Some(&key),
                            status: Status::Connected,
                            note: &note,
                        },
                        now,
                    )?;
                    return Ok(Ok(true));
                }
                _ => {}
            }
            if data::pending_count(conn)? >= MAX_PENDING {
                return Ok(Err("too many requests are waiting here"));
            }
            data::save_instance(
                conn,
                &data::Seen {
                    url: &url,
                    handle: &handle,
                    name: &name,
                    public_key: Some(&key),
                    status: Status::Pending,
                    note: &note,
                },
                now,
            )?;
            Ok(Ok(false))
        })
        .await?;
    Ok(match outcome {
        Ok(connected) => Json(serde_json::json!({ "connected": connected })).into_response(),
        Err(reason) => refused(StatusCode::CONFLICT, reason),
    })
}

/// Checks a signed POST from a server in one of `statuses`.
async fn verified(
    state: &AppState,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
    statuses: &'static [Status],
) -> AppResult<Result<data::Instance, Response>> {
    verified_as(state, "POST", uri, headers, body, statuses).await
}

/// Checks a signed request from a server in one of `statuses`.
async fn verified_as(
    state: &AppState,
    method: &'static str,
    uri: &axum::http::Uri,
    headers: HeaderMap,
    body: Bytes,
    statuses: &'static [Status],
) -> AppResult<Result<data::Instance, Response>> {
    let federation = Arc::clone(&state.federation);
    let path = uri
        .path_and_query()
        .map_or_else(String::new, |path| path.as_str().to_owned());
    state
        .db
        .call(move |conn| {
            Ok(federation
                .verify(conn, &headers, method, &path, &body, statuses)
                .map_err(|reason| refused(StatusCode::UNAUTHORIZED, reason)))
        })
        .await
}

async fn receive_accept(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let instance = match verified(&state, &uri, headers, body, &[Status::Requested]).await? {
        Ok(instance) => instance,
        Err(refusal) => return Ok(refusal),
    };
    let now = now_ms();
    state
        .db
        .call(move |conn| data::set_status(conn, instance.id, Status::Connected, now))
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn receive_decline(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let instance = match verified(&state, &uri, headers, body, &[Status::Requested]).await? {
        Ok(instance) => instance,
        Err(refusal) => return Ok(refusal),
    };
    let now = now_ms();
    state
        .db
        .call(move |conn| data::set_status(conn, instance.id, Status::Declined, now))
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn receive_disconnect(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let statuses = &[Status::Connected, Status::Requested, Status::Pending];
    let instance = match verified(&state, &uri, headers, body, statuses).await? {
        Ok(instance) => instance,
        Err(refusal) => return Ok(refusal),
    };
    let now = now_ms();
    state
        .db
        .call(move |conn| data::set_status(conn, instance.id, Status::Disconnected, now))
        .await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Events from a connected server.
async fn inbox(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Response> {
    let instance = match verified(&state, &uri, headers, body.clone(), &[Status::Connected]).await?
    {
        Ok(instance) => instance,
        Err(refusal) => return Ok(refusal),
    };
    let Ok(batch) = serde_json::from_slice::<federation::events::Batch>(&body) else {
        return Ok(refused(StatusCode::BAD_REQUEST, "not a batch of events"));
    };
    if batch.events.len() > 200 {
        return Ok(refused(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too many events at once",
        ));
    }
    // Events that don't apply are dropped; sending them again wouldn't help.
    let problems = federation::inbound::receive(&state, &instance, batch).await;
    Ok(Json(serde_json::json!({ "problems": problems })).into_response())
}

/// A file attached to a message in a channel shared with the asking server.
async fn serve_file(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    Path(file_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Response> {
    let statuses = &[Status::Connected];
    let instance = match verified_as(&state, "GET", &uri, headers, Bytes::new(), statuses).await? {
        Ok(instance) => instance,
        Err(refusal) => return Ok(refusal),
    };
    let instance_id = instance.id;
    let file = state
        .db
        .call(move |conn| {
            use rusqlite::OptionalExtension as _;
            Ok(conn
                .query_row(
                    "SELECT f.mime, f.sha256 FROM files f WHERE f.id = ?1 AND EXISTS (
                         SELECT 1 FROM message_files mf JOIN messages m ON m.id = mf.message_id
                         JOIN shared_channels s ON s.channel_id = m.channel_id
                         WHERE mf.file_id = f.id AND s.instance_id = ?2 AND s.status = 'active')",
                    rusqlite::params![file_id, instance_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?)
        })
        .await?;
    let Some((mime, Some(sha256))) = file else {
        return Ok(refused(StatusCode::NOT_FOUND, "no such shared file"));
    };
    let data = state.blobs.read(&sha256).await?;
    Ok(([(axum::http::header::CONTENT_TYPE, mime)], data).into_response())
}

#[derive(Deserialize)]
struct DirectoryQuery {
    #[serde(default)]
    q: String,
    id: Option<i64>,
}

/// Someone found in another server's directory.
#[derive(Serialize, Deserialize)]
struct Listed {
    id: i64,
    username: String,
    display_name: String,
}

/// People here, for a connected server whose people may write to ours.
async fn directory(
    State(state): State<AppState>,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<DirectoryQuery>,
) -> AppResult<Response> {
    let statuses = &[Status::Connected];
    let instance = match verified_as(&state, "GET", &uri, headers, Bytes::new(), statuses).await? {
        Ok(instance) => instance,
        Err(refusal) => return Ok(refusal),
    };
    if !instance.allow_direct {
        return Ok(refused(
            StatusCode::FORBIDDEN,
            "people there may not write to people here",
        ));
    }
    let pattern = format!("%{}%", query.q.trim().chars().take(50).collect::<String>());
    let people = state
        .db
        .call(move |conn| {
            let mut statement = conn.prepare(
                "SELECT id, username, display_name FROM users
                 WHERE instance_id IS NULL AND deactivated_at IS NULL
                   AND ((?1 IS NOT NULL AND id = ?1)
                        OR (?1 IS NULL AND length(?2) > 3 AND (username LIKE ?2 OR display_name LIKE ?2)))
                 ORDER BY display_name COLLATE NOCASE LIMIT 20",
            )?;
            let people = statement
                .query_map(rusqlite::params![query.id, pattern], |row| {
                    Ok(Listed {
                        id: row.get(0)?,
                        username: row.get(1)?,
                        display_name: row.get(2)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(people)
        })
        .await?;
    Ok(Json(serde_json::json!({ "people": people })).into_response())
}

// Admin → Connections

async fn render(state: &AppState, user: &CurrentUser, error: Option<&str>) -> AppResult<Markup> {
    let (instances, name, offers, backlogs) = state
        .db
        .call(|conn| {
            let instances = data::instances(conn)?;
            let mut backlogs = Vec::new();
            for instance in &instances {
                let (waiting, error) = data::backlog(conn, instance.id)?;
                if waiting > 0 {
                    backlogs.push((instance.id, waiting, error));
                }
            }
            Ok((
                instances,
                data::team_name(conn)?,
                data::offers(conn)?,
                backlogs,
            ))
        })
        .await?;
    let sidebar = shell_data(state, user.id).await?;
    Ok(views::federation::connections_page(
        &Shell {
            user,
            sidebar: &sidebar,
            current: None,
        },
        &views::federation::ConnectionsView {
            url: state.federation.url(),
            name: name.as_deref(),
            fingerprint: &state.federation.fingerprint(),
            instances: &instances,
            offers: &offers,
            backlogs: &backlogs,
            error,
        },
    ))
}

async fn page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    render(&state, &user, None).await
}

#[derive(Deserialize)]
struct ConnectForm {
    url: String,
    #[serde(default)]
    note: String,
}

async fn connect(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<ConnectForm>,
) -> AppResult<Response> {
    require_admin(&user)?;
    match ask_to_connect(&state, &form.url, &form.note).await {
        Ok(()) => Ok(Redirect::to("/admin/connections").into_response()),
        Err(error) => Ok(render(&state, &user, Some(&error)).await?.into_response()),
    }
}

async fn ask_to_connect(state: &AppState, url: &str, note: &str) -> Result<(), String> {
    let own = state
        .federation
        .url()
        .ok_or("Set Sideporch's public URL (--public-url) to connect with other servers.")?
        .to_owned();
    let url = federation::normalize_url(url)
        .ok_or("Enter the other server's address, like https://chat.example.org.")?;
    if url == own {
        return Err("That's this server.".to_owned());
    }
    let description = state.federation.describe(&url).await?;
    let key = description.key()?;
    let handle = federation::handle_of(&url).ok_or("That address isn't valid.")?;
    let note: String = note.trim().chars().take(500).collect();
    let name = team_name(state).await.map_err(|error| error.to_string())?;
    let request = serde_json::to_vec(&ConnectRequest {
        url: own,
        name,
        public_key: Base64::encode_string(state.federation.public_key()),
        note: note.clone(),
    })
    .map_err(|error| error.to_string())?;
    // Remember them first: when they accept, their answer is checked with
    // the key they publish now.
    let now = now_ms();
    let (their_url, their_handle, their_name) = (url.clone(), handle, description.name.clone());
    state
        .db
        .call(move |conn| {
            if data::instance_by_url(conn, &their_url)?
                .is_some_and(|instance| instance.status == Status::Connected)
            {
                return Ok(());
            }
            data::save_instance(
                conn,
                &data::Seen {
                    url: &their_url,
                    handle: &their_handle,
                    name: &their_name,
                    public_key: Some(&key),
                    status: Status::Requested,
                    note: &note,
                },
                now,
            )?;
            Ok(())
        })
        .await
        .map_err(|error| error.to_string())?;
    let (status, body) = state
        .federation
        .send("POST", &url, "/federation/requests", request)
        .await?;
    if !(200..300).contains(&status) {
        return Err(format!("{url} said no: {}", reason(&body)));
    }
    // They had asked us too, so we're connected now.
    if serde_json::from_slice::<serde_json::Value>(&body)
        .is_ok_and(|answer| answer.get("connected") == Some(&serde_json::Value::Bool(true)))
    {
        let now = now_ms();
        state
            .db
            .call(move |conn| {
                if let Some(instance) = data::instance_by_url(conn, &url)? {
                    data::set_status(conn, instance.id, Status::Connected, now)?;
                }
                Ok(())
            })
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// The reason another server gave for refusing a request.
fn reason(body: &[u8]) -> String {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|answer| {
            answer
                .get("error")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| "no reason given".to_owned())
}

async fn instance_for_admin(state: &AppState, id: i64) -> AppResult<data::Instance> {
    state
        .db
        .call(move |conn| data::instance(conn, id))
        .await?
        .ok_or(AppError::NotFound)
}

async fn accept(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let instance = instance_for_admin(&state, id).await?;
    if instance.status != Status::Pending {
        return Ok(Redirect::to("/admin/connections").into_response());
    }
    match state
        .federation
        .send("POST", &instance.url, "/federation/accept", Vec::new())
        .await
    {
        Ok((status, _)) if (200..300).contains(&status) => {
            let now = now_ms();
            state
                .db
                .call(move |conn| data::set_status(conn, id, Status::Connected, now))
                .await?;
            Ok(Redirect::to("/admin/connections").into_response())
        }
        Ok((_, body)) => {
            let error = format!(
                "{} didn't take the answer: {}",
                instance.handle,
                reason(&body)
            );
            Ok(render(&state, &user, Some(&error)).await?.into_response())
        }
        Err(error) => {
            let error = format!("{} couldn't be reached: {error}", instance.handle);
            Ok(render(&state, &user, Some(&error)).await?.into_response())
        }
    }
}

async fn decline(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let instance = instance_for_admin(&state, id).await?;
    if instance.status == Status::Pending {
        // Telling them is a courtesy; declining here is what counts.
        drop(
            state
                .federation
                .send("POST", &instance.url, "/federation/decline", Vec::new())
                .await,
        );
        let now = now_ms();
        state
            .db
            .call(move |conn| data::set_status(conn, id, Status::Declined, now))
            .await?;
    }
    Ok(Redirect::to("/admin/connections"))
}

async fn disconnect(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let instance = instance_for_admin(&state, id).await?;
    drop(
        state
            .federation
            .send("POST", &instance.url, "/federation/disconnect", Vec::new())
            .await,
    );
    let now = now_ms();
    state
        .db
        .call(move |conn| data::set_status(conn, id, Status::Disconnected, now))
        .await?;
    Ok(Redirect::to("/admin/connections"))
}

#[derive(Deserialize)]
struct DirectForm {
    allow: Option<String>,
}

async fn allow_direct(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Form(form): Form<DirectForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let allow = form.allow.is_some();
    state
        .db
        .call(move |conn| data::set_allow_direct(conn, id, allow))
        .await?;
    Ok(Redirect::to("/admin/connections"))
}

#[derive(Deserialize)]
struct NameForm {
    name: String,
}

async fn save_name(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<NameForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let name: String = form.name.trim().chars().take(80).collect();
    state
        .db
        .call(move |conn| data::set_team_name(conn, &name))
        .await?;
    Ok(Redirect::to("/admin/connections"))
}

// Sharing channels

#[derive(Deserialize)]
struct ServerForm {
    server: i64,
}

fn here_of(state: &AppState) -> AppResult<(String, String)> {
    Ok((
        state.federation.handle().ok_or_else(|| {
            AppError::bad_request("Set Sideporch's public URL to share with other servers.")
        })?,
        state
            .federation
            .url()
            .ok_or_else(|| {
                AppError::bad_request("Set Sideporch's public URL to share with other servers.")
            })?
            .to_owned(),
    ))
}

/// Offers a channel of ours to a connected server.
async fn share_channel(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<ServerForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    here_of(&state)?;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let (kind, name, topic, private): (String, Option<String>, String, bool) = conn
                .query_row(
                    "SELECT kind, name, topic, private FROM channels WHERE id = ?1",
                    [channel_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .map_err(|_| AppError::NotFound)?;
            if kind != "public" {
                return Err(AppError::bad_request(
                    "Conversations with people on other servers start from People.",
                ));
            }
            let instance = data::instance(conn, form.server)?
                .filter(|instance| instance.status == Status::Connected)
                .ok_or_else(|| AppError::bad_request("That server isn't connected."))?;
            let shares = data::shares_of(conn, channel_id)?;
            if shares.iter().any(|share| share.role == data::Role::Guest) {
                return Err(AppError::bad_request(
                    "This channel belongs to another server; only its host shares it.",
                ));
            }
            if shares.iter().any(|share| {
                share.instance_id == instance.id && share.status != data::ShareStatus::Ended
            }) {
                return Ok(());
            }
            data::save_share(
                conn,
                &data::Share {
                    channel_id,
                    instance_id: instance.id,
                    role: data::Role::Host,
                    remote_channel_id: None,
                    status: data::ShareStatus::Invited,
                },
                now,
            )?;
            data::enqueue(
                conn,
                instance.id,
                &federation::events::Event::Share {
                    channel: channel_id,
                    name: name.unwrap_or_default(),
                    topic,
                    private,
                    direct: None,
                },
                now,
            )
        })
        .await?;
    state.federation.wake.notify_one();
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

/// Stops sharing a channel with a server, either side.
async fn unshare_channel(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<ServerForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            let share = data::share(conn, channel_id, form.server)?.ok_or(AppError::NotFound)?;
            let host_channel = match share.role {
                data::Role::Host => Some(channel_id),
                data::Role::Guest => share.remote_channel_id,
            };
            data::save_share(
                conn,
                &data::Share {
                    status: data::ShareStatus::Ended,
                    ..share
                },
                now,
            )?;
            if let Some(channel) = host_channel {
                data::enqueue(
                    conn,
                    form.server,
                    &federation::events::Event::ShareEnded { channel },
                    now,
                )?;
            }
            Ok(())
        })
        .await?;
    state.federation.wake.notify_one();
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")))
}

/// A name for a copy of another server's channel that's free here.
fn free_channel_name(conn: &rusqlite::Connection, wanted: &str, server: &str) -> AppResult<String> {
    let base = super::normalize_channel_name(wanted).unwrap_or_else(|| "shared".to_owned());
    if !crate::store::channel_name_taken(conn, &base)? {
        return Ok(base);
    }
    let label: String = server
        .split(['.', ':'])
        .next()
        .unwrap_or("shared")
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect();
    for number in 1..100 {
        let candidate: String = if number == 1 {
            format!("{base}-{label}")
        } else {
            format!("{base}-{label}-{number}")
        }
        .chars()
        .take(40)
        .collect();
        if !crate::store::channel_name_taken(conn, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(AppError::bad_request(
        "Rename a channel here, then try again.",
    ))
}

/// Takes a channel another server offered: a copy of it here.
async fn take_offer(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let (handle, url) = here_of(&state)?;
    let user_id = user.id;
    let now = now_ms();
    let channel = state
        .db
        .call(move |conn| {
            let offer = data::take_offer(conn, id)?.ok_or(AppError::NotFound)?;
            let instance = data::instance(conn, offer.instance_id)?
                .filter(|instance| instance.status == Status::Connected)
                .ok_or_else(|| AppError::bad_request("That server isn't connected any more."))?;
            let name = free_channel_name(conn, &offer.name, &instance.handle)?;
            let channel = crate::store::create_channel(conn, &name, offer.private, user_id, now)?;
            crate::store::set_topic(conn, channel, &offer.topic)?;
            data::save_share(
                conn,
                &data::Share {
                    channel_id: channel,
                    instance_id: instance.id,
                    role: data::Role::Guest,
                    remote_channel_id: Some(offer.remote_channel_id),
                    status: data::ShareStatus::Active,
                },
                now,
            )?;
            data::enqueue(
                conn,
                instance.id,
                &federation::events::Event::ShareAccepted {
                    channel: offer.remote_channel_id,
                    copy: channel,
                },
                now,
            )?;
            // A private copy starts with whoever took it.
            if offer.private {
                let own = data::Own {
                    handle: &handle,
                    url: &url,
                };
                if let Some(person) = data::person(conn, &own, user_id)? {
                    data::enqueue(
                        conn,
                        instance.id,
                        &federation::events::Event::Joined {
                            channel: offer.remote_channel_id,
                            person,
                        },
                        now,
                    )?;
                }
            }
            Ok(channel)
        })
        .await?;
    state.federation.wake.notify_one();
    Ok(Redirect::to(&format!("/c/{channel}")))
}

async fn decline_offer(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let now = now_ms();
    state
        .db
        .call(move |conn| {
            if let Some(offer) = data::take_offer(conn, id)? {
                data::enqueue(
                    conn,
                    offer.instance_id,
                    &federation::events::Event::ShareDeclined {
                        channel: offer.remote_channel_id,
                    },
                    now,
                )?;
            }
            Ok(())
        })
        .await?;
    state.federation.wake.notify_one();
    Ok(Redirect::to("/admin/connections"))
}

// People on other servers

#[derive(Deserialize)]
struct ElsewhereQuery {
    #[serde(default)]
    q: String,
}

/// Someone found on a connected server.
pub struct Found {
    pub server: data::Instance,
    pub id: i64,
    pub username: String,
    pub display_name: String,
}

/// Searches the connected servers whose people may be written to.
async fn search_elsewhere(state: &AppState, query: &str) -> AppResult<(Vec<Found>, Vec<String>)> {
    let instances = state.db.call(|conn| data::instances(conn)).await?;
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let query: String = query
        .trim()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_'))
        .take(50)
        .collect();
    if query.chars().count() < 2 {
        return Ok((found, problems));
    }
    for instance in instances
        .into_iter()
        .filter(|instance| instance.status == Status::Connected)
    {
        let path = format!("/federation/people?q={}", query.replace(' ', "%20"));
        match state
            .federation
            .send("GET", &instance.url, &path, Vec::new())
            .await
        {
            Ok((200, body)) => {
                let listed: Vec<Listed> = serde_json::from_slice::<serde_json::Value>(&body)
                    .ok()
                    .and_then(|mut answer| {
                        serde_json::from_value(answer.get_mut("people")?.take()).ok()
                    })
                    .unwrap_or_default();
                for person in listed {
                    found.push(Found {
                        server: instance.clone(),
                        id: person.id,
                        username: person.username,
                        display_name: person.display_name,
                    });
                }
            }
            Ok((403, _)) => {}
            Ok((status, _)) => problems.push(format!("{} answered {status}", instance.handle)),
            Err(error) => problems.push(format!("{}: {error}", instance.handle)),
        }
    }
    Ok((found, problems))
}

async fn people_elsewhere(
    user: CurrentUser,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<ElsewhereQuery>,
) -> AppResult<Markup> {
    user.require(crate::community::Permission::StartDirectMessages)?;
    let (found, problems) = search_elsewhere(&state, &query.q).await?;
    let sidebar = shell_data(&state, user.id).await?;
    Ok(views::federation::elsewhere_page(
        &Shell {
            user: &user,
            sidebar: &sidebar,
            current: None,
        },
        &query.q,
        &found,
        &problems,
    ))
}

#[derive(Deserialize)]
struct MessageForm {
    server: i64,
    id: i64,
}

/// Starts (or opens) a direct conversation with someone on a connected
/// server.
async fn message_elsewhere(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<MessageForm>,
) -> AppResult<Redirect> {
    user.require(crate::community::Permission::StartDirectMessages)?;
    let (handle, url) = here_of(&state)?;
    let server = form.server;
    let instance = state
        .db
        .call(move |conn| data::instance(conn, server))
        .await?
        .filter(|instance| instance.status == Status::Connected)
        .ok_or_else(|| AppError::bad_request("That server isn't connected."))?;
    // Who they are, from their server rather than the form.
    let path = format!("/federation/people?id={}", form.id);
    let listed: Listed = match state
        .federation
        .send("GET", &instance.url, &path, Vec::new())
        .await
    {
        Ok((200, body)) => serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .and_then(|mut answer| {
                serde_json::from_value::<Vec<Listed>>(answer.get_mut("people")?.take()).ok()
            })
            .and_then(|people| people.into_iter().next())
            .ok_or_else(|| AppError::bad_request("They aren't there any more."))?,
        Ok(_) => {
            return Err(AppError::bad_request(
                "Their server doesn't let people there be written to.",
            ));
        }
        Err(error) => {
            return Err(AppError::bad_request(format!(
                "{} couldn't be reached: {error}",
                instance.handle
            )));
        }
    };
    let user_id = user.id;
    let now = now_ms();
    let conversation = state
        .db
        .call(move |conn| {
            let own = data::Own {
                handle: &handle,
                url: &url,
            };
            let person = federation::events::Person {
                server: instance.handle.clone(),
                url: instance.url.clone(),
                id: listed.id,
                username: listed.username,
                display_name: listed.display_name,
            };
            let stand_in = data::account(conn, &own, &person, now)?
                .ok_or_else(|| AppError::bad_request("You can't write to them."))?;
            let conversation = crate::store::direct_channel(conn, user_id, stand_in, now)?;
            if data::shares_of(conn, conversation)?.is_empty() {
                let me = data::person(conn, &own, user_id)?.ok_or(AppError::NotFound)?;
                data::save_share(
                    conn,
                    &data::Share {
                        channel_id: conversation,
                        instance_id: instance.id,
                        role: data::Role::Host,
                        remote_channel_id: None,
                        status: data::ShareStatus::Invited,
                    },
                    now,
                )?;
                data::enqueue(
                    conn,
                    instance.id,
                    &federation::events::Event::Share {
                        channel: conversation,
                        name: String::new(),
                        topic: String::new(),
                        private: true,
                        direct: Some(federation::events::Direct {
                            from: me,
                            to: listed.id,
                        }),
                    },
                    now,
                )?;
            }
            Ok(conversation)
        })
        .await?;
    state.federation.wake.notify_one();
    Ok(Redirect::to(&format!("/c/{conversation}")))
}
