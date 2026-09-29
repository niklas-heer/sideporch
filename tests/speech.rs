// Tests fail by panicking, so their helpers may unwrap, index and slice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::string_slice,
    clippy::arithmetic_side_effects
)]

//! Reading aloud and dictation, and the admin page for their models.

mod common;

use std::time::Duration;

use common::{admin, home_channel, invite, last_message_id, start, start_with};
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn without_models_devices_read_aloud_and_dictation_is_off() {
    let server = start().await;
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let member = invite(&server, &admin, "Mo Member", "mo").await;
    assert_eq!(
        member.get("/admin/speech").await.status(),
        StatusCode::FORBIDDEN
    );
    let page = admin.page("/admin/speech").await;
    assert!(
        page.contains("Supertonic 3") && page.contains("Whisper base") && page.contains("OpenRAIL")
    );

    admin.send(general, "Read me", None).await;
    let channel = admin.page(&format!("/c/{general}")).await;
    assert!(channel.contains("data-voice=\"device\""));
    assert!(!channel.contains("data-dictate"));
    let message = last_message_id(&channel);
    let response = admin.get(&format!("/c/{general}/m/{message}/speech")).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let response = admin
        .client
        .post(admin.url("/speech/transcribe"))
        .header("cookie", &admin.cookie)
        .body(vec![0_u8; 10])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn downloads_that_dont_match_are_refused() {
    // A model server that hands out the wrong files.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new().fallback(|| async { "not the model you are looking for" });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let server = start_with(|config| {
        config.model_base_url = Some(base);
        config.allow_private_link_previews = true;
    })
    .await;
    let admin = admin(&server).await;
    assert_eq!(
        admin
            .post("/admin/speech/models/whisper-tiny/install", &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let mut status = Value::Null;
    for _ in 0..50 {
        status = admin
            .get("/admin/speech/status")
            .await
            .json()
            .await
            .unwrap();
        let tiny = status["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["key"] == "whisper-tiny")
            .unwrap();
        if tiny["running"] == false {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tiny = status["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["key"] == "whisper-tiny")
        .unwrap()
        .clone();
    assert_eq!(tiny["installed"], false);
    assert!(
        tiny["error"]
            .as_str()
            .unwrap()
            .contains("larger than expected")
            || tiny["error"].as_str().unwrap().contains("doesn't match"),
        "{tiny}"
    );
    assert!(
        admin
            .page("/admin/speech")
            .await
            .contains("The download failed")
    );
    // Choosing a model that isn't installed fails.
    assert_eq!(
        admin
            .post("/admin/speech", &[("dictation_model", "whisper-tiny")])
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
}

/// With real models (set `SIDEPORCH_TEST_MODELS` to a directory holding
/// `supertonic-3/` and `whisper-base/`), a message read aloud by the
/// server is written down again by dictation.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_read_aloud_can_be_dictated_back() {
    let Ok(models) = std::env::var("SIDEPORCH_TEST_MODELS") else {
        eprintln!("skipped: set SIDEPORCH_TEST_MODELS to run the speech models");
        return;
    };
    let server = start().await;
    let target = server.data_dir().join("models");
    std::fs::create_dir_all(&target).unwrap();
    for model in ["supertonic-3", "whisper-base"] {
        std::os::unix::fs::symlink(
            std::path::Path::new(&models).join(model),
            target.join(model),
        )
        .unwrap();
    }
    let admin = admin(&server).await;
    let general = home_channel(&admin).await;
    let response = admin
        .post(
            "/admin/speech",
            &[
                ("voice_model", "supertonic-3"),
                ("dictation_model", "whisper-base"),
                ("voice", "M1"),
            ],
        )
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let channel = admin.page(&format!("/c/{general}")).await;
    assert!(channel.contains("data-voice=\"server\"") && channel.contains("data-dictate"));

    admin
        .send(
            general,
            "The **garden** meeting moves to Thursday morning.",
            None,
        )
        .await;
    let message = last_message_id(&admin.page(&format!("/c/{general}")).await);
    let response = admin.get(&format!("/c/{general}/m/{message}/speech")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "audio/wav");
    let wav = response.bytes().await.unwrap().to_vec();
    assert!(wav.len() > 22_050 * 2, "at least a second of speech");

    let response = admin
        .client
        .post(admin.url("/speech/transcribe"))
        .header("cookie", &admin.cookie)
        .header("content-type", "audio/wav")
        .header("x-sideporch-fetch", "1")
        .body(wav)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let answer: Value = response.json().await.unwrap();
    let text = answer["text"].as_str().unwrap().to_lowercase();
    assert!(
        text.contains("garden") && text.contains("thursday"),
        "{text}"
    );
    assert_eq!(answer["language"], "en");
}
