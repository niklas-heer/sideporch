// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Public communities: signing up, trust levels, roles and permissions,
//! reports and time-outs.

mod common;

use common::{Browser, PNG, admin, between, home_channel, invite, last_message_id, start};
use reqwest::StatusCode;

async fn set_registration(admin: &Browser, mode: &str, rules: &str) {
    let response = admin
        .post(
            "/admin/community",
            &[
                ("registration", mode),
                ("rules", rules),
                ("days_1", "1"),
                ("visits_1", "1"),
                ("messages_1", "3"),
                ("days_2", "7"),
                ("visits_2", "3"),
                ("messages_2", "20"),
                ("days_3", "30"),
                ("visits_3", "15"),
                ("messages_3", "100"),
                ("new_member_per_minute", "6"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

async fn sign_up(server: &common::Server, username: &str) -> (Browser, reqwest::Response) {
    let mut browser = Browser::anonymous(server);
    let response = browser
        .submit(
            "/signup",
            &[
                ("display_name", "Nia Newcomer"),
                ("username", username),
                ("password", "a long password"),
                ("rules", "agreed"),
                ("website", ""),
            ],
        )
        .await;
    (browser, response)
}

async fn body_of(response: reqwest::Response) -> String {
    response.text().await.unwrap()
}

#[tokio::test]
async fn open_sign_up_starts_people_at_level_zero() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    // Invite only by default: no sign-up page.
    assert_eq!(
        Browser::anonymous(&server).get("/signup").await.status(),
        StatusCode::NOT_FOUND
    );
    set_registration(&admin, "open", "Be *kind*.").await;
    let login = Browser::anonymous(&server).page("/login").await;
    assert!(login.contains("Create an account"));
    let form = Browser::anonymous(&server).page("/signup").await;
    assert!(form.contains("<em>kind</em>") && form.contains("I agree"));

    // Bots fill in the hidden field.
    let mut bot = Browser::anonymous(&server);
    let response = bot
        .submit(
            "/signup",
            &[
                ("display_name", "Bot"),
                ("username", "bot"),
                ("password", "a long password"),
                ("rules", "agreed"),
                ("website", "http://spam.example"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let (nia, response) = sign_up(&server, "nia").await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let profile = nia.page(&format!("/people/{}", nia.user_id().await)).await;
    assert!(profile.contains("Trust level 0: New"));

    // New members talk, but can't post links, upload, notify everyone,
    // create channels or start conversations yet.
    assert_eq!(
        nia.send(general, "Hello, everyone!", None).await,
        StatusCode::NO_CONTENT
    );
    let refused = nia
        .type_message(general, "Buy at https://spam.example")
        .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(refused).await.contains("post links"));
    let refused = nia.upload(general, "look", &[("dot.png", PNG)]).await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        nia.send(general, "@channel hi", None).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        nia.post("/channels", &[("name", "nia-land")])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let admin_id = admin.user_id().await;
    assert_eq!(
        nia.get(&format!("/dm/{admin_id}")).await.status(),
        StatusCode::BAD_REQUEST
    );
    // Notes to themselves are fine, and so is answering a conversation.
    let nia_id = nia.user_id().await;
    assert_eq!(
        nia.get(&format!("/dm/{nia_id}")).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        admin.get(&format!("/dm/{nia_id}")).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        nia.get(&format!("/dm/{admin_id}")).await.status(),
        StatusCode::SEE_OTHER
    );
    // The composer doesn't offer uploads.
    let page = nia.page(&format!("/c/{general}")).await;
    assert!(!page.contains("aria-label=\"Attach files\""));

    // They may send a few messages a minute.
    let mut statuses = Vec::new();
    for n in 0..8 {
        statuses.push(nia.send(general, &format!("message {n}"), None).await);
    }
    assert!(statuses.contains(&StatusCode::BAD_REQUEST), "{statuses:?}");

    // An admin moves them up; now links work.
    assert_eq!(
        admin
            .post(&format!("/people/{nia_id}/trust"), &[("level", "1")])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let profile = admin.page(&format!("/people/{nia_id}")).await;
    assert!(profile.contains("Trust level 1: Basic"));
}

#[tokio::test]
async fn people_earn_trust_by_taking_part() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    set_registration(&admin, "open", "").await;
    // Level 1 needs a message and a visit, not days.
    let response = admin
        .post(
            "/admin/community",
            &[
                ("registration", "open"),
                ("rules", ""),
                ("days_1", "0"),
                ("visits_1", "1"),
                ("messages_1", "2"),
                ("days_2", "7"),
                ("visits_2", "3"),
                ("messages_2", "20"),
                ("days_3", "30"),
                ("visits_3", "15"),
                ("messages_3", "100"),
                ("new_member_per_minute", "0"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let (nia, _) = sign_up(&server, "nia").await;
    let nia_id = nia.user_id().await;
    nia.send(general, "one", None).await;
    assert!(
        nia.page(&format!("/people/{nia_id}"))
            .await
            .contains("Trust level 0")
    );
    nia.send(general, "two", None).await;
    assert!(
        nia.page(&format!("/people/{nia_id}"))
            .await
            .contains("Trust level 1: Basic")
    );
    assert_eq!(
        nia.send(general, "now with https://example.com", None)
            .await,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn sign_ups_can_wait_for_approval() {
    let server = start().await;
    let admin = admin(&server).await;
    set_registration(&admin, "approval", "").await;
    let (_, response) = sign_up(&server, "nia").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        body_of(response)
            .await
            .contains("Your request to join is in")
    );
    // Waiting people can't sign in yet, and the name stays theirs.
    let mut nia = Browser::anonymous(&server);
    let response = nia
        .submit(
            "/login",
            &[("username", "nia"), ("password", "a long password")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(body_of(response).await.contains("still waiting"));
    let (_, again) = sign_up(&server, "nia").await;
    assert!(body_of(again).await.contains("taken"));

    let queue = admin.page("/moderation").await;
    assert!(queue.contains("Nia Newcomer"));
    let signup = between(&queue, "/moderation/signups/", "/approve");
    assert_eq!(
        admin
            .post(&format!("/moderation/signups/{signup}/approve"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let response = nia
        .submit(
            "/login",
            &[("username", "nia"), ("password", "a long password")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let nia_id = nia.user_id().await;
    // Someone vouched for them: level 1.
    assert!(
        nia.page(&format!("/people/{nia_id}"))
            .await
            .contains("Trust level 1")
    );
}

#[tokio::test]
async fn roles_grant_permissions_and_admins_can_turn_uploads_off() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mo = invite(&server, &admin, "Mo Member", "mo").await;
    let mo_id = mo.user_id().await;
    // Invited members upload by default.
    assert_eq!(
        mo.upload(general, "a", &[("a.png", PNG)]).await.status(),
        StatusCode::NO_CONTENT
    );

    // Turn uploads off for everyone but admins and a role.
    let mut form: Vec<(String, String)> = vec![];
    for (key, level) in [
        ("upload_files", "none"),
        ("post_links", "1"),
        ("mention_everyone", "1"),
        ("start_direct_messages", "1"),
        ("create_channels", "1"),
        ("create_private_channels", "1"),
        ("create_polls", "0"),
        ("add_emoji", "1"),
        ("invite_people", "none"),
        ("moderate", "none"),
    ] {
        form.push((format!("level_{key}"), level.to_owned()));
    }
    let pairs: Vec<(&str, &str)> = form.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    assert_eq!(
        admin.post("/admin/permissions", &pairs).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        mo.upload(general, "b", &[("b.png", PNG)]).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        admin.upload(general, "c", &[("c.png", PNG)]).await.status(),
        StatusCode::NO_CONTENT
    );

    // A role brings it back for its members, and lets them invite.
    assert_eq!(
        admin
            .post(
                "/admin/roles",
                &[("name", "Designers"), ("description", "Share mockups")]
            )
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let page = admin.page("/admin/permissions").await;
    let role_id = between(&page, "/admin/roles/", "/delete").to_owned();
    let upload_key = format!("role_{role_id}_upload_files");
    let invite_key = format!("role_{role_id}_invite_people");
    let mut pairs = pairs.clone();
    pairs.push((upload_key.as_str(), "on"));
    pairs.push((invite_key.as_str(), "on"));
    assert_eq!(
        admin.post("/admin/permissions", &pairs).await.status(),
        StatusCode::OK
    );
    let role_field = format!("role_{role_id}");
    assert_eq!(
        admin
            .post(
                &format!("/people/{mo_id}/roles"),
                &[(role_field.as_str(), "on")]
            )
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        mo.upload(general, "d", &[("d.png", PNG)]).await.status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        mo.page(&format!("/people/{mo_id}"))
            .await
            .contains("Designers")
    );
    assert_eq!(
        mo.post("/invites", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    assert!(mo.page("/people").await.contains("/join/"));
}

#[tokio::test]
async fn reports_and_time_outs() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mo = invite(&server, &admin, "Mo Member", "mo").await;
    let troll = invite(&server, &admin, "Tro Ll", "troll").await;
    let troll_id = troll.user_id().await;
    troll.send(general, "Something rude", None).await;
    let page = mo.page(&format!("/c/{general}")).await;
    let message = last_message_id(&page);

    // Members report; they don't see the queue.
    assert_eq!(
        mo.post(
            &format!("/c/{general}/m/{message}/report"),
            &[("reason", "Rude")]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(mo.get("/moderation").await.status(), StatusCode::FORBIDDEN);

    // A moderator role handles it.
    admin.post("/admin/roles", &[("name", "Moderators")]).await;
    let page = admin.page("/admin/permissions").await;
    let role_id = between(&page, "/admin/roles/", "/delete").to_owned();
    let moderate = format!("role_{role_id}_moderate");
    admin
        .post(
            "/admin/permissions",
            &[(moderate.as_str(), "on"), ("level_create_polls", "0")],
        )
        .await;
    let role_field = format!("role_{role_id}");
    let mo_id = mo.user_id().await;
    admin
        .post(
            &format!("/people/{mo_id}/roles"),
            &[(role_field.as_str(), "on")],
        )
        .await;
    let queue = mo.page("/moderation").await;
    assert!(queue.contains("Something rude") && queue.contains("Rude"));
    assert_eq!(
        mo.post(&format!("/moderation/reports/{message}/delete"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(mo.page("/moderation").await.contains("Nothing reported."));
    assert!(
        !mo.page(&format!("/c/{general}"))
            .await
            .contains("Something rude")
    );

    // Time-outs stop posting and reacting until they end.
    assert_eq!(
        mo.post(
            &format!("/people/{troll_id}/timeout"),
            &[("duration", "3600000")]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let refused = troll.type_message(general, "more").await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(body_of(refused).await.contains("paused your posting"));
    assert!(mo.page("/moderation").await.contains("Tro Ll"));
    mo.post(&format!("/people/{troll_id}/timeout"), &[("duration", "0")])
        .await;
    assert_eq!(
        troll.send(general, "sorry", None).await,
        StatusCode::NO_CONTENT
    );
    // Admins can't be timed out.
    let admin_id = admin.user_id().await;
    assert_eq!(
        mo.post(
            &format!("/people/{admin_id}/timeout"),
            &[("duration", "3600000")]
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
}
