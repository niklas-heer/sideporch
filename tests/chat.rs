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

use std::time::Duration;

use common::{Browser, admin, between, home_channel, invite, location, setup_path, start};
use reqwest::StatusCode;

#[tokio::test]
async fn the_first_visitor_creates_the_admin_account() {
    let server = start().await;
    let visitor = Browser::anonymous(&server);

    assert_eq!(location(&visitor.get("/").await), "/setup");
    let page = visitor.page("/setup").await;
    assert!(page.contains("The first person to open this page becomes the admin"));
    assert_eq!(
        visitor.get("/setup/not-the-token").await.status(),
        StatusCode::NOT_FOUND
    );

    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains("This is the start of #general."));

    // Setup works once; afterwards new people need an invite.
    assert_eq!(location(&visitor.get(&setup_path(&server)).await), "/login");
    let late = visitor
        .post(
            "/setup",
            &[
                ("display_name", "Mallory"),
                ("username", "mallory"),
                ("password", "too late now"),
            ],
        )
        .await;
    assert_eq!(late.status(), StatusCode::NOT_FOUND);
    assert_eq!(location(&visitor.get("/").await), "/login");
}

#[tokio::test]
async fn signing_in_and_out() {
    let server = start().await;
    admin(&server).await;
    let mut browser = Browser::anonymous(&server);

    let wrong = browser
        .submit("/login", &[("username", "ada"), ("password", "nope")])
        .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert!(wrong.text().await.unwrap().contains("don't match"));
    assert_eq!(location(&browser.get("/home").await), "/login?next=/home");

    let right = browser
        .submit(
            "/login",
            &[("username", "ADA"), ("password", "correct horse")],
        )
        .await;
    assert_eq!(right.status(), StatusCode::SEE_OTHER);
    assert!(browser.page("/home").await.contains("general"));

    browser.submit("/logout", &[]).await;
    assert_eq!(location(&browser.get("/home").await), "/login?next=/home");
}

#[tokio::test]
async fn invited_people_join_and_talk_in_channels() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let general = home_channel(&bea).await;

    let response = bea
        .post(
            &format!("/c/{general}/messages"),
            &[("body", "Hi **everyone** <script>alert(1)</script>")],
        )
        .await;
    assert_eq!(location(&response), format!("/c/{general}"));

    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains("Hi <strong>everyone</strong> &lt;script&gt;alert(1)&lt;/script&gt;"));
    assert!(!page.contains("<script>alert(1)"));

    // Only admins manage invites.
    assert_eq!(
        bea.post("/invites", &[]).await.status(),
        StatusCode::FORBIDDEN
    );

    // Usernames are unique, ignoring case.
    let mut newcomer = Browser::anonymous(&server);
    admin.post("/invites", &[]).await;
    let people = admin.page("/people").await;
    let token = between(&people, "/join/", "<");
    let duplicate = newcomer
        .submit(
            &format!("/join/{token}"),
            &[
                ("display_name", "Other Bea"),
                ("username", "BEA"),
                ("password", "a long password"),
            ],
        )
        .await;
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    assert!(
        duplicate
            .text()
            .await
            .unwrap()
            .contains("That username is taken")
    );
}

#[tokio::test]
async fn revoked_invites_stop_working() {
    let server = start().await;
    let admin = admin(&server).await;
    admin.post("/invites", &[]).await;
    let token = between(&admin.page("/people").await, "/join/", "<").to_owned();

    admin.post(&format!("/invites/{token}/revoke"), &[]).await;

    let visitor = Browser::anonymous(&server);
    let response = visitor.get(&format!("/join/{token}")).await;
    assert_eq!(response.status(), StatusCode::GONE);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("expired or was revoked")
    );
}

#[tokio::test]
async fn channels_can_be_created_and_named_safely() {
    let server = start().await;
    let admin = admin(&server).await;

    let created = admin.post("/channels", &[("name", "#Garden Club")]).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let page = admin.page(&location(&created)).await;
    assert!(page.contains("This is the start of #garden-club."));

    let duplicate = admin.post("/channels", &[("name", "garden-club")]).await;
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);
    let invalid = admin.post("/channels", &[("name", "no/slashes")]).await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn threads_collect_replies_under_one_root() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;

    assert_eq!(
        admin.send(general, "Who's bringing snacks?", None).await,
        StatusCode::NO_CONTENT
    );
    let page = admin.page(&format!("/c/{general}")).await;
    let root: i64 = between(&page, r#"data-message-id=""#, "\"")
        .parse()
        .unwrap();

    assert_eq!(
        admin.send(general, "I will", Some(root)).await,
        StatusCode::NO_CONTENT
    );
    let thread = admin.page(&format!("/c/{general}/t/{root}")).await;
    let reply: i64 = between(
        between(&thread, r#"id="replies""#, "</ol>"),
        r#"data-message-id=""#,
        "\"",
    )
    .parse()
    .unwrap();
    // Replying to a reply stays in the same thread.
    assert_eq!(
        admin.send(general, "Me too", Some(reply)).await,
        StatusCode::NO_CONTENT
    );

    let thread = admin.page(&format!("/c/{general}/t/{root}")).await;
    let replies = between(&thread, r#"id="replies""#, "</ol>");
    assert!(replies.contains("I will") && replies.contains("Me too"));
    assert!(thread.contains("2 replies"));

    let channel = admin.page(&format!("/c/{general}")).await;
    let messages = between(&channel, r#"id="messages""#, "</ol>");
    assert!(messages.contains("2 replies"));
    assert!(
        !messages.contains("Me too"),
        "replies stay out of the channel"
    );

    // Without JavaScript, replying returns to the thread.
    let response = admin
        .post(
            &format!("/c/{general}/messages"),
            &[("body", "no js"), ("parent_id", &root.to_string())],
        )
        .await;
    assert_eq!(location(&response), format!("/c/{general}/t/{root}"));
}

#[tokio::test]
async fn direct_messages_are_private_to_their_members() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let bea_id = bea.user_id().await;

    let opened = admin.get(&format!("/dm/{bea_id}")).await;
    let dm = location(&opened);
    let dm_id: i64 = dm.trim_start_matches("/c/").parse().unwrap();
    assert_eq!(
        admin.send(dm_id, "Just between us", None).await,
        StatusCode::NO_CONTENT
    );

    // Opening the conversation again reuses it, from either side.
    assert_eq!(location(&admin.get(&format!("/dm/{bea_id}")).await), dm);
    let admin_id = admin.user_id().await;
    assert_eq!(location(&bea.get(&format!("/dm/{admin_id}")).await), dm);

    // The recipient sees the conversation, marked unread, in the sidebar.
    let sidebar = bea.page("/home").await;
    let link = between(&sidebar, &format!(r#"data-channel-link="{dm_id}""#), ">");
    assert!(link.contains("data-unread"));

    assert!(bea.page(&dm).await.contains("Just between us"));
    let sidebar = bea.page("/home").await;
    let link = between(&sidebar, &format!(r#"data-channel-link="{dm_id}""#), ">");
    assert!(!link.contains("data-unread"), "reading clears the marker");

    assert_eq!(cat.get(&dm).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        cat.send(dm_id, "let me in", None).await,
        StatusCode::NOT_FOUND
    );
    assert!(!cat.page("/home").await.contains("Ada Admin"));
}

#[tokio::test]
async fn live_updates_reach_the_right_people() {
    let server = start().await;
    let admin = admin(&server).await;
    let bea = invite(&server, &admin, "Bea", "bea").await;
    let cat = invite(&server, &admin, "Cat", "cat").await;
    let general = home_channel(&admin).await;
    // Bea looks at #general; Cat is on another page.
    let mut bea_live = bea.live_in(general).await;
    let mut cat_live = cat.live().await;

    assert_eq!(
        admin.send(general, "Porch party at 6", None).await,
        StatusCode::NO_CONTENT
    );
    let event = bea_live
        .next_event(Duration::from_secs(5))
        .await
        .expect("event");
    assert_eq!(event["type"], "message");
    assert_eq!(event["channel_id"], general);
    assert!(event["html"].as_str().unwrap().contains("Porch party at 6"));
    // Elsewhere, a short notice marks the channel unread, once.
    let notice = cat_live
        .next_event(Duration::from_secs(5))
        .await
        .expect("notice");
    assert_eq!(notice["type"], "message");
    assert_eq!(notice["channel_id"], general);
    assert!(notice.get("html").is_none());
    admin.send(general, "Bring snacks", None).await;
    assert!(bea_live.next_event(Duration::from_secs(5)).await.is_some());
    assert!(
        cat_live
            .next_event(Duration::from_millis(300))
            .await
            .is_none()
    );
    // Mentions always come through.
    admin.send(general, "@cat you too", None).await;
    let mention = cat_live
        .next_event(Duration::from_secs(5))
        .await
        .expect("mention");
    assert_eq!(mention["activity"][0], cat.user_id().await);
    assert!(bea_live.next_event(Duration::from_secs(5)).await.is_some());

    let bea_id = bea.user_id().await;
    let dm: i64 = location(&admin.get(&format!("/dm/{bea_id}")).await)
        .trim_start_matches("/c/")
        .parse()
        .unwrap();
    let mut bea_live = bea.live_in(dm).await;
    assert_eq!(admin.send(dm, "psst", None).await, StatusCode::NO_CONTENT);
    let event = bea_live
        .next_event(Duration::from_secs(5))
        .await
        .expect("dm event");
    assert!(event["html"].as_str().unwrap().contains("psst"));
    assert!(
        cat_live
            .next_event(Duration::from_millis(300))
            .await
            .is_none()
    );

    // Thread replies carry the parent's new reply count.
    let root = event["id"].as_i64().unwrap();
    assert_eq!(
        bea.send(dm, "tell me more", Some(root)).await,
        StatusCode::NO_CONTENT
    );
    let reply = bea_live
        .next_event(Duration::from_secs(5))
        .await
        .expect("reply event");
    assert_eq!(reply["parent_id"], root);
    assert_eq!(reply["reply_count"], 1);
}

#[tokio::test]
async fn cross_site_form_posts_are_rejected() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;

    let forged = admin
        .client
        .post(admin.url(&format!("/c/{general}/messages")))
        .header("cookie", &admin.cookie)
        .header("origin", "https://evil.example")
        .form(&[("body", "forged")])
        .send()
        .await
        .unwrap();
    assert_eq!(forged.status(), StatusCode::FORBIDDEN);

    let same_site = admin
        .client
        .post(admin.url(&format!("/c/{general}/messages")))
        .header("cookie", &admin.cookie)
        .header("origin", &admin.base)
        .form(&[("body", "legit")])
        .send()
        .await
        .unwrap();
    assert_eq!(same_site.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn pages_ship_their_assets_and_security_headers() {
    let server = start().await;
    let visitor = Browser::anonymous(&server);

    let login = visitor.get("/login").await;
    let csp = login.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("script-src 'self'"));
    let html = login.text().await.unwrap();
    let css_path = format!("/assets/app.css{}", between(&html, "/assets/app.css", "\""));

    let css = visitor.get(&css_path).await;
    assert_eq!(css.headers()["content-type"], "text/css; charset=utf-8");
    let css = css.text().await.unwrap();
    assert!(css.contains("Atkinson Hyperlegible Next"));
    assert!(css.contains(".bg-floor"));
    for path in [
        "/assets/app.js",
        "/assets/logo.svg",
        "/assets/fonts/normal-latin.woff2",
        "/manifest.webmanifest",
    ] {
        assert_eq!(visitor.get(path).await.status(), StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn a_required_setup_link_is_kept_private_and_removed_after_use() {
    let data = tempfile::tempdir().unwrap();
    let app = sideporch::Sideporch::open(sideporch::Config {
        data_dir: data.path().to_owned(),
        public_url: None,
        require_setup_link: true,
        gif_api_base: None,
        allow_insecure_push: true,
        allow_private_link_previews: false,
        model_base_url: None,
        update_check: false,
        update_source: None,
        allow_private_federation: false,
        client_ip_header: None,
    })
    .await
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let file = app
        .save_setup_link(&base)
        .unwrap()
        .expect("a fresh server has a setup link");
    let link = std::fs::read_to_string(&file).unwrap();
    assert_eq!(link.trim(), format!("{base}{}", app.setup_path().unwrap()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let router = app.router();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    // Without the link there is no way in.
    let front = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(front.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(front.text().await.unwrap().contains("sideporch setup-link"));
    let open = client.get(format!("{base}/setup")).send().await.unwrap();
    assert_eq!(open.status(), StatusCode::NOT_FOUND);
    let response = client
        .post(link.trim())
        .form(&[
            ("display_name", "Ada"),
            ("username", "ada"),
            ("password", "correct horse"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(!file.exists(), "a used link is deleted");
    assert!(app.setup_is_secret() || app.setup_path().is_none());
    assert!(app.save_setup_link(&base).unwrap().is_none());
}

#[tokio::test]
async fn messages_are_github_flavored_markdown_with_diagrams() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let body = "## Plan\n- [x] **ship** it\n- [ ] ~~wait~~\n\n| a | b |\n| - | - |\n| 1 | 2 |\n\n```mermaid\ngraph LR\n  A-->B\n```";
    admin.send(general, body, None).await;
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains("<h2>Plan</h2>"), "{page}");
    assert!(page.contains("<strong>ship</strong>"));
    assert!(page.contains("<del>wait</del>"));
    assert!(page.contains("<td>2</td>"));
    assert!(page.contains("<pre class=\"mermaid\">graph LR\n  A--&gt;B</pre>"));

    // The diagram library is served compressed, only when a page asks.
    let script = admin.get("/assets/mermaid.js?v=12.0.0").await;
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(script.headers()["content-encoding"], "gzip");
}
