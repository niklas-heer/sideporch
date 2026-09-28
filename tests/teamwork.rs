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
