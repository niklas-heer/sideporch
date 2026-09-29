// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Profiles, the full emoji picker, files on disk, GIFs and the admin's
//! system page, driven through a real server.

mod common;

use axum::{Json, Router, extract::Path, routing::get};
use common::{PNG, admin, between, home_channel, invite, start, start_with};
use reqwest::{StatusCode, multipart};
use serde_json::{Value, json};

fn profile_form(fields: &[(&str, &str)]) -> multipart::Form {
    let mut form = multipart::Form::new();
    for (name, value) in fields {
        form = form.text((*name).to_owned(), (*value).to_owned());
    }
    form
}

#[tokio::test]
async fn people_have_profiles_with_pictures() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let form = profile_form(&[
        ("display_name", "Ada Lovelace"),
        ("status_emoji", ":coffee:"),
        ("status_text", "Brewing ideas"),
        ("bio", "I write **programs** for engines."),
        ("link_label", "Notes"),
        ("link_url", "https://example.com/notes"),
        ("link_label", ""),
        ("link_url", ""),
        ("favorite_emoji", "cactus tada"),
    ])
    .part(
        "avatar",
        multipart::Part::bytes(PNG.to_vec()).file_name("me.png"),
    );
    let saved = admin
        .client
        .post(admin.url("/settings/profile"))
        .header("cookie", &admin.cookie)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let me = admin.user_id().await;
    let profile = admin.page(&format!("/people/{me}")).await;
    assert!(profile.contains("Ada Lovelace") && profile.contains("Brewing ideas"));
    assert!(profile.contains("<strong>programs</strong>"));
    assert!(profile.contains(r#"href="https://example.com/notes""#) && profile.contains(">Notes<"));
    let avatar = between(&profile, r#"<img src="/files/"#, "\"").to_owned();

    // Messages show the picture, the status emoji and a link to the profile.
    admin.send(general, "Hello", None).await;
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains(&format!(r#"src="/files/{avatar}""#)));
    assert!(page.contains(&format!(r#"href="/people/{me}""#)));
    assert!(page.contains("☕"));
    // Favorites lead the reaction picker.
    let yours = between(&page, "Your emoji", "</section>");
    assert!(
        yours.find("data-emoji=\"cactus\"").unwrap() < yours.find("data-emoji=\"tada\"").unwrap()
    );

    // Others can see the picture too.
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let picture = member.get(&format!("/files/{avatar}")).await;
    assert_eq!(picture.status(), StatusCode::OK);
    assert_eq!(picture.bytes().await.unwrap().as_ref(), PNG);

    let bad = admin
        .client
        .post(admin.url("/settings/profile"))
        .header("cookie", &admin.cookie)
        .multipart(profile_form(&[
            ("display_name", "Ada"),
            ("status_emoji", "not-an-emoji"),
        ]))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn every_emoji_can_react_and_files_live_on_disk() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let catalog: Value = admin.get("/assets/emoji.json").await.json().await.unwrap();
    let categories = catalog["categories"].as_array().unwrap();
    assert_eq!(categories.len(), 9);
    assert_eq!(categories[0]["name"], "Smileys");
    let total: usize = categories
        .iter()
        .map(|c| c["emoji"].as_array().unwrap().len())
        .sum();
    assert!(total > 1_800, "{total}");

    admin.send(general, "Look :cactus: :de:", None).await;
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(
        page.contains("🌵") && page.contains("🇩🇪"),
        "shortcodes from the full set"
    );
    let message = common::last_message_id(&page);
    let reacted = admin
        .post(
            &format!("/c/{general}/m/{message}/reactions"),
            &[("emoji", "cactus")],
        )
        .await;
    assert_eq!(reacted.status(), StatusCode::SEE_OTHER);
    assert!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .contains("reacted with :cactus:")
    );
    let react_page = admin.page(&format!("/c/{general}/m/{message}/react")).await;
    assert!(react_page.contains("Travel") && react_page.contains("value=\"airplane\""));

    // Uploads are stored in files/ in the data directory, by content hash.
    let uploaded = admin
        .upload(general, "", &[("notes.txt", b"porch notes")])
        .await;
    assert_eq!(uploaded.status(), StatusCode::NO_CONTENT);
    let files = server.data_dir().join("files");
    let stored: Vec<_> = walk(&files);
    assert!(
        stored
            .iter()
            .any(|path| std::fs::read(path).unwrap() == b"porch notes"),
        "{stored:?}"
    );
    let page = admin.page(&format!("/c/{general}")).await;
    let file = between(&page, "/files/", "\"").to_owned();
    let download = admin.get(&format!("/files/{file}")).await;
    assert_eq!(download.bytes().await.unwrap().as_ref(), b"porch notes");
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
        } else {
            found.push(path);
        }
    }
    found
}

/// A fake GIPHY with one GIF.
async fn fake_giphy() -> String {
    let gif = |id: &str| {
        json!({
            "id": id,
            "title": "Porch dance",
            "images": {
                "fixed_width": { "url": format!("https://media.giphy.com/media/{id}/200w.gif"), "width": "200", "height": "150" },
                "downsized_medium": { "url": format!("https://media.giphy.com/media/{id}/giphy.gif"), "width": "480", "height": "360" }
            }
        })
    };
    let app = Router::new()
        .route(
            "/v1/gifs/search",
            get(move || async move { Json(json!({ "data": [gif("abc123")] })) }),
        )
        .route(
            "/v1/gifs/trending",
            get(move || async move { Json(json!({ "data": [gif("trend1"), gif("trend2")] })) }),
        )
        .route(
            "/v1/gifs/{id}",
            get(move |Path(id): Path<String>| async move { Json(json!({ "data": gif(&id) })) }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

/// Posts a GIF from the picker, as app.js does.
async fn send_gif(browser: &common::Browser, channel: i64, form: &[(&str, &str)]) -> StatusCode {
    browser
        .client
        .post(browser.url(&format!("/c/{channel}/messages")))
        .header("cookie", &browser.cookie)
        .header("x-sideporch-fetch", "1")
        .form(form)
        .send()
        .await
        .unwrap()
        .status()
}

/// A GIF header saying 320 by 240, which is all the library reads.
const GIF: &[u8] = b"GIF89a\x40\x01\xf0\x00\x00\x00\x00;";

#[tokio::test]
async fn gifs_come_from_the_teams_library_by_default() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains(r#"data-provider="local""#) && page.contains("data-gif-button"));
    let empty: Value = admin.get("/gifs").await.json().await.unwrap();
    assert_eq!(empty["results"], json!([]));

    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let add = |title: &'static str, data: &'static [u8]| {
        let form = multipart::Form::new()
            .text("title", title)
            .text("tags", "#Party, dance")
            .part("file", multipart::Part::bytes(data).file_name("porch.gif"));
        member
            .client
            .post(member.url("/gifs/library"))
            .header("cookie", &member.cookie)
            .multipart(form)
            .send()
    };
    assert_eq!(
        add("Porch dance", GIF).await.unwrap().status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        add("Not a GIF", b"plain text").await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let library = member.page("/gifs/library").await;
    assert!(library.contains("Porch dance") && library.contains("party dance"));

    let found: Value = admin.get("/gifs?q=PARTY").await.json().await.unwrap();
    let gif = &found["results"][0];
    assert_eq!(gif["title"], "Porch dance");
    assert_eq!(
        (gif["width"].as_u64(), gif["height"].as_u64()),
        (Some(320), Some(240))
    );
    let preview = gif["preview"].as_str().unwrap().to_owned();
    assert_eq!(
        admin.get(&preview).await.bytes().await.unwrap().as_ref(),
        GIF
    );
    let none: Value = admin.get("/gifs?q=nothing").await.json().await.unwrap();
    assert_eq!(none["results"], json!([]));

    let id = gif["id"].as_str().unwrap().to_owned();
    assert_eq!(
        send_gif(&admin, general, &[("gif", &id)]).await,
        StatusCode::NO_CONTENT
    );
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains(&format!(r#"src="{preview}""#)), "{page}");
    assert!(!page.contains("via GIPHY"));
    assert_eq!(
        send_gif(&admin, general, &[("gif", "999")]).await,
        StatusCode::BAD_REQUEST
    );
    assert!(admin.page("/gifs/library").await.contains("1 use"));

    // Only the person who added a GIF, or an admin, can remove it.
    let other = invite(&server, &admin, "Ola Other", "ola").await;
    assert_eq!(
        other
            .post(&format!("/gifs/library/{id}/delete"), &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        member
            .post(&format!("/gifs/library/{id}/delete"), &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(admin.get(&preview).await.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn gifs_can_come_from_giphy() {
    let giphy = fake_giphy().await;
    let server = start_with(|config| config.gif_api_base = Some(giphy)).await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let missing_key = admin
        .post("/admin/gifs", &[("provider", "giphy"), ("rating", "pg")])
        .await;
    assert_eq!(missing_key.status(), StatusCode::BAD_REQUEST);

    let saved = admin
        .post(
            "/admin/gifs",
            &[
                ("provider", "giphy"),
                ("giphy_key", "giphy-key-1234"),
                ("rating", "pg"),
            ],
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let settings = saved.text().await.unwrap();
    assert!(!settings.contains("giphy-key-1234") && settings.contains("…1234"));
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(page.contains(r#"data-provider="giphy""#) && page.contains("Powered by GIPHY"));

    let trending: Value = admin.get("/gifs").await.json().await.unwrap();
    assert_eq!(trending["results"].as_array().unwrap().len(), 2);
    let found: Value = admin.get("/gifs?q=dance").await.json().await.unwrap();
    assert_eq!(found["results"][0]["id"], "abc123");
    assert_eq!(found["attribution"], "Powered by GIPHY");

    assert_eq!(
        send_gif(&admin, general, &[("gif", "abc123")]).await,
        StatusCode::NO_CONTENT
    );
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(
        page.contains(r#"src="https://media.giphy.com/media/abc123/giphy.gif""#),
        "{page}"
    );
    assert!(page.contains("via GIPHY"));
    assert_eq!(
        send_gif(&admin, general, &[("gif", "../../etc")]).await,
        StatusCode::BAD_REQUEST
    );

    // Switching back keeps the key for later.
    admin
        .post("/admin/gifs", &[("provider", "local"), ("rating", "pg")])
        .await;
    assert!(admin.page("/admin/gifs").await.contains("…1234"));
    assert!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .contains(r#"data-provider="local""#)
    );
}

#[tokio::test]
async fn klipy_is_searched_from_the_browser() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let saved = admin
        .post(
            "/admin/gifs",
            &[
                ("provider", "klipy"),
                ("klipy_key", "klipy-key-5678"),
                ("rating", "g"),
            ],
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    let page = member.page(&format!("/c/{general}")).await;
    assert!(page.contains(r#"data-provider="klipy""#));
    assert!(
        page.contains(r#"data-klipy-key="klipy-key-5678""#)
            && page.contains(r#"data-klipy-filter="high""#)
    );
    assert!(page.contains("Powered by KLIPY"));
    let csp = member.get("/").await.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("connect-src 'self' https://api.klipy.com"));
    assert_eq!(
        member.get("/gifs?q=cat").await.status(),
        StatusCode::BAD_REQUEST
    );

    let gif = |url: &'static str| {
        [
            ("gif", "4242"),
            ("gif_url", url),
            ("gif_title", "Porch wave"),
            ("gif_width", "480"),
            ("gif_height", "270"),
        ]
    };
    assert_eq!(
        send_gif(
            &member,
            general,
            &gif("https://static.klipy.com/ii/wave.gif")
        )
        .await,
        StatusCode::NO_CONTENT
    );
    let page = member.page(&format!("/c/{general}")).await;
    assert!(
        page.contains(r#"src="https://static.klipy.com/ii/wave.gif""#)
            && page.contains("via KLIPY")
    );
    for url in [
        "https://evil.example/wave.gif",
        "https://klipy.com.evil.example/wave.gif",
        "http://static.klipy.com/wave.gif",
        "javascript:alert(1)",
    ] {
        assert_eq!(
            send_gif(&member, general, &gif(url)).await,
            StatusCode::BAD_REQUEST,
            "{url}"
        );
    }
}

#[tokio::test]
async fn admins_see_system_resources() {
    let server = start().await;
    let admin = admin(&server).await;
    let page = admin.page("/admin/system").await;
    assert!(page.contains("Storage") && page.contains("Database") && page.contains("Version"));
    assert!(page.contains(env!("CARGO_PKG_VERSION")));
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    assert_eq!(
        member.get("/admin/system").await.status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn badges_hover_cards_and_leveling_up_show_who_is_who() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let mo = invite(&server, &admin, "Mo Member", "mo").await;
    let mo_id = mo.user_id().await;

    // A role shown as a badge, in a color.
    admin.post("/admin/roles", &[("name", "Moderators")]).await;
    let permissions = admin.page("/admin/permissions").await;
    let role_id = between(&permissions, "/admin/roles/", "/badge").to_owned();
    admin
        .post(
            &format!("/admin/roles/{role_id}/badge"),
            &[("badge", "on"), ("color", "green")],
        )
        .await;
    let role_field = format!("role_{role_id}");
    admin
        .post(
            &format!("/people/{mo_id}/roles"),
            &[(role_field.as_str(), "on")],
        )
        .await;

    mo.send(general, "Hi from a moderator", None).await;
    admin.send(general, "Hi from an admin", None).await;
    let channel = mo.page(&format!("/c/{general}")).await;
    let mo_line = between(
        &channel,
        &format!(r#"data-person-card="{mo_id}" class"#),
        "</div>",
    );
    assert!(
        mo_line.contains("Moderators") && mo_line.contains("bg-emerald-100"),
        "{mo_line}"
    );
    let admin_id = admin.user_id().await;
    let admin_line = between(
        &channel,
        &format!(r#"data-person-card="{admin_id}" class"#),
        "</div>",
    );
    assert!(admin_line.contains(">Admin<"), "{admin_line}");

    // The hover card.
    let card = admin.page(&format!("/people/{mo_id}/card")).await;
    assert!(card.contains("Mo Member") && card.contains("@mo"));
    assert!(card.contains("Moderators") && card.contains("Local time"));
    assert!(card.contains("Level ") && card.contains(&format!("/dm/{mo_id}")));

    // Their own profile says what the next level takes.
    let profile = mo.page(&format!("/people/{mo_id}")).await;
    assert!(between(&profile, "data-next-level", "</p>").contains("Level 2"));

    // Reaching a level is announced once.
    assert!(!mo.page("/home").await.contains("data-level-up"));
    admin
        .post(&format!("/people/{mo_id}/trust"), &[("level", "2")])
        .await;
    let home = mo.page("/home").await;
    assert!(between(&home, "data-level-up", "</form>").contains("You reached level 2"));
    assert_eq!(
        mo.post("/settings/level-noticed", &[]).await.status(),
        StatusCode::NO_CONTENT
    );
    assert!(!mo.page("/home").await.contains("data-level-up"));
}
