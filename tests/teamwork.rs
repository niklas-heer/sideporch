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
use serde_json::Value;

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
