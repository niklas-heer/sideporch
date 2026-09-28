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

#[tokio::test]
async fn gifs_come_from_giphy() {
    let giphy = fake_giphy().await;
    let server = start_with(|config| config.gif_api_base = Some(giphy)).await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    assert!(
        !admin
            .page(&format!("/c/{general}"))
            .await
            .contains("data-gifs")
    );
    assert_eq!(
        admin.get("/gifs?q=cat").await.status(),
        StatusCode::BAD_REQUEST
    );

    let saved = admin
        .post(
            "/admin/gifs",
            &[("api_key", "giphy-key-1234"), ("rating", "pg")],
        )
        .await;
    assert_eq!(saved.status(), StatusCode::OK);
    let settings = saved.text().await.unwrap();
    assert!(!settings.contains("giphy-key-1234") && settings.contains("…1234"));
    assert!(
        admin
            .page(&format!("/c/{general}"))
            .await
            .contains("data-gifs")
    );

    let trending: Value = admin.get("/gifs").await.json().await.unwrap();
    assert_eq!(trending["results"].as_array().unwrap().len(), 2);
    let found: Value = admin.get("/gifs?q=dance").await.json().await.unwrap();
    assert_eq!(found["results"][0]["id"], "abc123");
    assert_eq!(found["attribution"], "Powered by GIPHY");

    let sent = admin
        .client
        .post(admin.url(&format!("/c/{general}/messages")))
        .header("cookie", &admin.cookie)
        .header("x-sideporch-fetch", "1")
        .form(&[("gif", "abc123")])
        .send()
        .await
        .unwrap();
    assert_eq!(sent.status(), StatusCode::NO_CONTENT);
    let page = admin.page(&format!("/c/{general}")).await;
    assert!(
        page.contains(r#"src="https://media.giphy.com/media/abc123/giphy.gif""#),
        "{page}"
    );
    assert!(page.contains("via GIPHY"));
    let bad = admin
        .client
        .post(admin.url(&format!("/c/{general}/messages")))
        .header("cookie", &admin.cookie)
        .form(&[("gif", "../../etc")])
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
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
