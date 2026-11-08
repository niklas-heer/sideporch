// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

mod common;

use common::{Browser, Server, admin, home_channel, invite, last_message_id, location, start};
use reqwest::StatusCode;

/// The newest reply on a thread page.
fn last_reply_id(page: &str) -> i64 {
    let list = common::between(page, r#"id="replies""#, "</ol>");
    list.rsplit(r#"data-message-id=""#)
        .next()
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

/// Creates a channel and returns its id.
async fn create_channel(browser: &Browser, name: &str, private: bool) -> i64 {
    let mut form = vec![("name", name)];
    if private {
        form.push(("private", "on"));
    }
    let created = browser.post("/channels", &form).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    location(&created)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

/// Ada starts a thread in #general with Bea and Cy in the server.
struct Thread {
    // Kept alive for the test; the server stops when it is dropped.
    _server: Server,
    ada: Browser,
    bea: Browser,
    cy: Browser,
    channel: i64,
    root: i64,
}

async fn start_thread() -> Thread {
    let server = start().await;
    let ada = admin(&server).await;
    let channel = home_channel(&ada).await;
    let bea = invite(&server, &ada, "Bea", "bea").await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    assert_eq!(
        ada.send(channel, "Shall we pick a date?", None).await,
        StatusCode::NO_CONTENT
    );
    let root = last_message_id(&ada.page(&format!("/c/{channel}")).await);
    Thread {
        _server: server,
        ada,
        bea,
        cy,
        channel,
        root,
    }
}

impl Thread {
    /// Follows or unfollows the thread the way its button does.
    async fn set_following(&self, who: &Browser, following: bool) {
        let verb = if following { "follow" } else { "unfollow" };
        let response = who
            .post(&format!("/c/{}/t/{}/{verb}", self.channel, self.root), &[])
            .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{verb}");
        assert_eq!(
            location(&response),
            format!("/c/{}/t/{}", self.channel, self.root)
        );
    }

    async fn reply(&self, who: &Browser, body: &str) {
        assert_eq!(
            who.send(self.channel, body, Some(self.root)).await,
            StatusCode::NO_CONTENT
        );
    }
}

#[tokio::test]
async fn following_without_posting_makes_you_a_recipient() {
    let thread = start_thread().await;
    let Thread { ada, bea, cy, .. } = &thread;

    // Cy has never written in the thread; following it is enough.
    let page = cy
        .page(&format!("/c/{}/t/{}", thread.channel, thread.root))
        .await;
    assert!(page.contains("Follow thread"));
    assert!(!page.contains("Unfollow thread"));
    thread.set_following(cy, true).await;
    let page = cy
        .page(&format!("/c/{}/t/{}", thread.channel, thread.root))
        .await;
    assert!(page.contains("Unfollow thread"));

    thread.reply(ada, "Friday works for me").await;
    assert!(cy.page("/activity").await.contains("Friday works for me"));
    // Bea neither wrote in the thread nor follows it.
    assert!(!bea.page("/activity").await.contains("Friday works for me"));
}

#[tokio::test]
async fn unfollowing_removes_you_even_after_posting() {
    let thread = start_thread().await;
    let Thread { ada, bea, cy, .. } = &thread;

    // Bea wrote in the thread, so she is a recipient until she unfollows.
    thread.reply(bea, "I can do Friday").await;
    thread.set_following(cy, true).await;
    thread.set_following(bea, false).await;
    let page = bea
        .page(&format!("/c/{}/t/{}", thread.channel, thread.root))
        .await;
    assert!(page.contains("Follow thread"));

    thread.reply(ada, "Who else is in?").await;
    assert!(cy.page("/activity").await.contains("Who else is in?"));
    assert!(!bea.page("/activity").await.contains("Who else is in?"));
}

#[tokio::test]
async fn mentions_still_reach_someone_who_unfollowed() {
    let thread = start_thread().await;
    let Thread { ada, bea, .. } = &thread;

    thread.reply(bea, "Count me in").await;
    thread.set_following(bea, false).await;
    thread.reply(ada, "@bea, can you book the room?").await;

    let activity = bea.page("/activity").await;
    assert!(activity.contains("can you book the room?"));
    assert!(activity.contains("Mentioned you"));
}

#[tokio::test]
async fn replying_after_unfollowing_follows_again() {
    let thread = start_thread().await;
    let Thread { ada, bea, .. } = &thread;

    thread.reply(bea, "Count me in").await;
    thread.set_following(bea, false).await;
    thread.reply(ada, "Anyone else?").await;
    assert!(!bea.page("/activity").await.contains("Anyone else?"));

    // Replying is a way back in: the unfollow ends with the reply.
    thread.reply(bea, "Actually, I'm in").await;
    let page = bea
        .page(&format!("/c/{}/t/{}", thread.channel, thread.root))
        .await;
    assert!(page.contains("Unfollow thread"));
    thread.reply(ada, "Great, see you Friday").await;
    assert!(
        bea.page("/activity")
            .await
            .contains("Great, see you Friday")
    );
}

#[tokio::test]
async fn following_a_thread_you_cannot_read_is_not_found() {
    let server = start().await;
    let ada = admin(&server).await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    let private = create_channel(&ada, "board", true).await;
    assert_eq!(
        ada.send(private, "Confidential", None).await,
        StatusCode::NO_CONTENT
    );
    let root = last_message_id(&ada.page(&format!("/c/{private}")).await);

    let follow = cy.post(&format!("/c/{private}/t/{root}/follow"), &[]).await;
    assert_eq!(follow.status(), StatusCode::NOT_FOUND);
    let unfollow = cy
        .post(&format!("/c/{private}/t/{root}/unfollow"), &[])
        .await;
    assert_eq!(unfollow.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        cy.get(&format!("/c/{private}/t/{root}")).await.status(),
        StatusCode::NOT_FOUND
    );

    // Following takes a root message: a reply id is not one.
    let general = home_channel(&ada).await;
    assert_eq!(
        ada.send(general, "Hello", None).await,
        StatusCode::NO_CONTENT
    );
    let hello = last_message_id(&ada.page(&format!("/c/{general}")).await);
    assert_eq!(
        ada.send(general, "Hi back", Some(hello)).await,
        StatusCode::NO_CONTENT
    );
    let thread_page = ada.page(&format!("/c/{general}/t/{hello}")).await;
    let reply = last_reply_id(&thread_page);
    assert_eq!(
        ada.post(&format!("/c/{general}/t/{reply}/follow"), &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
}

/// Posts a message in `channel` and returns its id.
async fn post_root(browser: &Browser, channel: i64, body: &str) -> i64 {
    assert_eq!(
        browser.send(channel, body, None).await,
        StatusCode::NO_CONTENT
    );
    last_message_id(&browser.page(&format!("/c/{channel}")).await)
}

async fn follow(browser: &Browser, channel: i64, root: i64) {
    let response = browser
        .post(&format!("/c/{channel}/t/{root}/follow"), &[])
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn unread_followed_threads_come_first_on_activity() {
    let server = start().await;
    let ada = admin(&server).await;
    let channel = home_channel(&ada).await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    let older = post_root(&ada, channel, "Plan A?").await;
    let newer = post_root(&ada, channel, "Plan B?").await;
    follow(&cy, channel, older).await;
    follow(&cy, channel, newer).await;

    // Plan A gets a reply first and is still unread; Plan B's replies are
    // newer, but Cy has read them.
    assert_eq!(
        ada.send(channel, "A: Friday works", Some(older)).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        ada.send(channel, "B: Saturday works", Some(newer)).await,
        StatusCode::NO_CONTENT
    );
    cy.page(&format!("/c/{channel}/t/{newer}")).await;

    let page = cy.page("/activity").await;
    let unread = page.find("Plan A?").unwrap();
    let read = page.find("Plan B?").unwrap();
    assert!(unread < read, "unread threads come first");
    assert_eq!(page.matches("New replies").count(), 1);
}

#[tokio::test]
async fn opening_a_thread_marks_it_read() {
    let server = start().await;
    let ada = admin(&server).await;
    let channel = home_channel(&ada).await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    let root = post_root(&ada, channel, "Plan A?").await;
    follow(&cy, channel, root).await;
    assert_eq!(
        ada.send(channel, "Friday works", Some(root)).await,
        StatusCode::NO_CONTENT
    );
    assert!(cy.page("/activity").await.contains("New replies"));

    cy.page(&format!("/c/{channel}/t/{root}")).await;
    assert!(!cy.page("/activity").await.contains("New replies"));
    // Ada's own replies never count as unread for her.
    assert!(!ada.page("/activity").await.contains("New replies"));
}

#[tokio::test]
async fn a_reply_read_live_in_the_open_thread_counts_as_read() {
    let server = start().await;
    let ada = admin(&server).await;
    let channel = home_channel(&ada).await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    let root = post_root(&ada, channel, "Plan A?").await;
    follow(&cy, channel, root).await;
    let thread = format!("/c/{channel}/t/{root}");
    cy.page(&thread).await;
    assert_eq!(
        ada.send(channel, "Friday works", Some(root)).await,
        StatusCode::NO_CONTENT
    );
    assert!(cy.page("/activity").await.contains("New replies"));

    let reply = last_reply_id(&ada.page(&thread).await);
    let mut live = cy.live().await;
    live.send(&serde_json::json!({
        "type": "read", "channel_id": channel, "message_id": reply, "thread_id": root,
    }))
    .await;
    let mut cleared = false;
    for _ in 0..50 {
        if !cy.page("/activity").await.contains("New replies") {
            cleared = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(cleared, "reading it live marks the thread read");
}

#[tokio::test]
async fn a_read_notice_cannot_mark_a_thread_in_another_channel() {
    let server = start().await;
    let ada = admin(&server).await;
    let channel = home_channel(&ada).await;
    let other = create_channel(&ada, "elsewhere", false).await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    let root = post_root(&ada, channel, "Plan A?").await;
    follow(&cy, channel, root).await;
    assert_eq!(
        ada.send(channel, "Friday works", Some(root)).await,
        StatusCode::NO_CONTENT
    );
    let mut live = cy.live().await;
    live.send(&serde_json::json!({
        "type": "read", "channel_id": other, "message_id": root, "thread_id": root,
    }))
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(cy.page("/activity").await.contains("New replies"));
}

#[tokio::test]
async fn unfollowed_threads_leave_the_list_and_started_ones_join_it() {
    let server = start().await;
    let ada = admin(&server).await;
    let channel = home_channel(&ada).await;
    let cy = invite(&server, &ada, "Cy", "cy").await;
    let root = post_root(&ada, channel, "Plan A?").await;
    // A plain post nobody answered is not a thread yet.
    post_root(&ada, channel, "Lunch?").await;
    assert!(!ada.page("/activity").await.contains("Lunch?"));

    // Ada wrote the thread, so she follows it once someone replies.
    assert!(!ada.page("/activity").await.contains("Plan A?"));
    assert_eq!(
        cy.send(channel, "Friday works", Some(root)).await,
        StatusCode::NO_CONTENT
    );
    assert!(ada.page("/activity").await.contains("Plan A?"));

    assert_eq!(
        ada.post(&format!("/c/{channel}/t/{root}/unfollow"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(!ada.page("/activity").await.contains("Plan A?"));
}
