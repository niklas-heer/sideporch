// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Everyday chat features, driven through a real server: changing and
//! pinning messages, accounts, private channels, activity, reminders,
//! previews, backups, imports, polls and outgoing webhooks.

mod common;

use std::time::Duration;

use common::{Browser, admin, between, home_channel, invite, last_message_id, start};
use reqwest::StatusCode;
use serde_json::{Value, json};

/// Posts a form the way app.js does, in the background.
async fn fetch_post(browser: &Browser, path: &str, form: &[(&str, &str)]) -> reqwest::Response {
    browser
        .client
        .post(browser.url(path))
        .header("cookie", &browser.cookie)
        .header("x-sideporch-fetch", "1")
        .form(form)
        .send()
        .await
        .unwrap()
}

/// The next live event of `kind`, skipping others.
async fn next_of(live: &mut common::Live, kind: &str) -> Value {
    loop {
        let event = live
            .next_event(Duration::from_secs(5))
            .await
            .unwrap_or_else(|| panic!("no {kind} event"));
        if event["type"] == kind {
            return event;
        }
    }
}

#[tokio::test]
async fn messages_can_be_edited_deleted_pinned_and_saved() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let mut live = member.live().await;

    member.send(general, "Helo porch", None).await;
    let page = member.page(&format!("/c/{general}")).await;
    let id = last_message_id(&page);
    let base = format!("/c/{general}/m/{id}");
    next_of(&mut live, "message").await;

    // The author fixes a typo; everyone sees it live, marked as edited.
    let source: Value = member
        .get(&format!("{base}/source"))
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(source["body"], "Helo porch");
    let edited = fetch_post(
        &member,
        &format!("{base}/edit"),
        &[("body", "Hello **porch**")],
    )
    .await;
    assert_eq!(edited.status(), StatusCode::NO_CONTENT);
    let changed = next_of(&mut live, "message_changed").await;
    assert!(
        changed["html"]
            .as_str()
            .unwrap()
            .contains("<strong>porch</strong>")
    );
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains("(edited)") && page.contains("<strong>porch</strong>"));
    // Search finds the new words, not the old ones.
    assert!(
        admin
            .page("/search?q=hello")
            .await
            .contains(&format!("#m{id}"))
    );
    assert!(
        !admin
            .page("/search?q=helo")
            .await
            .contains(&format!("#m{id}"))
    );
    // Only the author edits.
    assert_eq!(
        fetch_post(&admin, &format!("{base}/edit"), &[("body", "hijacked")])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );

    // Anyone in the channel pins; the channel lists pins.
    let pinned: Value = fetch_post(&admin, &format!("{base}/pin"), &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(pinned["pinned"], true);
    assert!(
        next_of(&mut live, "message_changed").await["html"]
            .as_str()
            .unwrap()
            .contains("Pinned by Ada Admin")
    );
    let pins = member.page(&format!("/c/{general}/pins")).await;
    assert!(pins.contains("<strong>porch</strong>"));
    assert!(
        member
            .page(&format!("/c/{general}"))
            .await
            .contains("aria-label=\"Pinned messages: 1\"")
    );

    // Saved messages are personal.
    let saved: Value = fetch_post(&member, &format!("{base}/save"), &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(saved["saved"], true);
    assert!(
        member
            .page("/saved")
            .await
            .contains("<strong>porch</strong>")
    );
    assert!(
        !admin
            .page("/saved")
            .await
            .contains("<strong>porch</strong>")
    );
    assert!(
        member
            .page(&format!("/c/{general}"))
            .await
            .contains("data-saved")
    );
}

#[tokio::test]
async fn deleting_keeps_threads_and_works_without_javascript() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    member.send(general, "Start of a thread", None).await;
    let id = last_message_id(&member.page(&format!("/c/{general}")).await);
    let base = format!("/c/{general}/m/{id}");
    fetch_post(&member, &format!("{base}/save"), &[]).await;
    let mut live = member.live().await;

    // A thread's first message stays as a placeholder while replies exist.
    member.send(general, "A reply", Some(id)).await;
    next_of(&mut live, "message").await;
    let other = invite(&server, &admin, "Ola Other", "ola").await;
    assert_eq!(
        fetch_post(&other, &format!("{base}/delete"), &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fetch_post(&member, &format!("{base}/delete"), &[])
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        next_of(&mut live, "message_changed").await["html"]
            .as_str()
            .unwrap()
            .contains("This message was deleted.")
    );
    let thread = member.page(&format!("/c/{general}/t/{id}")).await;
    assert!(thread.contains("This message was deleted.") && thread.contains("A reply"));
    assert!(!member.page("/saved").await.contains("Start of a thread"));

    // Deleting the last reply removes the placeholder too.
    let reply = last_message_id(&format!(
        r#"id="messages"{}</ol>"#,
        between(&thread, r#"id="replies""#, "</ol>")
    ));
    assert_eq!(
        fetch_post(&admin, &format!("/c/{general}/m/{reply}/delete"), &[])
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    let deleted = next_of(&mut live, "message_deleted").await;
    assert_eq!(deleted["id"], reply);
    assert_eq!(deleted["reply_count"], 0);
    assert_eq!(
        member.get(&format!("{base}/source")).await.status(),
        StatusCode::NOT_FOUND
    );

    // Without JavaScript, the actions page offers the same.
    member.send(general, "Plain", None).await;
    let plain = last_message_id(&member.page(&format!("/c/{general}")).await);
    let actions = member
        .page(&format!("/c/{general}/m/{plain}/actions"))
        .await;
    assert!(
        actions.contains("Save changes")
            && actions.contains("Pin to channel")
            && actions.contains("Delete")
    );
    let admin_actions = admin.page(&format!("/c/{general}/m/{plain}/actions")).await;
    assert!(!admin_actions.contains("Save changes") && admin_actions.contains("Delete"));
    let redirect = member
        .post(
            &format!("/c/{general}/m/{plain}/edit"),
            &[("body", "Plain, edited")],
        )
        .await;
    assert_eq!(redirect.status(), StatusCode::SEE_OTHER);
    let permalink = member.get(&format!("/c/{general}/m/{plain}")).await;
    assert!(common::location(&permalink).ends_with(&format!("#m{plain}")));
}

/// Signs in with a username and password, returning the browser.
async fn sign_in(server: &common::Server, username: &str, password: &str) -> (Browser, StatusCode) {
    let mut browser = Browser::anonymous(server);
    let status = browser
        .submit("/login", &[("username", username), ("password", password)])
        .await
        .status();
    (browser, status)
}

#[tokio::test]
async fn accounts_change_passwords_reset_and_deactivate() {
    let server = start().await;
    let admin = admin(&server).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let mo = member.user_id().await;

    // Changing the password needs the current one and signs out elsewhere.
    let (other_device, _) = sign_in(&server, "mo", "a long password").await;
    let wrong = member
        .post(
            "/settings/account",
            &[("current", "nope"), ("password", "brand new pass")],
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);
    let changed = member
        .post(
            "/settings/account",
            &[
                ("current", "a long password"),
                ("password", "brand new pass"),
            ],
        )
        .await;
    assert_eq!(changed.status(), StatusCode::OK);
    assert_eq!(member.get("/home").await.status(), StatusCode::OK);
    assert_eq!(
        other_device.get("/home").await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        sign_in(&server, "mo", "brand new pass").await.1,
        StatusCode::SEE_OTHER
    );

    // Only admins hand out reset links, and not for themselves.
    assert_eq!(
        member
            .post(&format!("/people/{mo}/reset-link"), &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    let me = admin.user_id().await;
    assert_eq!(
        admin
            .post(&format!("/people/{me}/deactivate"), &[])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let page = admin
        .post(&format!("/people/{mo}/reset-link"), &[])
        .await
        .text()
        .await
        .unwrap();
    let token = between(&page, "/reset/", "<").to_owned();
    let mut forgetful = Browser::anonymous(&server);
    assert!(
        forgetful
            .page(&format!("/reset/{token}"))
            .await
            .contains("Mo Member")
    );
    let reset = forgetful
        .submit(
            &format!("/reset/{token}"),
            &[("password", "remembered one")],
        )
        .await;
    assert_eq!(reset.status(), StatusCode::SEE_OTHER);
    assert_eq!(forgetful.get("/home").await.status(), StatusCode::OK);
    assert_eq!(member.get("/home").await.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        Browser::anonymous(&server)
            .get(&format!("/reset/{token}"))
            .await
            .status(),
        StatusCode::GONE
    );
}

#[tokio::test]
async fn admins_grant_rights_and_deactivate_accounts() {
    let server = start().await;
    let admin = admin(&server).await;
    let (forgetful, _) = {
        invite(&server, &admin, "Mo Member", "mo").await;
        sign_in(&server, "mo", "a long password").await
    };
    let mo = forgetful.user_id().await;
    // Admin rights and deactivation.
    admin
        .post(&format!("/people/{mo}/admin"), &[("admin", "true")])
        .await;
    assert!(forgetful.page("/admin/system").await.contains("Storage"));
    admin
        .post(&format!("/people/{mo}/admin"), &[("admin", "false")])
        .await;
    assert_eq!(
        forgetful.get("/admin/system").await.status(),
        StatusCode::FORBIDDEN
    );
    admin.post(&format!("/people/{mo}/deactivate"), &[]).await;
    assert_eq!(forgetful.get("/home").await.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        sign_in(&server, "mo", "a long password").await.1,
        StatusCode::UNAUTHORIZED
    );
    let people = admin.page("/people").await;
    assert!(between(&people, "Deactivated", "</section>").contains("Mo Member"));
    assert!(
        !invite(&server, &admin, "Ola Other", "ola")
            .await
            .page("/people")
            .await
            .contains("Deactivated")
    );
    admin.post(&format!("/people/{mo}/reactivate"), &[]).await;
    assert_eq!(
        sign_in(&server, "mo", "a long password").await.1,
        StatusCode::SEE_OTHER
    );
}

/// Creates a channel and returns its id.
async fn create_channel(browser: &Browser, name: &str, private: bool) -> i64 {
    let mut form = vec![("name", name)];
    if private {
        form.push(("private", "on"));
    }
    let created = browser.post("/channels", &form).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    common::location(&created)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

#[tokio::test]
async fn private_channels_are_for_members_only() {
    let server = start().await;
    let admin = admin(&server).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let outsider = invite(&server, &admin, "Ola Other", "ola").await;
    let mo = member.user_id().await;
    let secret = create_channel(&admin, "garden-plans", true).await;
    assert!(
        admin
            .page(&format!("/c/{secret}"))
            .await
            .contains("private channel")
    );
    assert_eq!(
        member.get(&format!("/c/{secret}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert!(
        !member
            .page("/channels/browse")
            .await
            .contains("garden-plans")
    );

    // Members add people, who then see it live and in search.
    admin
        .post(
            &format!("/c/{secret}/members"),
            &[("user_id", &mo.to_string())],
        )
        .await;
    assert!(member.page("/home").await.contains("garden-plans"));
    let mut member_live = member.live().await;
    let mut outsider_live = outsider.live().await;
    admin
        .send(secret, "Tomatoes go left @ola @channel", None)
        .await;
    next_of(&mut member_live, "message").await;
    assert!(
        outsider_live
            .next_event(Duration::from_millis(500))
            .await
            .is_none()
    );
    assert!(member.page("/search?q=tomatoes").await.contains("Tomatoes"));
    assert!(
        !outsider
            .page("/search?q=tomatoes")
            .await
            .contains("Tomatoes")
    );
    let message = last_message_id(&admin.page(&format!("/c/{secret}")).await);
    assert_eq!(
        outsider
            .get(&format!("/c/{secret}/m/{message}"))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    // Members leave; the last one can't.
    member.post(&format!("/c/{secret}/leave"), &[]).await;
    assert_eq!(
        member.get(&format!("/c/{secret}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        admin
            .post(&format!("/c/{secret}/leave"), &[])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn public_channels_can_be_left_rejoined_and_muted() {
    let server = start().await;
    let admin = admin(&server).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let noisy = create_channel(&admin, "alerts", false).await;
    assert!(member.page("/home").await.contains(">alerts<"));

    member.post(&format!("/c/{noisy}/leave"), &[]).await;
    assert!(!member.page("/home").await.contains(">alerts<"));
    let directory = member.page("/channels/browse").await;
    assert!(between(&directory, ">alerts<", "</li>").contains("Join"));
    assert!(
        member
            .page(&format!("/c/{noisy}"))
            .await
            .contains("You left #alerts")
    );
    member.post(&format!("/c/{noisy}/join"), &[]).await;
    assert!(member.page("/home").await.contains(">alerts<"));

    // Muted channels never look unread.
    let muted: Value = fetch_post(&member, &format!("/c/{noisy}/mute"), &[])
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(muted["muted"], true);
    admin.send(noisy, "Disk almost full", None).await;
    let home = member.page("/home").await;
    let link = between(&home, &format!("data-channel-link=\"{noisy}\""), ">");
    assert!(link.contains("data-muted") && !link.contains("data-unread"));
    fetch_post(&member, &format!("/c/{noisy}/mute"), &[]).await;
    let home = member.page("/home").await;
    assert!(between(&home, &format!("data-channel-link=\"{noisy}\""), ">").contains("data-unread"));
}

#[tokio::test]
async fn activity_collects_mentions_and_replies() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    assert!(member.page("/activity").await.contains("shows up here"));

    member.send(general, "Anyone seen the ladder?", None).await;
    let question = last_message_id(&member.page(&format!("/c/{general}")).await);
    let mut live = member.live().await;
    admin.send(general, "In the shed", Some(question)).await;
    let event = next_of(&mut live, "message").await;
    assert_eq!(event["activity"], json!([member.user_id().await]));
    admin.send(general, "Thanks @mo!", None).await;

    let home = member.page("/home").await;
    assert!(between(&home, "href=\"/activity\"", ">").contains("data-unread"));
    let activity = member.page("/activity").await;
    assert!(activity.contains("Mentioned you") && activity.contains("Thanks"));
    assert!(activity.contains("Replied in a thread") && activity.contains("In the shed"));
    assert!(activity.contains("data-new"));
    // Seen now.
    assert!(!member.page("/activity").await.contains("data-new"));
    assert!(
        !between(&member.page("/home").await, "href=\"/activity\"", ">").contains("data-unread")
    );
    // Your own messages never land there.
    assert!(!admin.page("/activity").await.contains("Thanks"));
}

#[tokio::test]
async fn typing_shows_to_readers() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let mut writer = admin.live().await;
    let mut reader = member.live().await;
    writer
        .send(&json!({ "type": "typing", "channel_id": general, "parent_id": null }))
        .await;
    let typing = next_of(&mut reader, "typing").await;
    assert_eq!(typing["name"], "Ada Admin");
    assert_eq!(typing["channel_id"], general);
    // Nobody hears about channels they can't read.
    let secret = create_channel(&admin, "secret", true).await;
    writer
        .send(&json!({ "type": "typing", "channel_id": secret, "parent_id": null }))
        .await;
    tokio::time::sleep(Duration::from_millis(2100)).await;
    writer
        .send(&json!({ "type": "typing", "channel_id": secret, "parent_id": null }))
        .await;
    assert!(
        reader
            .next_event(Duration::from_millis(500))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn reminders_and_scheduled_messages_arrive_later() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mut live = admin.live().await;
    live.send(&json!({ "type": "timezone", "name": "Europe/Berlin" }))
        .await;

    // /remind answers privately and lists the reminder.
    let answer: Value = admin
        .type_message(general, "/remind me in 5 minutes to stretch")
        .await
        .json()
        .await
        .unwrap();
    let notice = answer["ephemeral"][0].as_str().unwrap();
    assert!(
        notice.contains("remind you on") && notice.contains("stretch"),
        "{notice}"
    );
    let unclear: Value = admin
        .type_message(general, "/remind me to do it someday")
        .await
        .json()
        .await
        .unwrap();
    assert!(
        unclear["ephemeral"][0]
            .as_str()
            .unwrap()
            .contains("tell when"),
        "{unclear}"
    );
    let scheduled = admin.page("/scheduled").await;
    assert!(scheduled.contains("stretch"));

    // A message scheduled for later waits, and Send now delivers it.
    let queued: Value = fetch_post(
        &admin,
        &format!("/c/{general}/messages"),
        &[
            ("body", "Good morning, porch"),
            ("send_at", "tomorrow at 8:00"),
        ],
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(
        queued["ephemeral"][0]
            .as_str()
            .unwrap()
            .contains("Scheduled for")
    );
    assert!(
        !admin
            .page(&format!("/c/{general}"))
            .await
            .contains("Good morning, porch")
    );
    let scheduled = admin.page("/scheduled").await;
    assert!(scheduled.contains("Good morning, porch") && scheduled.contains("08:00"));
    let id = between(&scheduled, "/scheduled/", "/send");
    admin.post(&format!("/scheduled/{id}/send"), &[]).await;
    assert!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .contains("Good morning, porch")
    );
    assert_eq!(
        fetch_post(
            &admin,
            &format!("/c/{general}/messages"),
            &[("body", "Too late"), ("send_at", "2020-01-01 09:00")],
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );

    // Reminding about a message, then cancelling one.
    let message = last_message_id(&admin.page(&format!("/c/{general}")).await);
    let answer: Value = fetch_post(
        &admin,
        &format!("/c/{general}/m/{message}/remind"),
        &[("when", "in 1 hour")],
    )
    .await
    .json()
    .await
    .unwrap();
    assert!(answer["at"].as_str().is_some());
    let scheduled = admin.page("/scheduled").await;
    assert!(scheduled.contains("See the message"));
    let reminder = between(&scheduled, "/reminders/", "/cancel");
    admin
        .post(&format!("/reminders/{reminder}/cancel"), &[])
        .await;
    // The soonest one, to stretch, is first.
    let scheduled = admin.page("/scheduled").await;
    assert!(!scheduled.contains("stretch") && scheduled.contains("See the message"));
}

/// A local web page with Open Graph tags, behind a redirect.
async fn fake_site() -> String {
    use axum::{
        Router,
        response::{Html, Redirect},
        routing::get,
    };
    let app = Router::new()
        .route("/old", get(|| async { Redirect::permanent("/guide") }))
        .route(
            "/guide",
            get(|| async {
                Html(
                    r#"<html><head><title>Fallback</title>
                    <meta property="og:title" content="Porch Building Guide">
                    <meta property="og:description" content="Everything about porches.">
                    <meta property="og:image" content="https://images.example/porch.png">
                    <meta property="og:site_name" content="Porch Weekly"></head></html>"#,
                )
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[tokio::test]
async fn links_get_previews_unless_turned_off() {
    let site = fake_site().await;
    let server = common::start_with(|config| config.allow_private_link_previews = true).await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mut live = admin.live().await;
    admin
        .send(
            general,
            &format!("Read this: {site}/old, then `{site}/code`"),
            None,
        )
        .await;
    next_of(&mut live, "message").await;
    let changed = next_of(&mut live, "message_changed").await;
    let html = changed["html"].as_str().unwrap();
    assert!(
        html.contains("Porch Building Guide") && html.contains("Porch Weekly"),
        "{html}"
    );
    assert!(
        html.contains("https://images.example/porch.png") && html.contains(&format!("{site}/old"))
    );

    // Authors remove previews.
    let id = changed["id"].as_i64().unwrap();
    fetch_post(&admin, &format!("/c/{general}/m/{id}/preview/remove"), &[]).await;
    assert!(
        !admin
            .page(&format!("/c/{general}"))
            .await
            .contains("Porch Building Guide")
    );

    // Admins turn them off.
    admin.post("/admin/previews", &[]).await;
    assert!(!admin.page("/admin/previews").await.contains("checked"));
    admin
        .send(general, &format!("Again: {site}/guide"), None)
        .await;
    next_of(&mut live, "message").await;
    assert!(live.next_event(Duration::from_millis(800)).await.is_none());
}

#[tokio::test]
async fn previews_never_reach_private_addresses() {
    let site = fake_site().await;
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mut live = admin.live().await;
    admin
        .send(general, &format!("Internal: {site}/guide"), None)
        .await;
    next_of(&mut live, "message").await;
    assert!(live.next_event(Duration::from_millis(800)).await.is_none());
    assert!(
        !admin
            .page(&format!("/c/{general}"))
            .await
            .contains("Porch Building Guide")
    );
}

#[tokio::test]
async fn backups_download_schedule_and_restore() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    admin.send(general, "Remember the porch paint", None).await;
    admin
        .upload(general, "", &[("colors.txt", b"sage green")])
        .await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    assert_eq!(
        member.get("/admin/backups/download").await.status(),
        StatusCode::FORBIDDEN
    );

    let download = admin.get("/admin/backups/download?key=on").await;
    assert_eq!(download.status(), StatusCode::OK);
    assert!(
        download.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("sideporch-")
    );
    let archive = download.bytes().await.unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let file = scratch.path().join("backup.tar.gz");
    std::fs::write(&file, &archive).unwrap();
    // Downloads leave nothing behind in the data directory.
    assert!(
        std::fs::read_dir(server.data_dir())
            .unwrap()
            .flatten()
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".download"))
    );

    // Restoring refuses a directory with a database, then fills an empty one.
    assert!(sideporch::restore(&file, server.data_dir(), false).is_err());
    let restored = scratch.path().join("data");
    assert!(sideporch::restore(&file, &restored, false).unwrap() >= 3);
    assert!(restored.join("secret.key").exists());
    let copy = common::start_with(|config| config.data_dir = restored.clone()).await;
    let mut ada = Browser::anonymous(&copy);
    ada.submit(
        "/login",
        &[("username", "ada"), ("password", "correct horse")],
    )
    .await;
    let page = ada.page(&format!("/c/{general}")).await;
    assert!(page.contains("Remember the porch paint"));
    let file_id = between(&page, "/files/", "\"").to_owned();
    assert_eq!(
        ada.get(&format!("/files/{file_id}"))
            .await
            .bytes()
            .await
            .unwrap()
            .as_ref(),
        b"sage green"
    );

    // Scheduled backups keep the newest few.
    let saved = admin
        .post(
            "/admin/backups",
            &[("every_hours", "24"), ("keep", "1"), ("dir", "backups")],
        )
        .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    admin.post("/admin/backups/run", &[]).await;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    admin.post("/admin/backups/run", &[]).await;
    let stored: Vec<_> = std::fs::read_dir(server.data_dir().join("backups"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(stored.len(), 1);
    let page = admin.page("/admin/backups").await;
    let name = between(&page, "/admin/backups/files/", "\"").to_owned();
    let stored = admin.get(&format!("/admin/backups/files/{name}")).await;
    assert_eq!(stored.status(), StatusCode::OK);
    assert_eq!(
        admin
            .get("/admin/backups/files/..%2Fsideporch.db")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

/// A small Slack export.
fn slack_export() -> Vec<u8> {
    use std::io::Write as _;
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let mut add = |name: &str, value: &Value| {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(value.to_string().as_bytes()).unwrap();
    };
    add(
        "users.json",
        &json!([
            { "id": "U1", "name": "ada", "profile": { "display_name": "Ada" } },
            { "id": "U2", "name": "grace.h", "real_name": "Grace Hopper" },
            { "id": "U3", "name": "gone", "deleted": true },
            { "id": "B1", "name": "deploybot", "is_bot": true }
        ]),
    );
    add(
        "channels.json",
        &json!([
            { "id": "C1", "name": "general", "members": ["U1", "U2"] },
            { "id": "C2", "name": "garden", "members": ["U1"], "topic": { "value": "Tomatoes and more" } }
        ]),
    );
    add(
        "groups.json",
        &json!([{ "id": "G1", "name": "plans", "members": ["U1", "U2"] }]),
    );
    add(
        "dms.json",
        &json!([{ "id": "D1", "members": ["U1", "U2"] }]),
    );
    add(
        "garden/2020-07-29.json",
        &json!([
            { "type": "message", "subtype": "channel_join", "user": "U2", "text": "<@U2> has joined", "ts": "1596036000.000100" },
            { "type": "message", "user": "U2", "text": "Planting *today*, <@U1>?", "ts": "1596036100.000200",
              "reactions": [{ "name": "seedling", "users": ["U1"], "count": 1 }] },
            { "type": "message", "user": "U1", "text": "Yes!", "ts": "1596036200.000300", "thread_ts": "1596036100.000200" },
            { "type": "message", "subtype": "bot_message", "username": "Weather", "text": "Sunny all day", "ts": "1596036300.000400" },
            { "type": "message", "user": "U1", "text": "Photos", "ts": "1596036400.000500", "files": [{ "name": "beds.jpg" }] }
        ]),
    );
    add(
        "plans/2020-07-30.json",
        &json!([{ "type": "message", "user": "U1", "text": "Secret shed plans", "ts": "1596120000.000100" }]),
    );
    add(
        "D1/2020-07-30.json",
        &json!([{ "type": "message", "user": "U2", "text": "Psst, Ada", "ts": "1596120100.000100" }]),
    );
    zip.finish().unwrap().into_inner()
}

async fn upload_export(admin: &Browser, export: Vec<u8>) -> String {
    let form = reqwest::multipart::Form::new().part(
        "export",
        reqwest::multipart::Part::bytes(export).file_name("slack.zip"),
    );
    let response = admin
        .client
        .post(admin.url("/admin/import"))
        .header("cookie", &admin.cookie)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.text().await.unwrap()
}

#[tokio::test]
async fn slack_exports_import_once() {
    let server = start().await;
    let admin = admin(&server).await;
    let report = upload_export(&admin, slack_export()).await;
    assert!(
        report.contains("Imported.") && report.contains("6 messages"),
        "{report}"
    );
    assert!(report.contains("@grace.h") && report.contains("1 matched"));

    let directory = admin.page("/channels/browse").await;
    assert!(directory.contains(">garden<") && directory.contains("Tomatoes and more"));
    let garden: i64 = between(&directory, "href=\"/c/", "\"").parse().unwrap();
    let garden = if admin
        .page(&format!("/c/{garden}"))
        .await
        .contains("Sunny all day")
    {
        garden
    } else {
        garden + 1
    };
    let page = admin.page(&format!("/c/{garden}")).await;
    assert!(
        page.contains("<strong>today</strong>") && page.contains("@ada"),
        "{page}"
    );
    assert!(page.contains("Weather") && page.contains("Shared in Slack: beds.jpg"));
    assert!(page.contains("🌱") && page.contains("1 reply"));
    assert!(!page.contains("has joined"));
    assert!(admin.page("/search?q=secret").await.contains("shed plans"));

    // Private channels and direct messages keep their members.
    let home = admin.page("/home").await;
    assert!(home.contains("plans") && home.contains("Grace Hopper"));
    let people = admin.page("/people").await;
    assert!(between(&people, "Deactivated", "</section>").contains("gone"));

    // New people sign in only after an admin sends a reset link.
    assert_eq!(
        sign_in(&server, "grace.h", "").await.1,
        StatusCode::UNAUTHORIZED
    );

    // A second import adds nothing.
    let again = upload_export(&admin, slack_export()).await;
    assert!(
        again.contains("0 messages") && again.contains("already here"),
        "{again}"
    );
    let bad = reqwest::multipart::Form::new().part(
        "export",
        reqwest::multipart::Part::bytes(b"not a zip".to_vec()).file_name("x.zip"),
    );
    let response = admin
        .client
        .post(admin.url("/admin/import"))
        .header("cookie", &admin.cookie)
        .multipart(bad)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}
