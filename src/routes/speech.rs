//! Reading messages aloud, dictating them, and the admin page for the
//! speech models.

use axum::{
    Form, Json, Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use maud::Markup;
use serde::Deserialize;
use serde_json::json;

use super::shell_data;
use crate::{
    AppState,
    auth::CurrentUser,
    error::{AppError, AppResult},
    speech::{self, Settings, models},
    store,
    views::{Shell, speech as pages},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/speech", get(admin_page).post(save))
        .route("/admin/speech/status", get(status))
        .route("/admin/speech/models/{key}/install", post(install))
        .route("/admin/speech/models/{key}/remove", post(remove))
        .route("/admin/speech/try", get(try_voice))
        .route("/c/{channel_id}/m/{message_id}/speech", get(read_aloud))
        .route(
            "/speech/transcribe",
            post(transcribe).layer(DefaultBodyLimit::max(8 * 1024 * 1024)),
        )
        .route("/settings/speech", post(save_personal))
}

const fn require_admin(user: &CurrentUser) -> AppResult<()> {
    if user.is_admin {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

async fn settings(state: &AppState) -> AppResult<Settings> {
    state.db.call(|conn| Settings::load(conn)).await
}

async fn admin_page(user: CurrentUser, State(state): State<AppState>) -> AppResult<Markup> {
    require_admin(&user)?;
    let settings = settings(&state).await?;
    let sidebar = shell_data(&state, user.id).await?;
    let shell = Shell {
        user: &user,
        sidebar: &sidebar,
        current: None,
    };
    Ok(pages::admin_page(&shell, &state.speech, &settings))
}

/// Download progress, for the admin page while it waits.
async fn status(
    user: CurrentUser,
    State(state): State<AppState>,
) -> AppResult<Json<serde_json::Value>> {
    require_admin(&user)?;
    let models: Vec<serde_json::Value> = models::MODELS
        .iter()
        .map(|model| {
            let progress = state.speech.progress(model).unwrap_or_default();
            json!({
                "key": model.key,
                "installed": state.speech.installed(model),
                "running": progress.running,
                "received": progress.received,
                "total": model.size(),
                "error": progress.error,
            })
        })
        .collect();
    Ok(Json(json!({ "models": models })))
}

fn model(key: &str) -> AppResult<&'static models::Model> {
    models::find(key).ok_or(AppError::NotFound)
}

async fn install(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let model = model(&key)?;
    let db = state.db.clone();
    state
        .speech
        .install(model, std::sync::Arc::clone(&state.links), move || {
            // The first model of its kind is used right away.
            tokio::spawn(async move {
                let chosen = db
                    .call(move |conn| {
                        let mut settings = Settings::load(conn)?;
                        match model.kind {
                            models::Kind::Voice if settings.voice_model.is_none() => {
                                settings.voice_model = Some(model);
                            }
                            models::Kind::Dictation if settings.dictation_model.is_none() => {
                                settings.dictation_model = Some(model);
                            }
                            _ => return Ok(()),
                        }
                        settings.save(conn)
                    })
                    .await;
                if let Err(error) = chosen {
                    tracing::warn!(?error, "could not choose the downloaded model");
                }
            });
        })?;
    Ok(Redirect::to("/admin/speech"))
}

async fn remove(
    user: CurrentUser,
    State(state): State<AppState>,
    Path(key): Path<String>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let model = model(&key)?;
    state.speech.remove(model)?;
    state
        .db
        .call(move |conn| {
            let mut settings = Settings::load(conn)?;
            if settings
                .voice_model
                .is_some_and(|chosen| chosen.key == model.key)
            {
                settings.voice_model = None;
            }
            if settings
                .dictation_model
                .is_some_and(|chosen| chosen.key == model.key)
            {
                settings.dictation_model = None;
            }
            settings.save(conn)
        })
        .await?;
    Ok(Redirect::to("/admin/speech"))
}

#[derive(Deserialize)]
struct SettingsForm {
    #[serde(default)]
    voice_model: String,
    #[serde(default)]
    dictation_model: String,
    #[serde(default)]
    voice: String,
}

async fn save(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<SettingsForm>,
) -> AppResult<Redirect> {
    require_admin(&user)?;
    let chosen = |key: &str, kind: models::Kind| -> AppResult<Option<&'static models::Model>> {
        if key.is_empty() {
            return Ok(None);
        }
        let model = model(key)?;
        if model.kind != kind || !state.speech.installed(model) {
            return Err(AppError::bad_request("Install that model first."));
        }
        Ok(Some(model))
    };
    let settings = Settings {
        voice_model: chosen(&form.voice_model, models::Kind::Voice)?,
        dictation_model: chosen(&form.dictation_model, models::Kind::Dictation)?,
        voice: if speech::tts::VOICES
            .iter()
            .any(|(key, _)| *key == form.voice)
        {
            form.voice
        } else {
            "F1".to_owned()
        },
    };
    state.db.call(move |conn| settings.save(conn)).await?;
    Ok(Redirect::to("/admin/speech"))
}

fn wav(bytes: Vec<u8>) -> Response {
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static("audio/wav"));
    // The same message and voice always sound the same.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=3600"),
    );
    response
}

/// The voice and speed someone hears.
async fn personal(state: &AppState, user_id: i64) -> AppResult<(String, f32)> {
    state
        .db
        .call(move |conn| {
            let settings = Settings::load(conn)?;
            let (voice, speed): (String, f64) = conn.query_row(
                "SELECT speech_voice, speech_speed FROM users WHERE id = ?1",
                [user_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let voice = if voice.is_empty() {
                settings.voice
            } else {
                voice
            };
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "a speed near 1"
            )]
            let speed = speed.clamp(0.5, 2.0) as f32;
            Ok((voice, speed))
        })
        .await
}

#[derive(Deserialize)]
struct TryQuery {
    #[serde(default)]
    text: String,
    #[serde(default)]
    voice: String,
}

/// Lets an admin hear a voice before choosing it.
async fn try_voice(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<TryQuery>,
) -> AppResult<Response> {
    require_admin(&user)?;
    let settings = settings(&state).await?;
    let model = settings
        .voice_model
        .ok_or_else(|| AppError::bad_request("Install a voice model first."))?;
    let voice = if query.voice.is_empty() {
        settings.voice
    } else {
        query.voice
    };
    let text = if query.text.trim().is_empty() {
        "Hello! This is how messages sound when Sideporch reads them aloud.".to_owned()
    } else {
        query.text
    };
    Ok(wav(state
        .speech
        .speak(model, text, voice, None, 1.0)
        .await?))
}

/// A message, read aloud by the server's voice.
async fn read_aloud(
    user: CurrentUser,
    State(state): State<AppState>,
    Path((channel_id, message_id)): Path<(i64, i64)>,
) -> AppResult<Response> {
    let settings = settings(&state).await?;
    let model = settings
        .voice_model
        .ok_or_else(|| AppError::bad_request("This Sideporch has no voice installed."))?;
    let user_id = user.id;
    let message = state
        .db
        .call(move |conn| store::readable_message(conn, user_id, channel_id, message_id))
        .await?
        .filter(|message| !message.deleted)
        .ok_or(AppError::NotFound)?;
    let mut text = speech::text::speakable(&message.body);
    if let Some(poll) = &message.poll {
        let options: Vec<&str> = poll
            .options
            .iter()
            .map(|option| option.label.as_str())
            .collect();
        text.push_str(" The options are: ");
        text.push_str(&options.join(", "));
        text.push('.');
    }
    let (voice, speed) = personal(&state, user.id).await?;
    Ok(wav(state
        .speech
        .speak(model, text, voice, None, speed)
        .await?))
}

#[derive(Deserialize, Default)]
struct TranscribeQuery {
    /// A language code like `de`, or empty to detect it.
    #[serde(default)]
    language: String,
}

/// Turns a WAV recording from the composer into text.
async fn transcribe(
    user: CurrentUser,
    State(state): State<AppState>,
    Query(query): Query<TranscribeQuery>,
    body: Bytes,
) -> AppResult<Json<serde_json::Value>> {
    crate::community::check_not_timed_out(&user)?;
    let settings = settings(&state).await?;
    let model = settings
        .dictation_model
        .ok_or_else(|| AppError::bad_request("This Sideporch has no dictation model installed."))?;
    let language = Some(query.language)
        .filter(|language| language.len() == 2 && language.chars().all(|c| c.is_ascii_lowercase()));
    let (text, language) = state
        .speech
        .transcribe(model, body.to_vec(), language)
        .await?;
    Ok(Json(json!({ "text": text, "language": language })))
}

#[derive(Deserialize)]
struct PersonalForm {
    #[serde(default)]
    voice: String,
    #[serde(default)]
    speed: f64,
}

/// Someone's own voice and speed.
async fn save_personal(
    user: CurrentUser,
    State(state): State<AppState>,
    Form(form): Form<PersonalForm>,
) -> AppResult<Redirect> {
    let voice = if speech::tts::VOICES
        .iter()
        .any(|(key, _)| *key == form.voice)
    {
        form.voice
    } else {
        String::new()
    };
    let speed = if form.speed.is_finite() && form.speed > 0.0 {
        form.speed.clamp(0.5, 2.0)
    } else {
        1.0
    };
    let user_id = user.id;
    state
        .db
        .call(move |conn| {
            conn.execute(
                "UPDATE users SET speech_voice = ?1, speech_speed = ?2 WHERE id = ?3",
                rusqlite::params![voice, speed, user_id],
            )?;
            Ok(())
        })
        .await?;
    Ok(Redirect::to("/settings/appearance?saved=1"))
}
