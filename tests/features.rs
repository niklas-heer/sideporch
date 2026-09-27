// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Search, file uploads, custom emoji, reactions, push notifications and
//! Lua automations, driven through a real server.

mod common;

use std::time::Duration;

use axum::{Router, body::Bytes, http::HeaderMap, routing::post};
use base64ct::{Base64UrlUnpadded, Encoding as _};
use common::{
    Browser, PNG, admin, between, home_channel, invite, last_message_id, location, start,
};
use reqwest::StatusCode;
use serde_json::json;
use tokio::sync::mpsc;
use web_push_native::{
    Auth,
    p256::{SecretKey, elliptic_curve::sec1::ToEncodedPoint},
};

fn file_ids(page: &str) -> Vec<i64> {
    page.match_indices(r#"href="/files/"#)
        .map(|(index, matched)| {
            page[index + matched.len()..]
                .split('"')
                .next()
                .unwrap()
                .parse()
                .unwrap()
        })
        .collect()
}

#[tokio::test]
async fn search_finds_messages_people_may_read() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let general = home_channel(&admin).await;

    admin
        .send(general, "The tomato stakes did not survive", None)
        .await;
    let bea_id = bea.user_id().await;
    let dm: i64 = location(&admin.get(&format!("/dm/{bea_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.send(dm, "Secret tomato soup recipe", None).await;

    let for_cat = cat.page("/search?q=tomato").await;
    assert!(
        for_cat.contains("<mark class=\"rounded bg-lamp px-0.5 text-ink\">tomato</mark> stakes")
    );
    assert!(!for_cat.contains("soup"), "direct messages stay private");

    let for_bea = bea.page("/search?q=tomato").await;
    assert!(for_bea.contains("stakes") && for_bea.contains("soup"));
    assert!(for_bea.contains("Conversation with Ada Admin"));

    // Words match as prefixes, and search operators are just words.
    assert!(cat.page("/search?q=stak").await.contains("stakes"));
    let odd = cat.page("/search?q=%22NOT%20OR%20(%20*").await;
    assert!(odd.contains("Nothing matches"));
}

#[tokio::test]
async fn uploads_are_stored_shown_and_private() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let general = home_channel(&admin).await;

    let response = bea
        .upload(
            general,
            "Garden plan",
            &[
                ("plot.png", PNG),
                ("notes.txt", b"beans, then tomatoes"),
                ("sneaky.png", b"<svg onload=alert(1)>"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let page = admin.page(&format!("/c/{general}")).await;
    let ids = file_ids(&page);
    assert_eq!(ids.len(), 3);
    assert!(
        page.contains(&format!(r#"<img src="/files/{}""#, ids[0])),
        "images show inline"
    );
    assert!(page.contains(r#"alt="plot.png""#));
    assert!(page.contains("notes.txt") && page.contains("20 bytes"));

    let image = cat.get(&format!("/files/{}", ids[0])).await;
    assert_eq!(image.headers()["content-type"], "image/png");
    assert!(
        image.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("inline")
    );
    assert_eq!(image.bytes().await.unwrap().as_ref(), PNG);

    // Anything that isn't a known image type downloads instead of rendering.
    let sneaky = cat.get(&format!("/files/{}", ids[2])).await;
    assert_eq!(sneaky.headers()["content-type"], "application/octet-stream");
    assert!(
        sneaky.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .starts_with("attachment")
    );

    // Files in direct messages are visible to the members only.
    let bea_id = bea.user_id().await;
    let dm: i64 = location(&admin.get(&format!("/dm/{bea_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.upload(dm, "", &[("private.txt", b"for bea")]).await;
    let private = file_ids(&bea.page(&format!("/c/{dm}")).await)[0];
    assert_eq!(
        bea.get(&format!("/files/{private}")).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        cat.get(&format!("/files/{private}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        location(
            &Browser::anonymous(&server)
                .get(&format!("/files/{private}"))
                .await
        ),
        "/login"
    );

    // File names are searchable.
    assert!(
        bea.page("/search?q=notes.txt")
            .await
            .contains("Garden plan")
    );
}

#[tokio::test]
async fn custom_emoji_and_reactions() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let general = home_channel(&admin).await;

    let add = |name: &'static str, data: &'static [u8]| {
        let form = reqwest::multipart::Form::new().text("name", name).part(
            "image",
            reqwest::multipart::Part::bytes(data).file_name("emoji.png"),
        );
        bea.client
            .post(bea.url("/emoji"))
            .header("cookie", &bea.cookie)
            .multipart(form)
            .send()
    };
    assert_eq!(
        add("porch", PNG).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        add("porch", PNG).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        add("+1", PNG).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        add("notimage", b"hello").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );

    admin
        .send(general, "Evening on the porch :porch:", None)
        .await;
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains(r#"alt=":porch:""#));
    let message = last_message_id(&page);

    let mut admin_live = admin.live().await;
    let reactions = format!("/c/{general}/m/{message}/reactions");
    let response = bea.post(&reactions, &[("emoji", "+1")]).await;
    assert_eq!(location(&response), format!("/c/{general}#m{message}"));
    let event = admin_live.next_event(Duration::from_secs(5)).await.unwrap();
    assert_eq!(event["type"], "reactions");
    assert_eq!(event["message_id"], message);
    let bea_id = bea.user_id().await;
    assert!(
        event["html"]
            .as_str()
            .unwrap()
            .contains(&format!(r#"data-users="{bea_id}""#))
    );

    bea.post(&reactions, &[("emoji", ":porch:")]).await;
    cat.post(&reactions, &[("emoji", "+1")]).await;
    let for_bea = bea.page(&format!("/c/{general}")).await;
    let bar = between(&for_bea, &format!(r#"id="reactions-{message}""#), "</div>");
    assert!(bar.contains(r#"value="+1""#) && bar.contains(r#"value="porch""#));
    assert!(bar.contains("Bea, Cat reacted with :+1:"));
    assert_eq!(
        bar.matches(r#"aria-pressed="true""#).count(),
        2,
        "both of Bea's own reactions"
    );

    // Reacting again takes the reaction back.
    bea.post(&reactions, &[("emoji", "porch")]).await;
    let bar_page = bea.page(&format!("/c/{general}")).await;
    assert!(
        !between(&bar_page, &format!(r#"id="reactions-{message}""#), "</div>").contains("porch")
    );

    assert_eq!(
        bea.post(&reactions, &[("emoji", "not-an-emoji")])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );

    // The person who added an emoji or an admin can remove it; others can't.
    assert_eq!(
        cat.post("/emoji/porch/delete", &[]).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        admin.post("/emoji/porch/delete", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    assert!(!admin.page("/emoji").await.contains(":porch:"));
}

/// A stand-in push service that records what Sideporch sends it.
async fn push_service() -> (String, mpsc::UnboundedReceiver<(HeaderMap, Bytes)>) {
    let (sender, receiver) = mpsc::unbounded_channel();
    let app = Router::new().route(
        "/push/{id}",
        post(move |headers: HeaderMap, body: Bytes| {
            let sender = sender.clone();
            async move {
                sender.send((headers, body)).unwrap();
                StatusCode::CREATED
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, receiver)
}

#[tokio::test]
async fn push_notifications_reach_people_who_are_away() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let general = home_channel(&admin).await;
    let (service, mut pushes) = push_service().await;

    // Bea's browser subscribes with its own key pair and auth secret.
    let secret = SecretKey::from_slice(&[7; 32]).unwrap();
    let public = secret.public_key().to_encoded_point(false);
    let auth = [9_u8; 16];
    let subscription = json!({
        "endpoint": format!("{service}/push/bea"),
        "keys": {
            "p256dh": Base64UrlUnpadded::encode_string(public.as_bytes()),
            "auth": Base64UrlUnpadded::encode_string(&auth),
        }
    });
    assert_eq!(
        bea.post_json("/push/subscriptions", &subscription)
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let broken = json!({ "endpoint": format!("{service}/push/x"), "keys": { "p256dh": "AAAA", "auth": "AAAA" } });
    assert_eq!(
        bea.post_json("/push/subscriptions", &broken).await.status(),
        StatusCode::BAD_REQUEST
    );

    let decrypt = |body: &Bytes| -> serde_json::Value {
        let plain =
            web_push_native::decrypt(body.to_vec(), &secret, &Auth::clone_from_slice(&auth))
                .unwrap();
        serde_json::from_slice(&plain).unwrap()
    };

    let bea_id = bea.user_id().await;
    let dm: i64 = location(&admin.get(&format!("/dm/{bea_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.send(dm, "psst, cake in the kitchen", None).await;
    let (headers, body) = tokio::time::timeout(Duration::from_secs(5), pushes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(headers["content-encoding"], "aes128gcm");
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("vapid t=")
    );
    assert!(headers.contains_key("ttl"));
    let payload = decrypt(&body);
    assert_eq!(payload["title"], "Ada Admin");
    assert_eq!(payload["body"], "psst, cake in the kitchen");
    assert_eq!(payload["url"], format!("/c/{dm}"));

    // Mentions notify too; ordinary channel messages don't.
    admin.send(general, "Nothing to see here", None).await;
    admin
        .send(general, "@bea can you water the plants?", None)
        .await;
    let (_, body) = tokio::time::timeout(Duration::from_secs(5), pushes.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(decrypt(&body)["body"], "@bea can you water the plants?");

    // Someone looking at Sideporch right now gets no push.
    let mut live = bea.live().await;
    live.send(&json!({ "type": "visibility", "visible": true }))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    admin.send(dm, "are you there?", None).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), pushes.recv())
            .await
            .is_err()
    );
}

async fn save_automation(admin: &Browser, name: &str, source: &str) -> String {
    let response = admin
        .post(
            "/automations",
            &[("name", name), ("source", source), ("enabled", "on")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    location(&response)
}

#[tokio::test]
async fn automations_answer_messages_and_keep_data() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let general = home_channel(&admin).await;

    assert_eq!(
        bea.get("/automations").await.status(),
        StatusCode::FORBIDDEN
    );
    let example = admin.page("/automations/new").await;
    assert!(example.contains("sideporch.on_message"));

    let source = r#"
        sideporch.on_message(function(msg)
          if msg.text == "!ping" then
            sideporch.reply(msg, "pong for " .. msg.username .. " in #" .. msg.channel)
          elseif msg.text == "!count" then
            local count = tonumber(sideporch.get("count") or "0") + 1
            sideporch.set("count", count)
            sideporch.post("general", "count is " .. count)
          end
        end)
        -- Replies to everything; must not answer its own posts forever.
        sideporch.on_message(function(msg)
          if msg.text == "!echo" then sideporch.post("general", "echo") end
        end)
    "#;
    save_automation(&admin, "Porch butler", source).await;

    bea.send(general, "!ping", None).await;
    let ping = last_message_id(&admin.page(&format!("/c/{general}")).await);
    let thread = admin
        .wait_for(
            &format!("/c/{general}/t/{ping}"),
            "pong for bea in #general",
        )
        .await;
    assert!(thread.contains(r#"<span class="font-bold">Porch butler</span>"#));

    bea.send(general, "!count", None).await;
    admin.wait_for(&format!("/c/{general}"), "count is 1").await;
    bea.send(general, "!count", None).await;
    admin.wait_for(&format!("/c/{general}"), "count is 2").await;

    bea.send(general, "!echo", None).await;
    admin.wait_for(&format!("/c/{general}"), ">echo<").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .matches(">echo<")
            .count(),
        1
    );
}

#[tokio::test]
async fn automations_are_sandboxed_and_bounded() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;

    let escape = save_automation(&admin, "Escape", r#"os.execute("touch /tmp/pwned")"#).await;
    let page = admin.page(&escape).await;
    assert!(page.contains("The script stopped with an error"));
    assert!(page.contains("global &#39;os&#39;") || page.contains("global 'os'"));

    let fast = save_automation(&admin, "Too fast", "sideporch.every(1, function() end)").await;
    assert!(admin.page(&fast).await.contains("use at least 10 seconds"));

    let spin = save_automation(
        &admin,
        "Spinner",
        r#"sideporch.on_message(function(msg) if msg.text == "!spin" then while true do end end end)"#,
    )
    .await;
    save_automation(
        &admin,
        "Pinger",
        r#"sideporch.on_message(function(msg) if msg.text == "!ping" then sideporch.reply(msg, "pong") end end)"#,
    )
    .await;
    admin.send(general, "!spin", None).await;
    admin.wait_for(&spin, "the script ran too long").await;

    // Other automations and the server carry on.
    admin.send(general, "!ping", None).await;
    let ping = last_message_id(&admin.page(&format!("/c/{general}")).await);
    admin
        .wait_for(&format!("/c/{general}/t/{ping}"), ">pong<")
        .await;

    // Turning an automation off stops it; deleting removes it.
    let id = spin.trim_start_matches("/automations/");
    let off = admin
        .post(&spin, &[("name", "Spinner"), ("source", "-- off")])
        .await;
    assert_eq!(off.status(), StatusCode::SEE_OTHER);
    assert!(admin.page("/automations").await.contains(">Off<"));
    assert_eq!(
        admin
            .post(&format!("/automations/{id}/delete"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(!admin.page("/automations").await.contains("Spinner"));
}
