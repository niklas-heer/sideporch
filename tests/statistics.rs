// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! The statistics page: what it counts, who it ranks, and who may see it.

mod common;

use common::{Browser, admin, between, home_channel, invite, last_message_id, location, start};
use reqwest::StatusCode;

/// The number a statistics card shows for `key`.
fn stat(page: &str, key: &str) -> i64 {
    between(page, &format!(r#"data-stat="{key}">"#), "<")
        .replace(',', "")
        .trim()
        .parse()
        .unwrap()
}

/// Who a ranking lists, in order, as (person, count).
fn ranking(page: &str, key: &str) -> Vec<(i64, i64)> {
    let list = between(page, &format!(r#"data-ranking="{key}""#), "</ol>");
    list.split("data-person=\"")
        .skip(1)
        .map(|item| {
            let person = item.split('"').next().unwrap().parse().unwrap();
            let count = between(item, "data-count=\"", "\"").parse().unwrap();
            (person, count)
        })
        .collect()
}

async fn hide_from_rankings(browser: &Browser, name: &str, hide: bool) {
    let mut form = reqwest::multipart::Form::new().text("display_name", name.to_owned());
    if hide {
        form = form.text("hide_from_rankings", "on");
    }
    let response = browser
        .client
        .post(browser.url("/settings/profile"))
        .header("cookie", &browser.cookie)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn statistics_count_public_channels_and_rank_people() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mo = invite(&server, &admin, "Mo Member", "mo").await;
    let mo_id = mo.user_id().await;
    let admin_id = admin.user_id().await;

    for body in ["one", "two", "three"] {
        mo.send(general, body, None).await;
    }
    let page = mo.page(&format!("/c/{general}")).await;
    let mine = last_message_id(&page);
    admin.send(general, "hello", None).await;
    let reacted = admin
        .post(
            &format!("/c/{general}/m/{mine}/reactions"),
            &[("emoji", "tada")],
        )
        .await;
    assert_eq!(reacted.status(), StatusCode::SEE_OTHER);

    // A bot's message counts, but bots aren't ranked.
    admin
        .post(
            &format!("/c/{general}/webhooks"),
            &[("name", "Status checks")],
        )
        .await;
    let settings = admin.page(&format!("/c/{general}/settings")).await;
    let hook = format!("/hooks/{}", between(&settings, "/hooks/", "<"));
    admin
        .client
        .post(admin.url(&hook))
        .json(&serde_json::json!({ "text": "All systems go" }))
        .send()
        .await
        .unwrap();

    // Private channels and direct messages never count.
    let created = admin
        .post("/channels", &[("name", "secret"), ("private", "on")])
        .await;
    let secret: i64 = location(&created)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.send(secret, "hush", None).await;
    let dm: i64 = location(&admin.get(&format!("/dm/{mo_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    admin.send(dm, "psst", None).await;

    // Members see the page, linked from the sidebar.
    assert!(mo.page("/home").await.contains(r#"href="/statistics""#));
    let page = mo.page("/statistics?period=7d").await;
    assert_eq!(stat(&page, "messages"), 5);
    assert_eq!(stat(&page, "people"), 2);
    assert_eq!(stat(&page, "reactions"), 1);
    assert_eq!(ranking(&page, "messages"), vec![(mo_id, 3), (admin_id, 1)]);
    assert_eq!(ranking(&page, "reactions"), vec![(mo_id, 1)]);
    assert!(between(&page, r#"data-ranking="channels""#, "</ol>").contains("general"));
    assert!(!page.contains("secret"));
    assert!(page.contains("You rank #1"), "{page}");
    // Every period works.
    for period in ["30d", "12m", "all"] {
        let page = mo.page(&format!("/statistics?period={period}")).await;
        assert_eq!(stat(&page, "messages"), 5, "{period}");
    }

    // Leaving the rankings keeps the totals.
    hide_from_rankings(&mo, "Mo Member", true).await;
    let page = mo.page("/statistics").await;
    assert_eq!(stat(&page, "messages"), 5);
    assert_eq!(ranking(&page, "messages"), vec![(admin_id, 1)]);
    assert!(page.contains("You left the rankings"));
    hide_from_rankings(&mo, "Mo Member", false).await;
    assert_eq!(ranking(&mo.page("/statistics").await, "messages").len(), 2);

    // Admins decide who sees them; a form without levels leaves every
    // permission to roles.
    assert_eq!(
        admin.post("/admin/permissions", &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        mo.get("/statistics").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert!(!mo.page("/home").await.contains(r#"href="/statistics""#));
    assert_eq!(admin.get("/statistics").await.status(), StatusCode::OK);
}
