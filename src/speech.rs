//! Reading messages aloud and dictating them, with models that run on the
//! server's CPU, in Rust. Admins download the models they want into the
//! data directory; until then, reading aloud falls back to the voices of
//! each person's own device, and dictation stays off.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use rusqlite::Connection;
use sha2::{Digest as _, Sha256};

use crate::{
    error::{AppError, AppResult},
    store,
};

pub mod audio;
pub mod models;
pub mod stt;
pub mod text;
pub mod tts;

use models::{Kind, Model};

/// Models are unloaded after this long unused, to give memory back.
const IDLE: Duration = Duration::from_secs(15 * 60);
/// Spoken messages kept on disk, at most.
const CACHE_BYTES: u64 = 256 * 1024 * 1024;
/// Reading aloud is sent at this rate; plenty for speech, half the size.
const OUTPUT_RATE: u32 = 22_050;
/// The longest text read aloud in one go.
pub const MAX_SPEAK_CHARS: usize = 4000;
/// The longest recording dictated, in seconds.
pub const MAX_DICTATION_SECONDS: u32 = 120;

/// Which models are chosen, and each person's voice.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    pub voice_model: Option<&'static Model>,
    pub dictation_model: Option<&'static Model>,
    /// The voice people hear unless they chose another.
    pub voice: String,
}

impl Settings {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        let model = |key: &str| -> AppResult<Option<&'static Model>> {
            Ok(store::setting(conn, key)?.and_then(|key| models::find(&key)))
        };
        Ok(Self {
            voice_model: model("speech.voice_model")?,
            dictation_model: model("speech.dictation_model")?,
            voice: store::setting(conn, "speech.voice")?.unwrap_or_else(|| "F1".to_owned()),
        })
    }

    pub fn save(&self, conn: &Connection) -> AppResult<()> {
        store::set_setting(
            conn,
            "speech.voice_model",
            self.voice_model.map_or("", |model| model.key),
        )?;
        store::set_setting(
            conn,
            "speech.dictation_model",
            self.dictation_model.map_or("", |model| model.key),
        )?;
        store::set_setting(conn, "speech.voice", &self.voice)
    }
}

/// How a download is going.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub received: u64,
    pub error: Option<String>,
    pub running: bool,
}

struct Download {
    received: Arc<AtomicU64>,
    error: Option<String>,
    running: bool,
}

#[derive(Default)]
struct Loaded {
    voice: Option<(&'static str, Arc<tts::Engine>)>,
    dictation: Option<(&'static str, Arc<stt::Engine>)>,
    last_used: Option<Instant>,
}

/// The server's speech models.
pub struct Speech {
    data_dir: PathBuf,
    /// Where models are downloaded from; tests point it at a local server.
    base: String,
    loaded: Mutex<Loaded>,
    /// Speech work is heavy; one job at a time keeps the chat responsive.
    work: tokio::sync::Semaphore,
    downloads: Mutex<HashMap<&'static str, Download>>,
}

impl Speech {
    pub fn new(data_dir: &Path, base: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            data_dir: data_dir.to_owned(),
            base: base.unwrap_or_else(|| "https://huggingface.co".to_owned()),
            loaded: Mutex::new(Loaded::default()),
            work: tokio::sync::Semaphore::new(1),
            downloads: Mutex::new(HashMap::new()),
        })
    }

    pub fn installed(&self, model: &Model) -> bool {
        model.installed(&self.data_dir)
    }

    pub fn progress(&self, model: &Model) -> Option<Progress> {
        let downloads = self.downloads.lock().ok()?;
        downloads.get(model.key).map(|download| Progress {
            received: download.received.load(Ordering::Relaxed),
            error: download.error.clone(),
            running: download.running,
        })
    }

    /// Downloads a model in the background. `done` runs once it is in place.
    pub fn install(
        self: &Arc<Self>,
        model: &'static Model,
        http: Arc<crate::automations::http::Http>,
        done: impl FnOnce() + Send + 'static,
    ) -> AppResult<()> {
        let received = Arc::new(AtomicU64::new(0));
        {
            let mut downloads = self
                .downloads
                .lock()
                .map_err(|_| AppError::internal("downloads lock poisoned"))?;
            if downloads
                .get(model.key)
                .is_some_and(|download| download.running)
            {
                return Ok(());
            }
            downloads.insert(
                model.key,
                Download {
                    received: Arc::clone(&received),
                    error: None,
                    running: true,
                },
            );
        }
        let speech = Arc::clone(self);
        tokio::spawn(async move {
            let dir = model.dir(&speech.data_dir);
            let mut result = Ok(());
            for (path, size, sha256) in model.files {
                let target = dir.join(path);
                if std::fs::metadata(&target).is_ok_and(|meta| meta.len() == *size) {
                    received.fetch_add(*size, Ordering::Relaxed);
                    continue;
                }
                let url = model.url(&speech.base, path);
                if let Err(error) = http.download(&url, &target, *size, sha256, &received).await {
                    result = Err(format!("{path}: {error}"));
                    break;
                }
            }
            if let Err(error) = &result {
                tracing::warn!(model = model.key, %error, "model download failed");
            } else {
                tracing::info!(model = model.key, "model downloaded");
            }
            if let Ok(mut downloads) = speech.downloads.lock()
                && let Some(download) = downloads.get_mut(model.key)
            {
                download.running = false;
                download.error = result.as_ref().err().cloned();
            }
            if result.is_ok() {
                done();
            }
        });
        Ok(())
    }

    /// Deletes a model's files and forgets it if loaded.
    pub fn remove(&self, model: &Model) -> AppResult<()> {
        if let Ok(mut loaded) = self.loaded.lock() {
            if loaded
                .voice
                .as_ref()
                .is_some_and(|(key, _)| *key == model.key)
            {
                loaded.voice = None;
            }
            if loaded
                .dictation
                .as_ref()
                .is_some_and(|(key, _)| *key == model.key)
            {
                loaded.dictation = None;
            }
        }
        if let Ok(mut downloads) = self.downloads.lock() {
            downloads.remove(model.key);
        }
        match std::fs::remove_dir_all(model.dir(&self.data_dir)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(AppError::internal(error))
            }
            _ => Ok(()),
        }
    }

    fn voice_engine(&self, model: &'static Model) -> AppResult<Arc<tts::Engine>> {
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.last_used = Some(Instant::now());
            if let Some((key, engine)) = &loaded.voice
                && *key == model.key
            {
                return Ok(Arc::clone(engine));
            }
        }
        let started = Instant::now();
        let engine = Arc::new(tts::Engine::load(&model.dir(&self.data_dir))?);
        tracing::info!(model = model.key, elapsed = ?started.elapsed(), "loaded the voice model");
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.voice = Some((model.key, Arc::clone(&engine)));
        }
        Ok(engine)
    }

    fn dictation_engine(&self, model: &'static Model) -> AppResult<Arc<stt::Engine>> {
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.last_used = Some(Instant::now());
            if let Some((key, engine)) = &loaded.dictation
                && *key == model.key
            {
                return Ok(Arc::clone(engine));
            }
        }
        let started = Instant::now();
        let engine = Arc::new(stt::Engine::load(&model.dir(&self.data_dir), model.width)?);
        tracing::info!(model = model.key, elapsed = ?started.elapsed(), "loaded the dictation model");
        if let Ok(mut loaded) = self.loaded.lock() {
            loaded.dictation = Some((model.key, Arc::clone(&engine)));
        }
        Ok(engine)
    }

    fn cache_path(&self, key: &str) -> PathBuf {
        let hash = Sha256::digest(key.as_bytes())
            .iter()
            .fold(String::new(), |mut hex, byte| {
                use std::fmt::Write as _;
                // Writing to a String cannot fail.
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        self.data_dir
            .join("cache")
            .join("speech")
            .join(format!("{hash}.wav"))
    }

    /// Reads text aloud as a WAV file, from the cache when it was read
    /// before with the same voice.
    pub async fn speak(
        self: &Arc<Self>,
        model: &'static Model,
        text: String,
        voice: String,
        language: Option<String>,
        speed: f32,
    ) -> AppResult<Vec<u8>> {
        if !self.installed(model) {
            return Err(AppError::bad_request("The voice model isn't installed."));
        }
        let text: String = text.chars().take(MAX_SPEAK_CHARS).collect();
        if text.trim().is_empty() {
            return Err(AppError::bad_request("There's nothing to read."));
        }
        let language = language
            .or_else(|| tts::detect_language(&text).map(ToOwned::to_owned))
            .unwrap_or_else(|| "en".to_owned());
        let cache = self.cache_path(&format!("{}|{voice}|{language}|{speed}|{text}", model.key));
        if let Ok(bytes) = tokio::fs::read(&cache).await {
            return Ok(bytes);
        }
        let _permit = tokio::time::timeout(Duration::from_secs(120), self.work.acquire())
            .await
            .map_err(|_| AppError::bad_request("Reading aloud is busy. Try again in a moment."))?
            .map_err(AppError::internal)?;
        let speech = Arc::clone(self);
        let bytes = tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
            let engine = speech.voice_engine(model)?;
            let samples = engine.speak(&text, &language, &voice, speed)?;
            let samples = audio::resample(&samples, engine.sample_rate(), OUTPUT_RATE);
            Ok(audio::wav(&samples, OUTPUT_RATE))
        })
        .await
        .map_err(AppError::internal)??;
        Self::store_cached(&cache, &bytes);
        Ok(bytes)
    }

    fn store_cached(path: &Path, bytes: &[u8]) {
        let Some(dir) = path.parent() else { return };
        if std::fs::create_dir_all(dir).is_err() || std::fs::write(path, bytes).is_err() {
            return;
        }
        // Keep the cache within its size by dropping the oldest files.
        let mut files: Vec<(std::time::SystemTime, u64, PathBuf)> = std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let meta = entry.metadata().ok()?;
                        Some((meta.modified().ok()?, meta.len(), entry.path()))
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        let mut total: u64 = files.iter().map(|(_, size, _)| size).sum();
        for (_, size, file) in files {
            if total <= CACHE_BYTES {
                break;
            }
            if std::fs::remove_file(file).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    /// Transcribes a WAV recording.
    pub async fn transcribe(
        self: &Arc<Self>,
        model: &'static Model,
        wav: Vec<u8>,
        language: Option<String>,
    ) -> AppResult<(String, String)> {
        if !self.installed(model) {
            return Err(AppError::bad_request(
                "The dictation model isn't installed.",
            ));
        }
        let (samples, rate) = audio::read_wav(&wav)?;
        let seconds = u32::try_from(samples.len())
            .unwrap_or(u32::MAX)
            .checked_div(rate)
            .unwrap_or(u32::MAX);
        if seconds > MAX_DICTATION_SECONDS {
            return Err(AppError::bad_request(
                "Dictate at most two minutes at a time.",
            ));
        }
        let _permit = tokio::time::timeout(Duration::from_secs(120), self.work.acquire())
            .await
            .map_err(|_| AppError::bad_request("Dictation is busy. Try again in a moment."))?
            .map_err(AppError::internal)?;
        let speech = Arc::clone(self);
        tokio::task::spawn_blocking(move || {
            let engine = speech.dictation_engine(model)?;
            let samples = audio::resample(&samples, rate, stt::SAMPLE_RATE);
            engine.transcribe(&samples, language.as_deref())
        })
        .await
        .map_err(AppError::internal)?
    }

    /// Unloads models nobody used for a while.
    pub fn start_unloading(self: &Arc<Self>) {
        let speech = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let Some(speech) = speech.upgrade() else {
                    break;
                };
                if let Ok(mut loaded) = speech.loaded.lock()
                    && loaded.last_used.is_some_and(|used| used.elapsed() > IDLE)
                    && (loaded.voice.is_some() || loaded.dictation.is_some())
                {
                    loaded.voice = None;
                    loaded.dictation = None;
                    tracing::info!("unloaded idle speech models");
                }
            }
        });
    }

    /// Bytes the models take on disk.
    pub fn disk_usage(&self) -> u64 {
        models::MODELS
            .iter()
            .filter(|model| self.installed(model))
            .map(Model::size)
            .sum()
    }
}

/// Models of one kind.
pub fn of_kind(kind: Kind) -> impl Iterator<Item = &'static Model> {
    models::MODELS
        .iter()
        .filter(move |model| model.kind == kind)
}
