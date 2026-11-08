// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Retry-safe sends and catching up after a dropped connection. A send
//! that carries a client nonce is stored once however often it arrives,
//! and the messages a page missed come back in order, to people who may
//! read the channel and to nobody else.

mod common;

use std::time::Duration;

use axum::{Json, Router, routing::post};
use common::{Browser, admin, home_channel, invite, last_message_id, location, start};
use reqwest::StatusCode;
use serde_json::{Value, json};

const NONCE: &str = "0123456789abcdef";

/// Sends `body` the way app.js does, with `nonce` as the send's id.
/// `fetch` says whether the request comes from app.js, which is answered
/// with `204` rather than a redirect.
async fn send_with_nonce(
    browser: &Browser,
    channel_id: i64,
    body: &str,
    nonce: &str,
    parent: Option<i64>,
    fetch: bool,
) -> reqwest::Response {
    let parent = parent.map(|id| id.to_string());
    let mut form = vec![("body", body), ("client_nonce", nonce)];
    if let Some(parent) = &parent {
        form.push(("parent_id", parent));
    }
    let mut request = browser
        .client
        .post(browser.url(&format!("/c/{channel_id}/messages")))
        .header("cookie", &browser.cookie)
        .form(&form);
    if fetch {
        request = request.header("x-sideporch-fetch", "1");
    }
    request.send().await.unwrap()
}

/// The newest message ids and bodies a channel's `since` answers with.
async fn since(browser: &Browser, channel_id: i64, query: &str) -> (StatusCode, Value) {
    let response = browser.get(&format!("/c/{channel_id}/since?{query}")).await;
    let status = response.status();
    let body = response.json().await.unwrap_or(Value::Null);
    (status, body)
}

#[tokio::test]
async fn a_retried_send_is_stored_once_and_answered_as_the_first_was() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;

    let first = send_with_nonce(&admin, general, "Lunch at noon?", NONCE, None, true).await;
    assert_eq!(first.status(), StatusCode::NO_CONTENT);
    let id = last_message_id(&admin.page(&format!("/c/{general}")).await);

    // The first response was lost, so app.js sends the same send again.
    let again = send_with_nonce(&admin, general, "Lunch at noon?", NONCE, None, true).await;
    assert_eq!(again.status(), first.status());
    let page = admin.page(&format!("/c/{general}")).await;
    assert_eq!(last_message_id(&page), id);
    assert_eq!(page.matches("Lunch at noon?").count(), 1);

    // A plain form send gets the same redirect either way.
    let first_redirect = send_with_nonce(
        &admin,
        general,
        "Plain form",
        "fedcba9876543210",
        None,
        false,
    )
    .await;
    let retried_redirect = send_with_nonce(
        &admin,
        general,
        "Plain form",
        "fedcba9876543210",
        None,
        false,
    )
    .await;
    assert_eq!(first_redirect.status(), StatusCode::SEE_OTHER);
    assert_eq!(retried_redirect.status(), first_redirect.status());
    assert_eq!(location(&retried_redirect), location(&first_redirect));
    assert_eq!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .matches("Plain form")
            .count(),
        1
    );
}

#[tokio::test]
async fn a_retried_thread_reply_is_stored_once() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    admin.send(general, "Thread starter", None).await;
    let root = last_message_id(&admin.page(&format!("/c/{general}")).await);

    for _ in 0..2 {
        let reply = send_with_nonce(&admin, general, "A reply", NONCE, Some(root), true).await;
        assert_eq!(reply.status(), StatusCode::NO_CONTENT);
    }
    let thread = admin.page(&format!("/c/{general}/t/{root}")).await;
    assert_eq!(thread.matches("A reply").count(), 1);
    assert!(thread.contains("1 reply"));
}

#[tokio::test]
async fn a_retry_delivers_once_to_sockets_and_outgoing_webhooks() {
    let (sent, mut received) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let app = Router::new().route(
        "/hook",
        post(move |Json(body): Json<Value>| {
            let sent = sent.clone();
            async move {
                sent.send(body).unwrap();
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/hook", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    // Local services need the automations' private network setting.
    admin
        .post(
            "/settings/automations",
            &[("timezone", "UTC"), ("allow_private_network", "on")],
        )
        .await;
    admin
        .post(
            &format!("/c/{general}/outgoing"),
            &[("name", "CI"), ("url", &url), ("triggers", "!ship")],
        )
        .await;
    let mut live = admin.live_in(general).await;

    for _ in 0..3 {
        let sent = send_with_nonce(&admin, general, "!ship it", NONCE, None, true).await;
        assert_eq!(sent.status(), StatusCode::NO_CONTENT);
    }

    // One live event, for the one message.
    let event = live.next_event(Duration::from_secs(5)).await.unwrap();
    assert_eq!(event["type"], "message");
    let id = event["id"].as_i64().unwrap();
    assert_eq!(
        last_message_id(&admin.page(&format!("/c/{general}")).await),
        id
    );
    assert!(live.next_event(Duration::from_millis(500)).await.is_none());

    // One webhook call, for the one message.
    let body = tokio::time::timeout(Duration::from_secs(5), received.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body["text"], "!ship it");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(received.try_recv().is_err());
}

#[tokio::test]
async fn concurrent_copies_of_one_send_store_one_message() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;

    let sends = (0..8).map(|_| send_with_nonce(&admin, general, "Sent at once", NONCE, None, true));
    for response in futures_util::future::join_all(sends).await {
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
    }
    assert_eq!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .matches("Sent at once")
            .count(),
        1
    );
}

#[tokio::test]
async fn the_same_nonce_from_two_people_stores_two_messages() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let grace = invite(&server, &admin, "Grace", "grace").await;

    assert_eq!(
        send_with_nonce(&admin, general, "Ada sends this", NONCE, None, true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        send_with_nonce(&grace, general, "Grace sends this", NONCE, None, true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let page = admin.page(&format!("/c/{general}")).await;
    assert_eq!(page.matches("Ada sends this").count(), 1);
    assert_eq!(page.matches("Grace sends this").count(), 1);
}

#[tokio::test]
async fn a_malformed_nonce_is_refused_and_nothing_is_stored() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let too_long = "a".repeat(65);
    for bad in [
        "short",
        "has spaces in this one",
        too_long.as_str(),
        "emoji-nonce-😀-here",
    ] {
        let response = send_with_nonce(&admin, general, "Should not post", bad, None, true).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "nonce {bad:?}");
    }
    assert_eq!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .matches("Should not post")
            .count(),
        0
    );
}

#[tokio::test]
async fn catch_up_returns_exactly_the_missed_messages_in_order() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    admin.send(general, "one", None).await;
    let seen = last_message_id(&admin.page(&format!("/c/{general}")).await);
    admin.send(general, "two", None).await;
    admin.send(general, "three", None).await;
    admin.send(general, "four", None).await;
    let page = admin.page(&format!("/c/{general}")).await;
    let newest = last_message_id(&page);

    let (status, caught) = since(&admin, general, &format!("after={seen}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(caught["complete"], true);
    let messages = caught["messages"].as_array().unwrap();
    let bodies: Vec<&str> = messages
        .iter()
        .map(|message| message["html"].as_str().unwrap())
        .collect();
    assert_eq!(messages.len(), 3);
    assert!(
        bodies[0].contains(">two<")
            && bodies[0].contains(&format!("id=\"m{}\"", messages[0]["id"]))
    );
    assert!(bodies[1].contains(">three<"));
    assert!(bodies[2].contains(">four<"));
    let ids: Vec<i64> = messages
        .iter()
        .map(|message| message["id"].as_i64().unwrap())
        .collect();
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(*ids.last().unwrap(), newest);

    // Nothing new: an empty, complete answer.
    let (_, caught) = since(&admin, general, &format!("after={newest}")).await;
    assert_eq!(caught["messages"].as_array().unwrap().len(), 0);
    assert_eq!(caught["complete"], true);
}

#[tokio::test]
async fn catch_up_covers_thread_replies_only_when_asked() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    admin.send(general, "Starter", None).await;
    let root = last_message_id(&admin.page(&format!("/c/{general}")).await);
    send_with_nonce(&admin, general, "Reply one", NONCE, Some(root), true).await;
    send_with_nonce(
        &admin,
        general,
        "Reply two",
        "fedcba9876543210",
        Some(root),
        true,
    )
    .await;

    let (_, in_thread) = since(&admin, general, &format!("after=0&thread={root}")).await;
    let replies: Vec<&str> = in_thread["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["html"].as_str().unwrap())
        .collect();
    assert_eq!(replies.len(), 2);
    assert!(replies[0].contains("Reply one") && replies[1].contains("Reply two"));

    // The channel's list leaves replies to their threads.
    let (_, channel) = since(&admin, general, "after=0").await;
    let listed: Vec<&Value> = channel["messages"].as_array().unwrap().iter().collect();
    assert!(listed.iter().all(|message| message["parent_id"].is_null()));
}

#[tokio::test]
async fn catch_up_says_when_more_came_than_it_returned() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    for index in 0..101 {
        admin.send(general, &format!("message {index}"), None).await;
    }
    let (_, capped) = since(&admin, general, "after=0").await;
    assert_eq!(capped["messages"].as_array().unwrap().len(), 100);
    assert_eq!(capped["complete"], false);

    let (_, small) = since(&admin, general, "after=0&limit=3").await;
    assert_eq!(small["messages"].as_array().unwrap().len(), 3);
    assert_eq!(small["complete"], false);

    // Catching up from the last one returned leaves the rest.
    let last_returned = capped["messages"].as_array().unwrap().last().unwrap()["id"]
        .as_i64()
        .unwrap();
    let (_, rest) = since(&admin, general, &format!("after={last_returned}")).await;
    assert_eq!(rest["messages"].as_array().unwrap().len(), 1);
    assert_eq!(rest["complete"], true);
}

#[tokio::test]
async fn people_who_cannot_read_a_channel_get_nothing_from_it() {
    let server = start().await;
    let admin = admin(&server).await;
    let grace = invite(&server, &admin, "Grace", "grace").await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let bea_id = bea.user_id().await;

    let private = admin
        .post("/channels", &[("name", "secret-plans"), ("private", "on")])
        .await;
    let private_id: i64 = location(&private)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.send(private_id, "Only for members", None).await;
    let message = last_message_id(&admin.page(&format!("/c/{private_id}")).await);

    let dm_id: i64 = location(&admin.get(&format!("/dm/{bea_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.send(dm_id, "Just between us", None).await;

    for channel in [private_id, dm_id] {
        let (status, _) = since(&grace, channel, "after=0").await;
        assert_eq!(status, StatusCode::NOT_FOUND, "since in {channel}");
        let refresh = grace
            .post_json(
                &format!("/c/{channel}/messages/refresh"),
                &json!({ "ids": [message] }),
            )
            .await;
        assert_eq!(
            refresh.status(),
            StatusCode::NOT_FOUND,
            "refresh in {channel}"
        );
        let sidebar = grace.get(&format!("/sidebar?channel={channel}")).await;
        assert_eq!(sidebar.status(), StatusCode::OK);
        let listed = sidebar.text().await.unwrap();
        assert!(
            !listed.contains(&format!("data-channel-link=\"{channel}\"")),
            "sidebar lists {channel}"
        );
    }

    // Members still get them.
    let (status, members) = since(&admin, private_id, "after=0").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(members["messages"].as_array().unwrap().len(), 1);
    let (status, _) = since(&bea, dm_id, "after=0").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn refresh_shows_edits_and_reports_deleted_messages() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    admin.send(general, "original words", None).await;
    let edited = last_message_id(&admin.page(&format!("/c/{general}")).await);
    admin.send(general, "gone soon", None).await;
    let deleted = last_message_id(&admin.page(&format!("/c/{general}")).await);

    let edit = admin
        .post(
            &format!("/c/{general}/m/{edited}/edit"),
            &[("body", "edited words")],
        )
        .await;
    assert!(
        edit.status().is_success() || edit.status().is_redirection(),
        "{}",
        edit.status()
    );
    let delete = admin
        .post(&format!("/c/{general}/m/{deleted}/delete"), &[])
        .await;
    assert!(
        delete.status().is_success() || delete.status().is_redirection(),
        "{}",
        delete.status()
    );

    let refreshed: Value = admin
        .post_json(
            &format!("/c/{general}/messages/refresh"),
            &json!({ "ids": [edited, deleted] }),
        )
        .await
        .json()
        .await
        .unwrap();
    let messages = refreshed["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], edited);
    assert!(
        messages[0]["html"]
            .as_str()
            .unwrap()
            .contains("edited words")
    );
    assert_eq!(refreshed["gone"], json!([deleted]));
}

#[tokio::test]
async fn a_send_id_used_in_another_channel_is_refused() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let other = location(&admin.post("/channels", &[("name", "elsewhere")]).await);
    let other_id: i64 = other.trim_start_matches("/c/").parse().unwrap();

    assert_eq!(
        send_with_nonce(&admin, general, "First place", NONCE, None, true)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let reused = send_with_nonce(&admin, other_id, "Second place", NONCE, None, true).await;
    assert_eq!(reused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        admin
            .page(&format!("/c/{other_id}"))
            .await
            .matches("Second place")
            .count(),
        0
    );
}
