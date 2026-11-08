// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! The chat API for bots, against a real server: authentication, each scope,
//! the public-only boundary, posting through the shared pipeline, pagination,
//! rate limits, and what is stored.

mod common;

use std::time::Duration;

use common::{Browser, Live, Server, admin, between, home_channel, invite, location, start};
use reqwest::{Client, Method, StatusCode, header::HeaderMap, redirect::Policy};
use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const ALL_SCOPES: [&str; 3] = ["channels:read", "messages:read", "messages:write"];

/// The server's database, waiting briefly on its write lock.
fn db(server: &Server) -> Connection {
    let conn = Connection::open(server.data_dir().join("sideporch.db")).unwrap();
    conn.busy_timeout(Duration::from_secs(5)).unwrap();
    conn
}

/// Makes a channel as `admin`, public or private, and returns its id.
async fn make_channel(admin: &Browser, name: &str, private: bool) -> i64 {
    let mut form = vec![("name", name)];
    if private {
        form.push(("private", "on"));
    }
    let created = admin.post("/channels", &form).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    location(&created)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

/// Makes a bot named `name` with one token, as `admin` does on the Bots page.
/// `channels` is `None` for every public channel. Returns the bot's id and
/// the token, read from the page that shows it once.
async fn make_bot(
    server: &Server,
    admin: &Browser,
    name: &str,
    scopes: &[&str],
    channels: Option<&[i64]>,
    expires: &str,
) -> (i64, String) {
    let created = admin
        .post(
            "/settings/bots",
            &[
                ("name", name),
                ("display_name", "Deploys"),
                ("icon", ":rocket:"),
            ],
        )
        .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let bot_id: i64 = db(server)
        .query_row("SELECT id FROM bots WHERE name = ?1", [name], |row| {
            row.get(0)
        })
        .unwrap();
    let mut form: Vec<(&str, &str)> = vec![
        ("label", "Deploy token"),
        ("expires", expires),
        (
            "channel_scope",
            if channels.is_some() { "list" } else { "all" },
        ),
    ];
    for scope in scopes {
        form.push(("scope", *scope));
    }
    let channel_ids: Vec<String> = channels
        .unwrap_or_default()
        .iter()
        .map(i64::to_string)
        .collect();
    for id in &channel_ids {
        form.push(("channel", id.as_str()));
    }
    let response = admin
        .post(&format!("/settings/bots/{bot_id}/tokens"), &form)
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page = response.text().await.unwrap();
    let token = format!("spb_{}", between(&page, "data-copy=\"spb_", "\""));
    (bot_id, token)
}

/// The id of the newest message in a channel.
fn newest_message(server: &Server, channel_id: i64) -> i64 {
    db(server)
        .query_row(
            "SELECT MAX(id) FROM messages WHERE channel_id = ?1",
            [channel_id],
            |row| row.get(0),
        )
        .unwrap()
}

/// A direct conversation between `admin` and the person with `user_id`.
async fn direct_conversation(admin: &Browser, user_id: i64) -> i64 {
    let response = admin.get(&format!("/dm/{user_id}")).await;
    location(&response)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

/// What an API answer looked like.
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    json: Value,
}

impl Answer {
    fn error_code(&self) -> &str {
        self.json["error"]["code"].as_str().unwrap_or_default()
    }
}

/// Talks to the API the way a bot's program does: no cookies, a bearer token.
struct Api {
    base: String,
    client: Client,
}

impl Api {
    fn new(server: &Server) -> Self {
        Self {
            base: server.base.clone(),
            client: Client::builder().redirect(Policy::none()).build().unwrap(),
        }
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        token: Option<&str>,
        body: Option<Value>,
    ) -> Answer {
        let mut request = self.client.request(method, format!("{}{path}", self.base));
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap();
        let json = serde_json::from_str(&text)
            .unwrap_or_else(|_| panic!("{path} answered something other than JSON: {text}"));
        Answer {
            status,
            headers,
            json,
        }
    }

    async fn get(&self, token: &str, path: &str) -> Answer {
        self.send(Method::GET, path, Some(token), None).await
    }

    async fn post(&self, token: &str, path: &str, body: Value) -> Answer {
        self.send(Method::POST, path, Some(token), Some(body)).await
    }
}

fn assert_refused(answer: &Answer, status: StatusCode, code: &str) {
    assert_eq!(answer.status, status, "{}", answer.json);
    assert_eq!(answer.error_code(), code, "{}", answer.json);
    assert!(
        answer.json["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "{}",
        answer.json
    );
}

/// Waits for the next live message event, skipping everything else.
async fn next_message(live: &mut Live) -> Value {
    for _ in 0..50 {
        if let Some(event) = live.next_event(Duration::from_millis(200)).await
            && event["type"] == "message"
        {
            return event;
        }
    }
    panic!("no live message arrived");
}

#[tokio::test]
async fn tokens_that_are_missing_wrong_revoked_or_expired_are_refused() {
    let server = start().await;
    let admin = admin(&server).await;
    let api = Api::new(&server);
    let (bot_id, token) = make_bot(&server, &admin, "deploy-bot", &ALL_SCOPES, None, "never").await;

    let missing = api.send(Method::GET, "/api/v1/me", None, None).await;
    assert_refused(&missing, StatusCode::UNAUTHORIZED, "missing_token");
    assert_eq!(missing.headers["www-authenticate"], "Bearer");

    let wrong = api.get("spb_not-a-token", "/api/v1/me").await;
    assert_refused(&wrong, StatusCode::UNAUTHORIZED, "invalid_token");

    let me = api.get(&token, "/api/v1/me").await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.json["bot"]["name"], "deploy-bot");
    assert_eq!(me.json["channels"], "all");

    // Expired: the token's expiry is in the past.
    let (_, expiring) = make_bot(&server, &admin, "old-bot", &ALL_SCOPES, None, "30").await;
    db(&server)
        .execute(
            "UPDATE bot_tokens SET expires_at = 1 WHERE bot_id = (SELECT id FROM bots WHERE name = 'old-bot')",
            [],
        )
        .unwrap();
    assert_refused(
        &api.get(&expiring, "/api/v1/me").await,
        StatusCode::UNAUTHORIZED,
        "invalid_token",
    );

    // Revoked token: refused on the very next request.
    let token_id: i64 = db(&server)
        .query_row(
            "SELECT id FROM bot_tokens WHERE bot_id = ?1",
            [bot_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        admin
            .post(&format!("/settings/bots/tokens/{token_id}/revoke"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_refused(
        &api.get(&token, "/api/v1/me").await,
        StatusCode::UNAUTHORIZED,
        "invalid_token",
    );

    // A revoked bot refuses every token it has, including new ones.
    let (other_id, other) = make_bot(&server, &admin, "tidy-bot", &ALL_SCOPES, None, "never").await;
    assert_eq!(
        api.get(&other, "/api/v1/channels").await.status,
        StatusCode::OK
    );
    assert_eq!(
        admin
            .post(&format!("/settings/bots/{other_id}/revoke"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_refused(
        &api.get(&other, "/api/v1/channels").await,
        StatusCode::UNAUTHORIZED,
        "invalid_token",
    );
}

#[tokio::test]
async fn each_scope_is_enforced() {
    let server = start().await;
    let admin = admin(&server).await;
    let channel = home_channel(&admin).await;
    let api = Api::new(&server);
    let message = json!({ "body": "hello from a bot" });
    let path = format!("/api/v1/channels/{channel}/messages");

    let (_, reader) = make_bot(&server, &admin, "reader", &["channels:read"], None, "never").await;
    assert_eq!(
        api.get(&reader, "/api/v1/channels").await.status,
        StatusCode::OK
    );
    assert_refused(
        &api.get(&reader, &path).await,
        StatusCode::FORBIDDEN,
        "missing_scope",
    );
    assert_refused(
        &api.post(&reader, &path, message.clone()).await,
        StatusCode::FORBIDDEN,
        "missing_scope",
    );

    let (_, history) = make_bot(
        &server,
        &admin,
        "history",
        &["messages:read"],
        None,
        "never",
    )
    .await;
    assert_eq!(api.get(&history, &path).await.status, StatusCode::OK);
    assert_refused(
        &api.get(&history, "/api/v1/channels").await,
        StatusCode::FORBIDDEN,
        "missing_scope",
    );
    assert_refused(
        &api.post(&history, &path, message.clone()).await,
        StatusCode::FORBIDDEN,
        "missing_scope",
    );

    let (_, writer) = make_bot(
        &server,
        &admin,
        "writer",
        &["messages:write"],
        None,
        "never",
    )
    .await;
    let posted = api.post(&writer, &path, message).await;
    assert_eq!(posted.status, StatusCode::CREATED, "{}", posted.json);
    assert_refused(
        &api.get(&writer, &path).await,
        StatusCode::FORBIDDEN,
        "missing_scope",
    );
    assert_refused(
        &api.get(&writer, "/api/v1/channels").await,
        StatusCode::FORBIDDEN,
        "missing_scope",
    );
}

#[tokio::test]
async fn a_post_reaches_live_clients_as_the_bot_and_is_traced_to_it() {
    let server = start().await;
    let admin = admin(&server).await;
    let channel = home_channel(&admin).await;
    let api = Api::new(&server);
    let (bot_id, token) = make_bot(&server, &admin, "deploy-bot", &ALL_SCOPES, None, "never").await;
    let mut live = admin.live_in(channel).await;

    let posted = api
        .post(
            &token,
            &format!("/api/v1/channels/{channel}/messages"),
            json!({ "body": "Deployed **v2** to staging" }),
        )
        .await;
    assert_eq!(posted.status, StatusCode::CREATED, "{}", posted.json);
    assert_eq!(posted.json["bot_id"], bot_id);
    assert_eq!(posted.json["author"]["type"], "bot");
    assert_eq!(posted.json["author"]["name"], "Deploys");

    let event = next_message(&mut live).await;
    assert_eq!(event["channel_id"], channel);
    assert_eq!(event["author"], "b:Deploys");
    assert!(event["html"].as_str().unwrap().contains("Deployed"));

    let stored: Option<i64> = db(&server)
        .query_row(
            "SELECT bot_id FROM messages WHERE id = ?1",
            [posted.json["id"].as_i64().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, Some(bot_id));

    // Reading shows the bot as the author, with the same name and no email.
    let page = api
        .get(&token, &format!("/api/v1/channels/{channel}/messages"))
        .await;
    let messages = page.json["messages"].as_array().unwrap();
    let author = &messages.last().unwrap()["author"];
    assert_eq!(author["type"], "bot");
    assert_eq!(author["name"], "Deploys");
    assert!(!page.json.to_string().contains("email"));
}

#[tokio::test]
async fn thread_replies_land_in_the_thread() {
    let server = start().await;
    let admin = admin(&server).await;
    let channel = home_channel(&admin).await;
    let api = Api::new(&server);
    let (_, token) = make_bot(&server, &admin, "ci-bot", &ALL_SCOPES, None, "never").await;
    assert!(
        admin
            .send(channel, "Build 42 failed", None)
            .await
            .is_success()
    );
    let root = newest_message(&server, channel);
    let mut live = admin.live_in(channel).await;

    let reply = api
        .post(
            &token,
            &format!("/api/v1/channels/{channel}/messages"),
            json!({ "body": "Re-running it now", "thread_id": root }),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.json);
    assert_eq!(reply.json["parent_id"], root);

    let event = next_message(&mut live).await;
    assert_eq!(event["parent_id"], root);
    assert_eq!(event["reply_count"], 1);

    let missing = api
        .post(
            &token,
            &format!("/api/v1/channels/{channel}/messages"),
            json!({ "body": "into nowhere", "thread_id": 999_999 }),
        )
        .await;
    assert_refused(&missing, StatusCode::NOT_FOUND, "not_found");
}

#[tokio::test]
async fn private_channels_and_direct_messages_are_not_found_even_on_an_explicit_list() {
    let server = start().await;
    let admin = admin(&server).await;
    let api = Api::new(&server);
    let public = make_channel(&admin, "releases", false).await;
    let private = make_channel(&admin, "secret-plans", true).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let bea_id = bea.user_id().await;
    let conversation = direct_conversation(&admin, bea_id).await;

    // A token that lists only `releases` can't reach anything else.
    let (_, token) = make_bot(
        &server,
        &admin,
        "listed",
        &ALL_SCOPES,
        Some(&[public]),
        "never",
    )
    .await;
    let channels = api.get(&token, "/api/v1/channels").await;
    assert_eq!(channels.json["channels"].as_array().unwrap().len(), 1);
    assert_eq!(channels.json["channels"][0]["name"], "releases");
    assert_eq!(
        api.get(&token, &format!("/api/v1/channels/{public}/messages"))
            .await
            .status,
        StatusCode::OK
    );
    let other_public = make_channel(&admin, "random", false).await;
    assert_refused(
        &api.get(&token, &format!("/api/v1/channels/{other_public}/messages"))
            .await,
        StatusCode::FORBIDDEN,
        "channel_not_allowed",
    );

    // Private channels and direct messages answer as missing, even when a
    // token's list names them (which only a direct edit can do).
    let (bot_id, listing) = make_bot(
        &server,
        &admin,
        "sneaky",
        &ALL_SCOPES,
        Some(&[public]),
        "never",
    )
    .await;
    for hidden in [private, conversation] {
        db(&server)
            .execute(
                "INSERT INTO bot_token_channels (token_id, channel_id)
                 SELECT id, ?2 FROM bot_tokens WHERE bot_id = ?1",
                rusqlite::params![bot_id, hidden],
            )
            .unwrap();
    }
    for hidden in [private, conversation] {
        let path = format!("/api/v1/channels/{hidden}/messages");
        assert_refused(
            &api.get(&listing, &path).await,
            StatusCode::NOT_FOUND,
            "not_found",
        );
        assert_refused(
            &api.post(&listing, &path, json!({ "body": "peek" })).await,
            StatusCode::NOT_FOUND,
            "not_found",
        );
    }
    assert_refused(
        &api.get(&listing, "/api/v1/channels/nope/messages").await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    let names: Vec<String> = api.get(&listing, "/api/v1/channels").await.json["channels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|channel| channel["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["releases"]);
}

#[tokio::test]
async fn a_channel_made_private_after_the_token_was_issued_stops_working() {
    let server = start().await;
    let admin = admin(&server).await;
    let api = Api::new(&server);
    let later = make_channel(&admin, "later-public", false).await;
    let (_, token) = make_bot(
        &server,
        &admin,
        "watcher",
        &ALL_SCOPES,
        Some(&[later]),
        "never",
    )
    .await;
    let path = format!("/api/v1/channels/{later}/messages");
    assert_eq!(api.get(&token, &path).await.status, StatusCode::OK);

    db(&server)
        .execute("UPDATE channels SET private = 1 WHERE id = ?1", [later])
        .unwrap();

    assert_refused(
        &api.get(&token, &path).await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    assert_refused(
        &api.post(&token, &path, json!({ "body": "still here?" }))
            .await,
        StatusCode::NOT_FOUND,
        "not_found",
    );
    let channels = api.get(&token, "/api/v1/channels").await;
    assert!(channels.json["channels"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn pages_of_messages_read_oldest_first_in_both_directions() {
    let server = start().await;
    let admin = admin(&server).await;
    let channel = home_channel(&admin).await;
    let api = Api::new(&server);
    let (_, token) = make_bot(&server, &admin, "reader", &ALL_SCOPES, None, "never").await;
    let path = format!("/api/v1/channels/{channel}/messages");
    let mut ids = Vec::new();
    for n in 1..=5 {
        assert!(
            admin
                .send(channel, &format!("message {n}"), None)
                .await
                .is_success()
        );
        ids.push(newest_message(&server, channel));
    }

    let newest = api.get(&token, &format!("{path}?limit=2")).await;
    assert_eq!(newest.status, StatusCode::OK);
    assert_eq!(newest.json["has_more"], true);
    let bodies = |answer: &Answer| -> Vec<String> {
        answer.json["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["body"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(bodies(&newest), ["message 4", "message 5"]);

    let before = api
        .get(&token, &format!("{path}?limit=2&before={}", ids[3]))
        .await;
    assert_eq!(bodies(&before), ["message 2", "message 3"]);
    assert_eq!(before.json["has_more"], true);

    let oldest = api
        .get(&token, &format!("{path}?limit=2&before={}", ids[1]))
        .await;
    assert_eq!(bodies(&oldest), ["message 1"]);
    assert_eq!(oldest.json["has_more"], false);

    let after = api
        .get(&token, &format!("{path}?limit=10&after={}", ids[2]))
        .await;
    assert_eq!(bodies(&after), ["message 4", "message 5"]);
    assert_eq!(after.json["has_more"], false);

    assert_refused(
        &api.get(&token, &format!("{path}?limit=0")).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    assert_refused(
        &api.get(&token, &format!("{path}?before=x")).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    assert_refused(
        &api.get(&token, &format!("{path}?before=1&after=2")).await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
}

#[tokio::test]
async fn bots_post_only_where_anyone_may() {
    let server = start().await;
    let admin = admin(&server).await;
    let channel = make_channel(&admin, "announcements", false).await;
    let api = Api::new(&server);
    let (_, token) = make_bot(&server, &admin, "notifier", &ALL_SCOPES, None, "never").await;
    let path = format!("/api/v1/channels/{channel}/messages");
    assert!(
        admin
            .send(channel, "Read this first", None)
            .await
            .is_success()
    );
    let root = newest_message(&server, channel);
    db(&server)
        .execute(
            "UPDATE channels SET post_policy = 'managers' WHERE id = ?1",
            [channel],
        )
        .unwrap();

    assert_refused(
        &api.post(&token, &path, json!({ "body": "news" })).await,
        StatusCode::FORBIDDEN,
        "posting_not_allowed",
    );
    let reply = api
        .post(
            &token,
            &path,
            json!({ "body": "a reply", "thread_id": root }),
        )
        .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.json);
}

#[tokio::test]
async fn bodies_are_checked_and_answers_are_json() {
    let server = start().await;
    let admin = admin(&server).await;
    let channel = home_channel(&admin).await;
    let api = Api::new(&server);
    let (_, token) = make_bot(&server, &admin, "checker", &ALL_SCOPES, None, "never").await;
    let path = format!("/api/v1/channels/{channel}/messages");

    for body in [
        json!({ "body": "   " }),
        json!({ "body": "x".repeat(10_001) }),
    ] {
        assert_refused(
            &api.post(&token, &path, body).await,
            StatusCode::BAD_REQUEST,
            "invalid_request",
        );
    }
    assert_refused(
        &api.post(&token, &path, json!({ "text": "wrong field" }))
            .await,
        StatusCode::BAD_REQUEST,
        "invalid_request",
    );
    let unknown = api.get(&token, "/api/v1/tomorrow").await;
    assert_refused(&unknown, StatusCode::NOT_FOUND, "not_found");
    let longest = api
        .post(&token, &path, json!({ "body": "x".repeat(10_000) }))
        .await;
    assert_eq!(longest.status, StatusCode::CREATED);
}

#[tokio::test]
async fn requests_are_rate_limited_per_token() {
    let server = start().await;
    let admin = admin(&server).await;
    let api = Api::new(&server);
    let (_, token) = make_bot(&server, &admin, "busy-bot", &ALL_SCOPES, None, "never").await;

    for _ in 0..120 {
        assert_eq!(api.get(&token, "/api/v1/me").await.status, StatusCode::OK);
    }
    let limited = api.get(&token, "/api/v1/me").await;
    assert_refused(&limited, StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    assert!(limited.headers.contains_key("retry-after"));

    // Another token has its own allowance.
    let (_, other) = make_bot(&server, &admin, "calm-bot", &ALL_SCOPES, None, "never").await;
    assert_eq!(api.get(&other, "/api/v1/me").await.status, StatusCode::OK);
}

#[tokio::test]
async fn tokens_are_shown_once_and_only_their_hash_is_stored() {
    let server = start().await;
    let admin = admin(&server).await;
    let (_, token) = make_bot(&server, &admin, "vault-bot", &ALL_SCOPES, None, "never").await;

    // Not shown again on the Bots page.
    let page = admin.page("/settings/bots").await;
    assert!(!page.contains(&token), "the token is on the page again");

    // Its hash is stored and the token itself is not, in the database or its
    // write-ahead log.
    let conn = db(&server);
    let stored: Vec<u8> = conn
        .query_row(
            "SELECT token_hash FROM bot_tokens ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(stored, Sha256::digest(token.as_bytes()).to_vec());
    drop(conn);
    for file in ["sideporch.db", "sideporch.db-wal"] {
        if let Ok(bytes) = std::fs::read(server.data_dir().join(file)) {
            assert!(
                !bytes
                    .windows(token.len())
                    .any(|window| window == token.as_bytes()),
                "{file} holds the token"
            );
        }
    }

    // Last use is recorded, and the first request after making it is enough.
    let api = Api::new(&server);
    assert_eq!(api.get(&token, "/api/v1/me").await.status, StatusCode::OK);
    let used: Option<i64> = db(&server)
        .query_row(
            "SELECT last_used_at FROM bot_tokens ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(used.is_some());
}

#[tokio::test]
async fn people_without_bot_rights_cannot_make_bots() {
    let server = start().await;
    let admin = admin(&server).await;
    let member = invite(&server, &admin, "Cy", "cy").await;
    let page = member.get("/settings/bots").await;
    assert_ne!(page.status(), StatusCode::OK);
    let made = member
        .post(
            "/settings/bots",
            &[("name", "sneaky"), ("display_name", "Sneaky")],
        )
        .await;
    assert_ne!(made.status(), StatusCode::SEE_OTHER);
    let count: i64 = db(&server)
        .query_row("SELECT COUNT(*) FROM bots", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
