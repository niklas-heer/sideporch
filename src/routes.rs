use axum::{
    Form, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;

use crate::{
    AppState, assets,
    auth::{self, CurrentUser},
    automations,
    error::{AppError, AppResult},
    files::{self, MessageInput},
    messages::{self, Draft, Sender},
    now_ms, push, realtime, search,
    store::{self, ChannelKind},
    views::{self, AccountForm, ChannelView, Render, Shell},
    webhook,
};

/// Messages shown per page of channel history.
const PAGE_SIZE: usize = 100;
const PAGE_FETCH: u32 = 101;
const MAX_MESSAGE_CHARS: usize = 10_000;
const INVITE_DAYS: i64 = 7;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/home", get(home))
        .route("/login", get(login_form).post(login))
        .route("/logout", post(logout))
        .route("/setup/{token}", get(setup_form).post(setup))
        .route("/join/{token}", get(join_form).post(join))
        .route("/channels/new", get(new_channel_form))
        .route("/channels", post(create_channel))
        .route("/c/{channel_id}", get(channel))
        .route("/c/{channel_id}/t/{message_id}", get(thread))
        .route(
            "/c/{channel_id}/messages",
            post(post_message).layer(DefaultBodyLimit::max(files::UPLOAD_BODY_LIMIT)),
        )
        .route("/c/{channel_id}/m/{message_id}/react", get(react_page))
        .route("/c/{channel_id}/m/{message_id}/reactions", post(react))
        .route("/files/{file_id}", get(files::download))
        .route(
            "/emoji",
            get(files::emoji_page)
                .post(files::add_emoji)
                .layer(DefaultBodyLimit::max(files::UPLOAD_BODY_LIMIT)),
        )
        .route("/emoji/{name}/delete", post(files::delete_emoji))
        .route("/search", get(search::search))
        .route("/push/key", get(push::public_key))
        .route("/push/subscriptions", post(push::subscribe))
        .route("/push/unsubscribe", post(push::unsubscribe))
        .route("/automations", get(automation_list).post(create_automation))
        .route("/automations/new", get(new_automation))
        .route(
            "/automations/{automation_id}",
            get(edit_automation).post(update_automation),
        )
        .route(
            "/automations/{automation_id}/delete",
            post(delete_automation),
        )
        .route("/c/{channel_id}/settings", get(channel_settings))
        .route("/c/{channel_id}/topic", post(set_topic))
        .route("/c/{channel_id}/webhooks", post(create_webhook))
        .route(
            "/c/{channel_id}/webhooks/{webhook_id}/delete",
            post(delete_webhook),
        )
        .route("/dm/{user_id}", get(direct_message))
        .route("/people", get(people))
        .route("/invites", post(create_invite))
        .route("/invites/{token}/revoke", post(revoke_invite))
        .route("/hooks/{token}", post(incoming_webhook))
        .route("/ws", get(realtime::connect))
        .route("/healthz", get(|| async { "ok" }))
        .merge(assets::router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::same_origin,
        ))
        .layer(middleware::from_fn(security_headers))
        .layer(DefaultBodyLimit::max(256 * 1024))
        .with_state(state)
}

async fn security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' https: http: data:; style-src 'self' 'unsafe-inline'; \
             script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    response
}

/// The URL people use to reach this server, for invite and webhook links.
fn base_url(state: &AppState, headers: &HeaderMap) -> String {
    if let Some(url) = &state.public_url {
        return url.clone();
    }
    let header_value = |name: &str| headers.get(name).and_then(|value| value.to_str().ok());
    let scheme = header_value("x-forwarded-proto").unwrap_or("http");
    let host = header_value("x-forwarded-host")
        .or_else(|| header_value(header::HOST.as_str()))
        .unwrap_or("localhost");
    format!("{scheme}://{host}")
}

fn redirect_with_cookie(to: &str, cookie: &str) -> AppResult<Response> {
    let mut response = Redirect::to(to).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(cookie).map_err(AppError::internal)?,
    );
    Ok(response)
}

async fn signed_in(state: &AppState, headers: &HeaderMap) -> AppResult<Option<CurrentUser>> {
    match auth::session_token(headers) {
        Some(token) => auth::lookup_session(state, token).await,
        None => Ok(None),
    }
}

async fn index(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if signed_in(&state, &headers).await?.is_none() {
        if state.setup_pending() {
            return Ok((
                StatusCode::SERVICE_UNAVAILABLE,
                views::error_page(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "This Sideporch isn't set up yet. Open the setup link printed in the server log.",
                ),
            )
                .into_response());
        }
        return Ok(Redirect::to("/login").into_response());
    }
    let home = state.db.call(|conn| store::home_channel(conn)).await?;
    Ok(home
        .map_or_else(
            || Redirect::to("/home"),
            |id| Redirect::to(&format!("/c/{id}")),
        )
        .into_response())
}

pub async fn shell_data(state: &AppState, user_id: i64) -> AppResult<store::Sidebar> {
    state
        .db
        .call(move |conn| store::sidebar(conn, user_id))
        .await
}

async fn home(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let sidebar = shell_data(&state, user.id).await?;
    Ok(views::home_page(&Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    }))
}

// Accounts

#[derive(Deserialize)]
struct LoginForm {
    username: String,
    password: String,
}

async fn login_form(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if signed_in(&state, &headers).await?.is_some() {
        return Ok(Redirect::to("/").into_response());
    }
    Ok(views::login_page(None, "").into_response())
}

async fn login(State(state): State<AppState>, Form(form): Form<LoginForm>) -> AppResult<Response> {
    let username = form.username.trim().to_owned();
    let lookup = username.clone();
    let record = state
        .db
        .call(move |conn| store::login_record(conn, &lookup))
        .await?;
    let verified = match record {
        Some((id, hash)) => auth::verify_password(form.password, hash)
            .await?
            .then_some(id),
        None => None,
    };
    let Some(user_id) = verified else {
        return Ok((
            StatusCode::UNAUTHORIZED,
            views::login_page(Some("That username and password don't match."), &username),
        )
            .into_response());
    };
    let cookie = auth::start_session(&state, user_id).await?;
    redirect_with_cookie("/", &cookie)
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> AppResult<Response> {
    if let Some(token) = auth::session_token(&headers) {
        auth::end_session(&state, token).await?;
    }
    redirect_with_cookie("/login", &auth::clear_cookie(&state))
}

#[derive(Deserialize)]
struct AccountInput {
    display_name: String,
    username: String,
    password: String,
}

struct ValidAccount {
    display_name: String,
    username: String,
    password: String,
}

fn validate_account(input: AccountInput) -> Result<ValidAccount, (&'static str, AccountForm)> {
    let display_name = input.display_name.trim().to_owned();
    let username = input.username.trim().to_lowercase();
    let form = AccountForm {
        display_name: display_name.clone(),
        username: username.clone(),
    };
    let username_ok = (2..=32).contains(&username.chars().count())
        && username
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '_'));
    if !(1..=64).contains(&display_name.chars().count()) {
        return Err(("Enter a name between 1 and 64 characters.", form));
    }
    if !username_ok {
        return Err((
            "Usernames are 2 to 32 characters: letters, numbers, dots, dashes and underscores.",
            form,
        ));
    }
    if !(8..=256).contains(&input.password.chars().count()) {
        return Err(("Choose a password with at least 8 characters.", form));
    }
    Ok(ValidAccount {
        display_name,
        username,
        password: input.password,
    })
}

async fn setup_form(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Markup> {
    if !state.setup_token_matches(&token) {
        return Err(AppError::NotFound);
    }
    Ok(views::setup_page(&token, None, &AccountForm::default()))
}

async fn setup(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(input): Form<AccountInput>,
) -> AppResult<Response> {
    if !state.setup_token_matches(&token) {
        return Err(AppError::NotFound);
    }
    let account = match validate_account(input) {
        Ok(account) => account,
        Err((error, form)) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                views::setup_page(&token, Some(error), &form),
            )
                .into_response());
        }
    };
    let hash = auth::hash_password(account.password).await?;
    let now = now_ms();
    let created = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if store::user_count(&tx)? > 0 {
                return Ok(None);
            }
            let id = store::create_user(
                &tx,
                &account.username,
                &account.display_name,
                &hash,
                true,
                now,
            )?;
            store::create_channel(&tx, "general", id, now)?;
            tx.commit()?;
            Ok(Some(id))
        })
        .await?;
    let user_id = created.ok_or(AppError::NotFound)?;
    state.finish_setup();
    let cookie = auth::start_session(&state, user_id).await?;
    redirect_with_cookie("/", &cookie)
}

async fn check_invite(state: &AppState, token: &str) -> AppResult<()> {
    let token = token.to_owned();
    let now = now_ms();
    let valid = state
        .db
        .call(move |conn| store::invite_is_valid(conn, &token, now))
        .await?;
    if valid {
        Ok(())
    } else {
        Err(AppError::Gone(
            "This invite link has expired or was revoked. Ask for a new one.".to_owned(),
        ))
    }
}

async fn join_form(State(state): State<AppState>, Path(token): Path<String>) -> AppResult<Markup> {
    check_invite(&state, &token).await?;
    Ok(views::join_page(&token, None, &AccountForm::default()))
}

async fn join(
    State(state): State<AppState>,
    Path(token): Path<String>,
    Form(input): Form<AccountInput>,
) -> AppResult<Response> {
    check_invite(&state, &token).await?;
    let account = match validate_account(input) {
        Ok(account) => account,
        Err((error, form)) => {
            return Ok((
                StatusCode::BAD_REQUEST,
                views::join_page(&token, Some(error), &form),
            )
                .into_response());
        }
    };
    let hash = auth::hash_password(account.password).await?;
    let now = now_ms();
    let invite = token.clone();
    let username = account.username.clone();
    let display_name = account.display_name.clone();
    let created = state
        .db
        .call(move |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            if !store::invite_is_valid(&tx, &invite, now)? {
                return Ok(Err(
                    "This invite link has expired or was revoked. Ask for a new one.",
                ));
            }
            if store::username_taken(&tx, &username)? {
                return Ok(Err("That username is taken. Try another one."));
            }
            let id = store::create_user(&tx, &username, &display_name, &hash, false, now)?;
            store::record_invite_use(&tx, &invite)?;
            tx.commit()?;
            Ok(Ok(id))
        })
        .await?;
    match created {
        Ok(user_id) => {
            let cookie = auth::start_session(&state, user_id).await?;
            redirect_with_cookie("/", &cookie)
        }
        Err(error) => {
            let form = AccountForm {
                display_name: account.display_name,
                username: account.username,
            };
            Ok((
                StatusCode::BAD_REQUEST,
                views::join_page(&token, Some(error), &form),
            )
                .into_response())
        }
    }
}

// Channels

#[derive(Deserialize)]
struct HistoryQuery {
    before: Option<i64>,
}

async fn channel(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Query(query): Query<HistoryQuery>,
) -> AppResult<Markup> {
    render_channel(&state, &user, channel_id, query.before, None).await
}

async fn thread(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Markup> {
    render_channel(&state, &user, channel_id, None, Some(message_id)).await
}

async fn render_channel(
    state: &AppState,
    user: &CurrentUser,
    channel_id: i64,
    before: Option<i64>,
    thread: Option<i64>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (channel, messages, older, thread, sidebar, ctx) = state
        .db
        .call(move |conn| {
            let channel =
                store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            let mut messages = store::channel_messages(conn, channel_id, before, PAGE_FETCH)?;
            let older = if messages.len() > PAGE_SIZE {
                messages.remove(0);
                messages.first().map(|message| message.id)
            } else {
                None
            };
            let thread = match thread {
                Some(root_id) => {
                    let root = store::message(conn, root_id)?
                        .filter(|root| root.channel_id == channel_id && root.parent_id.is_none())
                        .ok_or(AppError::NotFound)?;
                    let replies = store::replies(conn, root_id)?;
                    Some((root, replies))
                }
                None => None,
            };
            if before.is_none()
                && let Some(latest) = store::latest_message_id(conn, channel_id)?
            {
                store::mark_read(conn, user_id, channel_id, latest)?;
            }
            let sidebar = store::sidebar(conn, user_id)?;
            let ctx = store::render_context(conn)?;
            Ok((channel, messages, older, thread, sidebar, ctx))
        })
        .await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: Some(channel.id),
    };
    Ok(views::channel_page(
        &shell,
        &ChannelView {
            channel: &channel,
            messages: &messages,
            older,
            thread: thread
                .as_ref()
                .map(|(root, replies)| (root, replies.as_slice())),
            render: &Render::for_user(&ctx, user_id),
        },
    ))
}

/// Browsers running app.js send this header and read `204` as success.
fn wants_no_content(headers: &HeaderMap) -> bool {
    headers.contains_key("x-sideporch-fetch")
}

async fn post_message(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    headers: HeaderMap,
    input: MessageInput,
) -> AppResult<Response> {
    let body = input.body.trim().to_owned();
    if body.is_empty() && input.files.is_empty() {
        return Err(AppError::bad_request(
            "Write something or attach a file before sending.",
        ));
    }
    if body.chars().count() > MAX_MESSAGE_CHARS {
        return Err(AppError::bad_request(
            "Messages can be at most 10,000 characters long.",
        ));
    }
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::channel_for(conn, channel_id, user_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let files = files::store_uploads(&state, user_id, input.files).await?;
    let message = messages::post(
        &state,
        Draft {
            channel_id,
            parent_id: input.parent_id,
            sender: Sender::User(user_id),
            body,
            attachments: Vec::new(),
            files,
        },
    )
    .await?;
    if wants_no_content(&headers) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let target = message.parent_id.map_or_else(
        || format!("/c/{channel_id}"),
        |parent| format!("/c/{channel_id}/t/{parent}"),
    );
    Ok(Redirect::to(&target).into_response())
}

async fn react_page(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Markup> {
    let user_id = user.id;
    let (sidebar, ctx) = state
        .db
        .call(move |conn| {
            store::channel_for(conn, channel_id, user_id)?.ok_or(AppError::NotFound)?;
            store::message(conn, message_id)?
                .filter(|message| message.channel_id == channel_id)
                .ok_or(AppError::NotFound)?;
            Ok((store::sidebar(conn, user_id)?, store::render_context(conn)?))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: Some(channel_id),
    };
    Ok(views::react_page(&shell, channel_id, message_id, &ctx))
}

#[derive(Deserialize)]
struct ReactionForm {
    emoji: String,
}

async fn react(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ReactionForm>,
) -> AppResult<Response> {
    let emoji = form.emoji.trim().trim_matches(':').to_owned();
    messages::toggle_reaction(&state, user.id, channel_id, message_id, emoji).await?;
    if wants_no_content(&headers) {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    Ok(Redirect::to(&format!("/c/{channel_id}#m{message_id}")).into_response())
}

async fn new_channel_form(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::new_channel_page(&shell, None, ""))
}

#[derive(Deserialize)]
struct ChannelForm {
    name: String,
}

fn normalize_channel_name(raw: &str) -> Option<String> {
    let name = raw
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_lowercase()
        .replace(' ', "-");
    let valid = (1..=40).contains(&name.chars().count())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    valid.then_some(name)
}

async fn create_channel(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<ChannelForm>,
) -> AppResult<Response> {
    let user_id = user.id;
    let now = now_ms();
    let result = match normalize_channel_name(&form.name) {
        Some(name) => {
            state
                .db
                .call(move |conn| {
                    if store::public_channel_id(conn, &name)?.is_some() {
                        return Ok(Err("A channel with that name already exists."));
                    }
                    Ok(Ok(store::create_channel(conn, &name, user_id, now)?))
                })
                .await?
        }
        None => Err("Use 1 to 40 lowercase letters, numbers, dashes or underscores."),
    };
    match result {
        Ok(id) => Ok(Redirect::to(&format!("/c/{id}")).into_response()),
        Err(error) => {
            let sidebar = shell_data(&state, user.id).await?;
            let shell = Shell {
                user: &user,
                sidebar: &sidebar,
                current: None,
            };
            Ok((
                StatusCode::BAD_REQUEST,
                views::new_channel_page(&shell, Some(error), &form.name),
            )
                .into_response())
        }
    }
}

/// Loads a public channel the user may manage.
async fn managed_channel(
    state: &AppState,
    user: &CurrentUser,
    channel_id: i64,
) -> AppResult<store::Channel> {
    let user_id = user.id;
    let channel = state
        .db
        .call(move |conn| store::channel_for(conn, channel_id, user_id))
        .await?
        .ok_or(AppError::NotFound)?;
    if channel.kind == ChannelKind::Public {
        Ok(channel)
    } else {
        Err(AppError::NotFound)
    }
}

async fn channel_settings(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    let channel = managed_channel(&state, &user, channel_id).await?;
    let user_id = user.id;
    let (hooks, sidebar) = state
        .db
        .call(move |conn| {
            Ok((
                store::webhooks(conn, channel_id)?,
                store::sidebar(conn, user_id)?,
            ))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: Some(channel.id),
    };
    Ok(views::channel_settings_page(
        &shell,
        &channel,
        &hooks,
        &base_url(&state, &headers),
    ))
}

#[derive(Deserialize)]
struct TopicForm {
    topic: String,
}

async fn set_topic(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<TopicForm>,
) -> AppResult<Response> {
    managed_channel(&state, &user, channel_id).await?;
    let topic: String = form.topic.trim().chars().take(200).collect();
    state
        .db
        .call(move |conn| store::set_topic(conn, channel_id, &topic))
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")).into_response())
}

#[derive(Deserialize)]
struct WebhookForm {
    name: String,
}

async fn create_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(channel_id): Path<i64>,
    Form(form): Form<WebhookForm>,
) -> AppResult<Response> {
    managed_channel(&state, &user, channel_id).await?;
    let name: String = form.name.trim().chars().take(80).collect();
    if name.is_empty() {
        return Err(AppError::bad_request(
            "Give the webhook a name, such as the tool that will use it.",
        ));
    }
    let token = auth::random_token()?;
    let user_id = user.id;
    let now = now_ms();
    state
        .db
        .call(move |conn| store::create_webhook(conn, channel_id, &name, &token, user_id, now))
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")).into_response())
}

async fn delete_webhook(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, webhook_id)): Path<(i64, i64)>,
) -> AppResult<Response> {
    managed_channel(&state, &user, channel_id).await?;
    state
        .db
        .call(move |conn| store::delete_webhook(conn, channel_id, webhook_id))
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}/settings")).into_response())
}

async fn incoming_webhook(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let parsed = match webhook::parse(content_type, &body) {
        Ok(parsed) => parsed,
        Err(rejection) => return (StatusCode::BAD_REQUEST, rejection.code()).into_response(),
    };
    let override_channel = parsed.channel.clone();
    let target = state
        .db
        .call(move |conn| {
            let Some(hook) = store::webhook_by_token(conn, &token)? else {
                return Ok(None);
            };
            // Like Mattermost, a payload may pick another public channel by name.
            let channel_id = match override_channel.as_deref() {
                Some(name) => store::public_channel_id(conn, name)?.unwrap_or(hook.channel_id),
                None => hook.channel_id,
            };
            Ok(Some((hook, channel_id)))
        })
        .await;
    let (hook, channel_id) = match target {
        Ok(Some(target)) => target,
        Ok(None) => return (StatusCode::NOT_FOUND, "no_service").into_response(),
        Err(error) => {
            tracing::error!(?error, "webhook lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "internal_error").into_response();
        }
    };
    let draft = Draft {
        channel_id,
        parent_id: None,
        sender: Sender::Webhook {
            id: hook.id,
            name: parsed.username.unwrap_or(hook.name),
            icon: parsed.icon_url,
        },
        body: parsed.text,
        attachments: parsed.attachments,
        files: Vec::new(),
    };
    match messages::post(&state, draft).await {
        Ok(_) => (StatusCode::OK, "ok").into_response(),
        Err(error) => {
            tracing::error!(?error, "webhook delivery failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal_error").into_response()
        }
    }
}

// People and invites

async fn direct_message(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(other): Path<i64>,
) -> AppResult<Response> {
    let user_id = user.id;
    let now = now_ms();
    let channel_id = state
        .db
        .call(move |conn| {
            if !store::user_exists(conn, other)? {
                return Err(AppError::NotFound);
            }
            store::direct_channel(conn, user_id, other, now)
        })
        .await?;
    Ok(Redirect::to(&format!("/c/{channel_id}")).into_response())
}

async fn people(
    user: CurrentUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> AppResult<Markup> {
    let user_id = user.id;
    let is_admin = user.is_admin;
    let now = now_ms();
    let (users, invites, sidebar) = state
        .db
        .call(move |conn| {
            let invites = if is_admin {
                store::active_invites(conn, now)?
            } else {
                Vec::new()
            };
            Ok((store::users(conn)?, invites, store::sidebar(conn, user_id)?))
        })
        .await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::people_page(
        &shell,
        &users,
        &invites,
        &base_url(&state, &headers),
    ))
}

async fn create_invite(user: CurrentUser, State(state): State<AppState>) -> AppResult<Response> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    let token = auth::random_token()?;
    let now = now_ms();
    let expires = now.saturating_add(INVITE_DAYS.saturating_mul(24 * 60 * 60 * 1000));
    let user_id = user.id;
    state
        .db
        .call(move |conn| store::create_invite(conn, &token, user_id, now, expires))
        .await?;
    Ok(Redirect::to("/people").into_response())
}

async fn revoke_invite(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> AppResult<Response> {
    if !user.is_admin {
        return Err(AppError::Forbidden);
    }
    state
        .db
        .call(move |conn| store::revoke_invite(conn, &token))
        .await?;
    Ok(Redirect::to("/people").into_response())
}

// Automations (admins only)

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn automation_list(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let list = state.db.call(|conn| store::automations(conn)).await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::automations::list_page(&shell, &list))
}

async fn automation_editor(
    state: &AppState,
    user: &CurrentUser,
    editor: &views::automations::Editor<'_>,
) -> AppResult<Markup> {
    let sidebar = shell_data(state, user.id).await?;
    let shell = Shell {
        user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(views::automations::editor_page(&shell, editor))
}

async fn new_automation(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let editor = views::automations::Editor {
        id: None,
        name: "",
        source: automations::EXAMPLE,
        enabled: true,
        last_error: None,
        form_error: None,
    };
    automation_editor(&state, &user, &editor).await
}

async fn edit_automation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
) -> AppResult<Markup> {
    require_admin(&user)?;
    let automation = state
        .db
        .call(move |conn| store::automation(conn, automation_id))
        .await?
        .ok_or(AppError::NotFound)?;
    let editor = views::automations::Editor {
        id: Some(automation.id),
        name: &automation.name,
        source: &automation.source,
        enabled: automation.enabled,
        last_error: automation.last_error.as_deref(),
        form_error: None,
    };
    automation_editor(&state, &user, &editor).await
}

#[derive(Deserialize)]
struct AutomationForm {
    name: String,
    source: String,
    enabled: Option<String>,
}

async fn save_automation(
    state: &AppState,
    user: &CurrentUser,
    id: Option<i64>,
    form: AutomationForm,
) -> AppResult<Response> {
    require_admin(user)?;
    let name: String = form.name.trim().chars().take(80).collect();
    let enabled = form.enabled.is_some();
    if name.is_empty() || form.source.len() > 100_000 {
        let editor = views::automations::Editor {
            id,
            name: &name,
            source: &form.source,
            enabled,
            last_error: None,
            form_error: Some("Give the automation a name, and keep the script under 100 kB."),
        };
        let page = automation_editor(state, user, &editor).await?;
        return Ok((StatusCode::BAD_REQUEST, page).into_response());
    }
    let user_id = user.id;
    let now = now_ms();
    let source = form.source;
    let saved = state
        .db
        .call(move |conn| {
            if let Some(id) = id {
                store::automation(conn, id)?.ok_or(AppError::NotFound)?;
            }
            store::save_automation(conn, id, &name, &source, enabled, user_id, now)
        })
        .await?;
    state.automations.reload(state).await?;
    Ok(Redirect::to(&format!("/automations/{saved}")).into_response())
}

async fn create_automation(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<AutomationForm>,
) -> AppResult<Response> {
    save_automation(&state, &user, None, form).await
}

async fn update_automation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
    Form(form): Form<AutomationForm>,
) -> AppResult<Response> {
    save_automation(&state, &user, Some(automation_id), form).await
}

async fn delete_automation(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(automation_id): Path<i64>,
) -> AppResult<Response> {
    require_admin(&user)?;
    state
        .db
        .call(move |conn| store::delete_automation(conn, automation_id))
        .await?;
    state.automations.reload(&state).await?;
    Ok(Redirect::to("/automations").into_response())
}

#[cfg(test)]
mod tests {
    use super::normalize_channel_name;

    #[test]
    fn normalizes_channel_names() {
        assert_eq!(
            normalize_channel_name(" #Garden Club ").as_deref(),
            Some("garden-club")
        );
        assert_eq!(
            normalize_channel_name("alerts_2").as_deref(),
            Some("alerts_2")
        );
        assert_eq!(normalize_channel_name("bad/name"), None);
        assert_eq!(normalize_channel_name("#"), None);
    }
}
