// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Demo mode: a server that starts over every day, keeping what an admin
//! chose.

mod common;

use common::{Browser, admin, between, home_channel, invite, location, start};
use reqwest::StatusCode;

async fn channel(admin: &Browser, name: &str, private: bool) -> i64 {
    let mut form = vec![("name", name)];
    if private {
        form.push(("private", "on"));
    }
    location(&admin.post("/channels", &form).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap()
}

#[tokio::test]
async fn demo_resets_keep_what_admins_chose() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let news = channel(&admin, "news", false).await;
    let team = channel(&admin, "team", true).await;
    let chatter = channel(&admin, "chatter", false).await;
    let visitor = invite(&server, &admin, "Vic Visitor", "vic").await;
    let helper = invite(&server, &admin, "Hana Helper", "hana").await;
    admin.post("/admin/roles", &[("name", "Helpers")]).await;
    let permissions = admin.page("/admin/permissions").await;
    let role_id = between(&permissions, "/admin/roles/", "/badge").to_owned();
    let role_field = format!("role_{role_id}");
    let helper_id = helper.user_id().await;
    admin
        .post(
            &format!("/people/{helper_id}/roles"),
            &[(role_field.as_str(), "on")],
        )
        .await;

    admin.send(news, "Welcome to the demo!", None).await;
    admin.send(team, "Plans for tomorrow", None).await;
    visitor.send(news, "First!", None).await;
    visitor.send(chatter, "Hello chatter", None).await;
    helper.send(news, "Happy to help", None).await;
    let visitor_id = visitor.user_id().await;
    let dm = location(&admin.get(&format!("/dm/{visitor_id}")).await);

    // Switched on, with #news and the private #team kept.
    let general_field = general.to_string();
    let news_field = news.to_string();
    let team_field = team.to_string();
    let saved = admin
        .post(
            "/admin/demo",
            &[
                ("enabled", "on"),
                ("hour", "4"),
                ("keep", &general_field),
                ("keep", &news_field),
                ("keep", &team_field),
            ],
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert!(visitor.page("/home").await.contains("This is a demo"));
    assert!(
        Browser::anonymous(&server)
            .page("/login")
            .await
            .contains("04:00 UTC")
    );

    let reset = admin.post("/admin/demo/reset", &[]).await;
    assert!(reset.text().await.unwrap().contains("1 people"));

    // Visitors are gone, with what they wrote; admins and people with a
    // role stay, and so do kept channels.
    assert_eq!(visitor.get("/home").await.status(), StatusCode::SEE_OTHER);
    assert_eq!(helper.get("/home").await.status(), StatusCode::OK);
    let news_page = admin.page(&format!("/c/{news}")).await;
    assert!(news_page.contains("Welcome to the demo!") && news_page.contains("Happy to help"));
    assert!(!news_page.contains("First!"));
    assert!(
        admin
            .page(&format!("/c/{team}"))
            .await
            .contains("Plans for tomorrow")
    );
    assert_eq!(
        admin.get(&format!("/c/{chatter}")).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(admin.get(&dm).await.status(), StatusCode::NOT_FOUND);
}
