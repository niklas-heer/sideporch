//! Slack-compatible incoming webhooks, checked against payloads captured
//! from Gatus (see tests/fixtures/gatus/README.md).

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

use common::{Browser, admin, between, home_channel, location, start};
use reqwest::StatusCode;

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/gatus/{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path).unwrap()
}

/// Creates a webhook in the channel and returns its URL path.
async fn create_webhook(browser: &Browser, channel_id: i64, name: &str) -> String {
    let response = browser
        .post(&format!("/c/{channel_id}/webhooks"), &[("name", name)])
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let settings = browser.page(&format!("/c/{channel_id}/settings")).await;
    format!("/hooks/{}", between(&settings, "/hooks/", "<"))
}

/// Sends a body the way Gatus does and returns the status and response text.
async fn deliver(browser: &Browser, hook: &str, body: String) -> (StatusCode, String) {
    let response = browser
        .client
        .post(browser.url(hook))
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .unwrap();
    (response.status(), response.text().await.unwrap())
}

#[tokio::test]
async fn gatus_slack_alerts_render_in_the_channel() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let hook = create_webhook(&admin, general, "Status checks").await;
    let sender = Browser::anonymous(&server);

    let (status, text) = deliver(&sender, &hook, fixture("slack-triggered")).await;
    assert_eq!((status, text.as_str()), (StatusCode::OK, "ok"));

    let page = admin.page(&format!("/c/{general}")).await;
    assert!(
        page.contains(r#"<span class="font-bold">Status checks</span>"#),
        "the webhook name is the sender"
    );
    assert!(page.contains("border-left-color: #DD0000"));
    assert!(page.contains(r#"aria-label="helmet_with_white_cross">⛑️</span> Gatus"#));
    assert!(page.contains("An alert for <strong>core/website</strong> has been triggered due to having failed 3 time(s) in a row:"));
    assert!(page.contains("<blockquote>Homepage is unreachable</blockquote>"));
    assert!(page.contains("Condition results"));
    assert!(
        page.contains(
            r#"<span role="img" aria-label="x">❌</span> - <code>[STATUS] == 200</code>"#
        )
    );
    assert!(page.contains("<code>[RESPONSE_TIME] &lt; 500</code>"));

    let (status, _) = deliver(&sender, &hook, fixture("slack-resolved")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = deliver(&sender, &hook, fixture("slack-no-conditions")).await;
    assert_eq!(status, StatusCode::OK);

    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains("border-left-color: #36A64F"));
    assert!(page.contains("has been resolved after passing successfully 2 time(s) in a row"));
    assert!(
        page.contains(r#"<p class="font-bold">Uptime</p>"#),
        "custom Gatus title"
    );
}

#[tokio::test]
async fn gatus_mattermost_alerts_pick_their_channel_and_sender() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let alerts = location(&admin.post("/channels", &[("name", "alerts")]).await);
    let hook = create_webhook(&admin, general, "Monitoring").await;
    let sender = Browser::anonymous(&server);

    for name in [
        "mattermost-triggered",
        "mattermost-resolved",
        "mattermost-no-conditions",
    ] {
        let (status, text) = deliver(&sender, &hook, fixture(name)).await;
        assert_eq!((status, text.as_str()), (StatusCode::OK, "ok"), "{name}");
    }

    let page = admin.page(&alerts).await;
    assert!(page.contains(r#"<span class="font-bold">gatus</span>"#));
    assert!(page.contains(
        r#"src="https://raw.githubusercontent.com/TwiN/gatus/master/.github/assets/logo.png""#
    ));
    assert!(page.contains("has been triggered due to having failed 1 time(s) in a row"));
    assert_eq!(page.matches("data-message-id=").count(), 3);

    let general_page = admin.page(&format!("/c/{general}")).await;
    assert!(
        !general_page.contains("data-message-id="),
        "nothing landed in the webhook's own channel"
    );
}

#[tokio::test]
async fn unknown_channel_overrides_fall_back_to_the_webhook_channel() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let hook = create_webhook(&admin, general, "Monitoring").await;
    let sender = Browser::anonymous(&server);

    // The fixture names #alerts, which doesn't exist here.
    let (status, _) = deliver(&sender, &hook, fixture("mattermost-triggered")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .contains("core/website")
    );
}

#[tokio::test]
async fn webhook_errors_use_slack_codes() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let hook = create_webhook(&admin, general, "CI").await;
    let sender = Browser::anonymous(&server);

    assert_eq!(
        deliver(&sender, &hook, "{not json".into()).await,
        (StatusCode::BAD_REQUEST, "invalid_payload".into())
    );
    assert_eq!(
        deliver(&sender, &hook, r#"{"text": ""}"#.into()).await,
        (StatusCode::BAD_REQUEST, "no_text".into())
    );
    assert_eq!(
        deliver(
            &sender,
            "/hooks/not-a-real-token",
            r#"{"text": "hi"}"#.into()
        )
        .await,
        (StatusCode::NOT_FOUND, "no_service".into())
    );

    // Deleting a webhook disables its URL.
    let settings = admin.page(&format!("/c/{general}/settings")).await;
    let delete = format!(
        "/c/{general}/webhooks/{}",
        between(&settings, &format!("/c/{general}/webhooks/"), "\"")
    );
    assert_eq!(
        admin.post(&delete, &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        deliver(&sender, &hook, r#"{"text": "hi"}"#.into()).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn slack_form_encoded_payloads_are_accepted() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let hook = create_webhook(&admin, general, "Legacy").await;
    let sender = Browser::anonymous(&server);

    let response = sender
        .client
        .post(sender.url(&hook))
        .form(&[(
            "payload",
            r#"{"text": "Deploy <https://ci.example/42|#42> finished :rocket:"}"#,
        )])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains(r#"<a href="https://ci.example/42" target="_blank" rel="noopener noreferrer nofollow">#42</a> finished"#));
}
