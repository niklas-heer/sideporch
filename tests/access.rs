// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Running in public: limits on sign-ins and sign-ups by address, the
//! proof of work sign-ups need, bans, and removing spammers.

mod common;

use common::{Browser, Server, admin, start_behind_proxy};
use reqwest::StatusCode;

async fn open_sign_up(admin: &Browser) {
    let response = admin
        .post(
            "/admin/community",
            &[
                ("registration", "open"),
                ("rules", ""),
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

async fn sign_in(
    server: &Server,
    address: &str,
    username: &str,
    password: &str,
) -> reqwest::Response {
    Browser::anonymous(server)
        .at_address(address)
        .post("/login", &[("username", username), ("password", password)])
        .await
}

async fn sign_up(server: &Server, address: &str, username: &str) -> (Browser, reqwest::Response) {
    let mut browser = Browser::anonymous(server).at_address(address);
    let (challenge, proof) = common::sign_up_proof(&browser).await;
    let response = browser
        .submit(
            "/signup",
            &[
                ("display_name", username),
                ("username", username),
                ("password", "a long password"),
                ("website", ""),
                ("challenge", &challenge),
                ("proof", &proof),
            ],
        )
        .await;
    (browser, response)
}

#[tokio::test]
async fn wrong_passwords_pause_signing_in_by_address_and_by_account() {
    let server = start_behind_proxy().await;
    admin(&server).await;

    // Guessing from one address: after ten misses, even the right password
    // waits there, but not elsewhere.
    for guess in 0..10 {
        let response = sign_in(&server, "203.0.113.5", &format!("nobody{guess}"), "guess").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let paused = sign_in(&server, "203.0.113.5", "ada", "correct horse").await;
    assert_eq!(paused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        paused
            .text()
            .await
            .unwrap()
            .contains("Too many wrong passwords")
    );
    let elsewhere = sign_in(&server, "198.51.100.1", "ada", "correct horse").await;
    assert_eq!(elsewhere.status(), StatusCode::SEE_OTHER);

    // Guessing one account from many addresses pauses that account.
    for guess in 0..20 {
        let response = sign_in(&server, &format!("192.0.2.{guess}"), "ada", "wrong").await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let paused = sign_in(&server, "198.51.100.2", "ada", "correct horse").await;
    assert_eq!(paused.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn sign_ups_need_a_proof_of_work_and_are_limited_per_address() {
    let server = start_behind_proxy().await;
    let admin = admin(&server).await;
    open_sign_up(&admin).await;

    // Without solving the form's puzzle, nobody gets in.
    let mut bot = Browser::anonymous(&server).at_address("203.0.113.9");
    let form = bot.page("/signup").await;
    let challenge = common::between(&form, r#"data-proof=""#, "\"").to_owned();
    let refused = bot
        .submit(
            "/signup",
            &[
                ("display_name", "Bot"),
                ("username", "bot"),
                ("password", "a long password"),
                ("website", ""),
                ("challenge", &challenge),
                ("proof", "1"),
            ],
        )
        .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert!(refused.text().await.unwrap().contains("keeps bots out"));

    // Three accounts an hour from one address; others still get in.
    for name in ["one", "two", "three"] {
        let (_, response) = sign_up(&server, "203.0.113.9", name).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER, "{name}");
    }
    let (_, fourth) = sign_up(&server, "203.0.113.9", "four").await;
    assert_eq!(fourth.status(), StatusCode::BAD_REQUEST);
    assert!(fourth.text().await.unwrap().contains("Several accounts"));
    let (_, other) = sign_up(&server, "198.51.100.9", "four").await;
    assert_eq!(other.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn moderators_ban_people_their_addresses_and_emails() {
    let server = start_behind_proxy().await;
    let admin = admin(&server).await.at_address("198.51.100.1");
    let general = common::home_channel(&admin).await;
    let spammer = common::invite(&server, &admin, "Spam Bot", "spambot")
        .await
        .at_address("203.0.113.66");
    let bystander = common::invite(&server, &admin, "Bea", "bea")
        .await
        .at_address("192.0.2.10");
    // Signed-in visits note the address.
    spammer.send(general, "Cheap followers!", None).await;
    spammer.send(general, "Cheap followers, again!", None).await;
    let spammer_id = spammer.user_id().await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let profile = admin.page(&format!("/people/{spammer_id}")).await;
    assert!(profile.contains("203.0.113.66"), "{profile}");
    // Nobody else sees addresses.
    let theirs = bystander.page(&format!("/people/{spammer_id}")).await;
    assert!(!theirs.contains("203.0.113.66"));

    // Banning deactivates them, bans the address and removes their posts.
    let banned = admin
        .post(
            &format!("/people/{spammer_id}/ban"),
            &[
                ("reason", "Spam"),
                ("duration", "0"),
                ("addresses", "on"),
                ("remove", "on"),
            ],
        )
        .await;
    assert_eq!(banned.status(), StatusCode::SEE_OTHER);
    let channel = admin.page(&format!("/c/{general}")).await;
    assert!(!channel.contains("Cheap followers"));
    let refused = Browser::anonymous(&server)
        .at_address("203.0.113.66")
        .get("/login")
        .await;
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(bystander.get("/home").await.status(), StatusCode::OK);
    let moderation = admin.page("/moderation").await;
    assert!(common::between(&moderation, "data-bans", "</ul>").contains("203.0.113.66"));

    // Ranges and domains from the moderation page, never your own address.
    let own = admin
        .post(
            "/moderation/bans",
            &[
                ("target", "198.51.100.0/24"),
                ("reason", ""),
                ("duration", "86400000"),
            ],
        )
        .await;
    assert_eq!(own.status(), StatusCode::BAD_REQUEST);
    admin
        .post(
            "/moderation/bans",
            &[
                ("target", "192.0.2.0/24"),
                ("reason", "Botnet"),
                ("duration", "86400000"),
            ],
        )
        .await;
    assert_eq!(bystander.get("/home").await.status(), StatusCode::FORBIDDEN);
    let moderation = admin.page("/moderation").await;
    let ban_id = common::between(&moderation, "/moderation/bans/", "/lift").to_owned();
    admin
        .post(&format!("/moderation/bans/{ban_id}/lift"), &[])
        .await;
    assert_eq!(bystander.get("/home").await.status(), StatusCode::OK);

    // Admins can't be banned, and nobody bans themselves.
    let admin_id = admin.user_id().await;
    assert_eq!(
        admin
            .post(&format!("/people/{admin_id}/ban"), &[("duration", "0")])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        bystander
            .post(&format!("/people/{admin_id}/ban"), &[("duration", "0")])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn repeated_messages_are_refused_and_reported_newcomers_pause() {
    let server = start_behind_proxy().await;
    let admin = admin(&server).await;
    let general = common::home_channel(&admin).await;
    open_sign_up(&admin).await;
    let (newcomer, _) = sign_up(&server, "203.0.113.20", "newcomer").await;
    let bea = common::invite(&server, &admin, "Bea", "bea").await;

    // The same message a third time within ten minutes is refused; short
    // ones may repeat.
    let spam = "Visit my shop for cheap followers";
    assert_eq!(
        newcomer.send(general, spam, None).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        newcomer.send(general, spam, None).await,
        StatusCode::NO_CONTENT
    );
    let third = newcomer.type_message(general, spam).await;
    assert_eq!(third.status(), StatusCode::BAD_REQUEST);
    assert!(third.text().await.unwrap().contains("same message"));
    for _ in 0..3 {
        assert_eq!(
            newcomer.send(general, "ok", None).await,
            StatusCode::NO_CONTENT
        );
    }

    // Two people report the newcomer, who then can't post until a
    // moderator looks.
    let page = admin.page(&format!("/c/{general}")).await;
    let message = page
        .split("<li id=\"m")
        .skip(1)
        .find(|item| item.contains("Visit my shop"))
        .and_then(|item| item.split('"').next())
        .unwrap()
        .to_owned();
    for reporter in [&admin, &bea] {
        let response = reporter
            .post(
                &format!("/c/{general}/m/{message}/report"),
                &[("reason", "Spam")],
            )
            .await;
        assert!(response.status().is_redirection() || response.status().is_success());
    }
    let paused = newcomer.type_message(general, "Something new").await;
    assert_eq!(paused.status(), StatusCode::BAD_REQUEST);
    assert!(paused.text().await.unwrap().contains("posting is paused"));
}
