//! Reading aloud with Supertonic 3: four ONNX graphs run with tract. Text
//! goes in as Unicode characters (no phonemizer), a duration predictor says
//! how long it takes, a flow-matching estimator turns noise into speech
//! latents in a few steps, and a vocoder makes the waveform.

use std::path::Path;

use serde::Deserialize;
use tract_onnx::prelude::*;

use crate::error::{AppError, AppResult};

/// Languages the model speaks, by ISO 639-1 code.
pub const LANGUAGES: &[&str] = &[
    "en", "ko", "ja", "ar", "bg", "cs", "da", "de", "el", "es", "et", "fi", "fr", "hi", "hr", "hu",
    "id", "it", "lt", "lv", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "tr", "uk", "vi",
];

/// The voices that come with the model.
pub const VOICES: &[(&str, &str)] = &[
    ("F1", "Voice F1 (female)"),
    ("F2", "Voice F2 (female)"),
    ("F3", "Voice F3 (female)"),
    ("F4", "Voice F4 (female)"),
    ("F5", "Voice F5 (female)"),
    ("M1", "Voice M1 (male)"),
    ("M2", "Voice M2 (male)"),
    ("M3", "Voice M3 (male)"),
    ("M4", "Voice M4 (male)"),
    ("M5", "Voice M5 (male)"),
];

/// Flow-matching steps: more is smoother and slower.
const STEPS: usize = 5;
/// Characters per chunk; longer text is read in chunks.
const CHUNK_CHARS: usize = 300;
/// Silence between chunks, in seconds.
const PAUSE: f32 = 0.3;

#[derive(Deserialize)]
struct Config {
    ae: AeConfig,
    ttl: TtlConfig,
}

#[derive(Deserialize)]
struct AeConfig {
    sample_rate: u32,
    base_chunk_size: usize,
}

#[derive(Deserialize)]
struct TtlConfig {
    chunk_compress_factor: usize,
    latent_dim: usize,
}

#[derive(Deserialize)]
struct StyleComponent {
    data: Vec<Vec<Vec<f32>>>,
    dims: Vec<usize>,
}

#[derive(Deserialize)]
struct VoiceFile {
    style_ttl: StyleComponent,
    style_dp: StyleComponent,
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(format!("reading aloud failed: {error}"))
}

fn tensor3(component: &StyleComponent) -> AppResult<Tensor> {
    let [a, b, c] = component.dims.as_slice() else {
        return Err(internal("a voice file has the wrong shape"));
    };
    let flat: Vec<f32> = component.data.iter().flatten().flatten().copied().collect();
    Ok(tract_ndarray::Array3::from_shape_vec((*a, *b, *c), flat)
        .map_err(internal)?
        .into())
}

/// A loaded model. Graphs are kept parsed; each call fixes their input
/// shapes and optimizes them, since tract can't prove some of the model's
/// symbolic reshapes.
pub struct Engine {
    config: Config,
    indexer: Vec<i64>,
    duration: InferenceModel,
    encoder: InferenceModel,
    estimator: InferenceModel,
    vocoder: InferenceModel,
    voices: std::collections::HashMap<String, (Tensor, Tensor)>,
}

fn parse(path: &Path) -> AppResult<InferenceModel> {
    tract_onnx::onnx()
        .with_ignore_value_info(true)
        .with_ignore_output_shapes(true)
        .model_for_path(path)
        .map_err(internal)
}

/// Fixes every input's shape to the tensors it is about to get: `text_ids`
/// holds integers, everything else floats.
fn plan(
    model: &InferenceModel,
    inputs: &[(&str, &Tensor)],
) -> AppResult<std::sync::Arc<TypedSimplePlan>> {
    let mut model = model.clone();
    let names: Vec<String> = model
        .input_outlets()
        .map_err(internal)?
        .iter()
        .map(|outlet| model.node(outlet.node).name.clone())
        .collect();
    for (index, name) in names.iter().enumerate() {
        let (_, tensor) = inputs
            .iter()
            .find(|(input, _)| input == name)
            .ok_or_else(|| internal(format!("the model wants an input {name}")))?;
        let fact: InferenceFact = if name == "text_ids" {
            i64::fact(tensor.shape()).into()
        } else {
            f32::fact(tensor.shape()).into()
        };
        model.set_input_fact(index, fact).map_err(internal)?;
    }
    let typed = model.into_typed().map_err(internal)?;
    // The estimator trips a tract optimization; it runs unoptimized.
    typed
        .clone()
        .into_optimized()
        .map_or_else(|_| typed.into_runnable(), TypedModel::into_runnable)
        .map_err(internal)
}

/// Runs a plan with named inputs, in the order the model declares them.
fn run(model: &InferenceModel, inputs: &[(&str, &Tensor)]) -> AppResult<TVec<TValue>> {
    let plan = plan(model, inputs)?;
    let names: Vec<String> = model
        .input_outlets()
        .map_err(internal)?
        .iter()
        .map(|outlet| model.node(outlet.node).name.clone())
        .collect();
    let values: TVec<TValue> = names
        .iter()
        .filter_map(|name| inputs.iter().find(|(input, _)| input == name))
        .map(|(_, tensor)| (*tensor).clone().into())
        .collect();
    plan.run(values).map_err(internal)
}

impl Engine {
    pub fn load(dir: &Path) -> AppResult<Self> {
        let read = |name: &str| std::fs::read_to_string(dir.join(name)).map_err(internal);
        let config: Config = serde_json::from_str(&read("onnx/tts.json")?).map_err(internal)?;
        let indexer: Vec<i64> =
            serde_json::from_str(&read("onnx/unicode_indexer.json")?).map_err(internal)?;
        let mut voices = std::collections::HashMap::new();
        for (key, _) in VOICES {
            let voice: VoiceFile =
                serde_json::from_str(&read(&format!("voice_styles/{key}.json"))?)
                    .map_err(internal)?;
            voices.insert(
                (*key).to_owned(),
                (tensor3(&voice.style_ttl)?, tensor3(&voice.style_dp)?),
            );
        }
        Ok(Self {
            config,
            indexer,
            duration: parse(&dir.join("onnx/duration_predictor.onnx"))?,
            encoder: parse(&dir.join("onnx/text_encoder.onnx"))?,
            estimator: parse(&dir.join("onnx/vector_estimator.onnx"))?,
            vocoder: parse(&dir.join("onnx/vocoder.onnx"))?,
            voices,
        })
    }

    pub const fn sample_rate(&self) -> u32 {
        self.config.ae.sample_rate
    }

    /// Reads `text` in `language` with `voice`; `speed` 1 is normal.
    pub fn speak(
        &self,
        text: &str,
        language: &str,
        voice: &str,
        speed: f32,
    ) -> AppResult<Vec<f32>> {
        let (style_ttl, style_dp) = self
            .voices
            .get(voice)
            .or_else(|| self.voices.get("F1"))
            .ok_or_else(|| internal("no voices"))?;
        let language = if LANGUAGES.contains(&language) {
            language
        } else {
            "en"
        };
        let mut audio = Vec::new();
        let pause = vec![0.0_f32; seconds_to_samples(PAUSE, self.sample_rate())];
        for chunk in chunks(
            text,
            if matches!(language, "ko" | "ja") {
                120
            } else {
                CHUNK_CHARS
            },
        ) {
            let wave = self.chunk(&chunk, language, style_ttl, style_dp, speed)?;
            if !audio.is_empty() {
                audio.extend_from_slice(&pause);
            }
            audio.extend(wave);
        }
        Ok(audio)
    }

    /// Turns noise into speech latents in a few flow-matching steps.
    fn denoise(
        &self,
        mut xt: Tensor,
        text_emb: &Tensor,
        style_ttl: &Tensor,
        latent_mask: &Tensor,
        text_mask: &Tensor,
    ) -> AppResult<Tensor> {
        let total = tensor1(&[step_value(STEPS)]);
        let first = tensor1(&[0.0_f32]);
        let estimator = plan(
            &self.estimator,
            &[
                ("noisy_latent", &xt),
                ("text_emb", text_emb),
                ("style_ttl", style_ttl),
                ("latent_mask", latent_mask),
                ("text_mask", text_mask),
                ("current_step", &first),
                ("total_step", &total),
            ],
        )?;
        let order: Vec<String> = self
            .estimator
            .input_outlets()
            .map_err(internal)?
            .iter()
            .map(|outlet| self.estimator.node(outlet.node).name.clone())
            .collect();
        for step in 0..STEPS {
            let current = tensor1(&[step_value(step)]);
            let named: [(&str, &Tensor); 7] = [
                ("noisy_latent", &xt),
                ("text_emb", text_emb),
                ("style_ttl", style_ttl),
                ("latent_mask", latent_mask),
                ("text_mask", text_mask),
                ("current_step", &current),
                ("total_step", &total),
            ];
            let values: TVec<TValue> = order
                .iter()
                .filter_map(|name| named.iter().find(|(input, _)| input == name))
                .map(|(_, tensor)| (*tensor).clone().into())
                .collect();
            let out = estimator.run(values).map_err(internal)?;
            xt = out
                .into_iter()
                .next()
                .ok_or_else(|| internal("no latent"))?
                .into_tensor();
        }
        Ok(xt)
    }

    fn chunk(
        &self,
        text: &str,
        language: &str,
        style_ttl: &Tensor,
        style_dp: &Tensor,
        speed: f32,
    ) -> AppResult<Vec<f32>> {
        let prepared = format!("<{language}>{}</{language}>", prepare(text));
        let ids: Vec<i64> = prepared
            .chars()
            .map(|c| {
                usize::try_from(u32::from(c))
                    .ok()
                    .and_then(|index| self.indexer.get(index).copied())
                    .unwrap_or(-1)
            })
            .collect();
        let t = ids.len();
        let text_ids: Tensor = tract_ndarray::Array2::from_shape_vec((1, t), ids)
            .map_err(internal)?
            .into();
        let text_mask: Tensor = tract_ndarray::Array3::<f32>::ones((1, 1, t)).into();

        let out = run(
            &self.duration,
            &[
                ("text_ids", &text_ids),
                ("style_dp", style_dp),
                ("text_mask", &text_mask),
            ],
        )?;
        let seconds = out
            .first()
            .and_then(|value| {
                value
                    .to_plain_array_view::<f32>()
                    .ok()?
                    .iter()
                    .next()
                    .copied()
            })
            .ok_or_else(|| internal("no duration"))?
            / speed.clamp(0.5, 2.0);
        let out = run(
            &self.encoder,
            &[
                ("text_ids", &text_ids),
                ("style_ttl", style_ttl),
                ("text_mask", &text_mask),
            ],
        )?;
        let text_emb = out
            .into_iter()
            .next()
            .ok_or_else(|| internal("no text embedding"))?
            .into_tensor();

        let samples = seconds_to_samples(seconds, self.sample_rate());
        let chunk = self
            .config
            .ae
            .base_chunk_size
            .saturating_mul(self.config.ttl.chunk_compress_factor)
            .max(1);
        let frames = samples.div_ceil(chunk).max(1);
        let latent = self
            .config
            .ttl
            .latent_dim
            .saturating_mul(self.config.ttl.chunk_compress_factor);
        let xt: Tensor = tract_ndarray::Array3::from_shape_vec(
            (1, latent, frames),
            noise(latent.saturating_mul(frames)),
        )
        .map_err(internal)?
        .into();
        let latent_mask: Tensor = tract_ndarray::Array3::<f32>::ones((1, 1, frames)).into();
        let xt = self.denoise(xt, &text_emb, style_ttl, &latent_mask, &text_mask)?;
        let out = run(&self.vocoder, &[("latent", &xt)])?;
        let wave = out
            .first()
            .ok_or_else(|| internal("no audio"))?
            .to_plain_array_view::<f32>()
            .map_err(internal)?
            .iter()
            .copied()
            .take(samples)
            .collect();
        Ok(wave)
    }
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "a handful of steps"
)]
const fn step_value(step: usize) -> f32 {
    step as f32
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "durations are short and positive"
)]
fn seconds_to_samples(seconds: f32, rate: u32) -> usize {
    (f64::from(seconds.max(0.0)) * f64::from(rate)) as usize
}

/// Standard normal noise to start the flow from.
fn noise(count: usize) -> Vec<f32> {
    let mut seed = [0_u8; 8];
    getrandom::fill(&mut seed).unwrap_or_default();
    let mut state = u64::from_le_bytes(seed) | 1;
    let mut uniform = move || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let bits = state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
        #[expect(
            clippy::as_conversions,
            clippy::cast_precision_loss,
            reason = "53 random bits"
        )]
        let value = bits as f64 / 9_007_199_254_740_992.0;
        value.max(1e-12)
    };
    (0..count)
        .map(|_| {
            let (a, b) = (uniform(), uniform());
            let normal = (-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos();
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "noise"
            )]
            let normal = normal as f32;
            normal
        })
        .collect()
}

/// Cleans text the way the model was trained: normalized, without emoji,
/// with plain quotes and dashes, ending in punctuation.
pub fn prepare(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        let replaced = match c {
            '–' | '‑' | '—' => "-",
            '_' | '[' | ']' | '|' | '/' | '#' | '→' | '←' => " ",
            '\u{201C}' | '\u{201D}' => "\"",
            '\u{2018}' | '\u{2019}' | '´' | '`' => "'",
            '@' => " at ",
            '♥' | '☆' | '♡' | '©' | '\\' => "",
            c if is_emoji(c) => "",
            _ => {
                out.push(c);
                continue;
            }
        };
        out.push_str(replaced);
    }
    let mut out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    for punctuation in [",", ".", "!", "?", ";", ":"] {
        out = out.replace(&format!(" {punctuation}"), punctuation);
    }
    if !out.is_empty() && !out.ends_with(['.', '!', '?', ';', ':', ',', '\'', '"', ')', '…', '。'])
    {
        out.push('.');
    }
    out
}

fn is_emoji(c: char) -> bool {
    matches!(u32::from(c), 0x1F300..=0x1FAFF | 0x2600..=0x27BF | 0x1F1E6..=0x1F1FF | 0xFE0F)
}

/// Splits text into chunks of whole sentences.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut sentence = String::new();
    let flush = |sentence: &mut String, current: &mut String, chunks: &mut Vec<String>| {
        let piece = sentence.trim().to_owned();
        sentence.clear();
        if piece.is_empty() {
            return;
        }
        if !current.is_empty()
            && current
                .chars()
                .count()
                .saturating_add(piece.chars().count())
                >= max
        {
            chunks.push(std::mem::take(current).trim().to_owned());
        }
        // A sentence longer than a chunk is split at spaces.
        if piece.chars().count() > max {
            for word in piece.split_whitespace() {
                if current.chars().count().saturating_add(word.chars().count()) >= max
                    && !current.is_empty()
                {
                    chunks.push(std::mem::take(current).trim().to_owned());
                }
                current.push_str(word);
                current.push(' ');
            }
        } else {
            current.push_str(&piece);
            current.push(' ');
        }
    };
    let mut characters = text.chars().peekable();
    while let Some(c) = characters.next() {
        sentence.push(c);
        if matches!(c, '.' | '!' | '?' | '\n')
            && characters.peek().is_none_or(|next| next.is_whitespace())
        {
            flush(&mut sentence, &mut current, &mut chunks);
        }
    }
    flush(&mut sentence, &mut current, &mut chunks);
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_owned());
    }
    chunks
}

/// The model's code for a text's language, guessed from the text; `None`
/// when unsure or unsupported.
pub fn detect_language(text: &str) -> Option<&'static str> {
    let info = whatlang::detect(text)?;
    // Short texts are guessed poorly; the default language reads them.
    if text.chars().filter(|c| c.is_alphabetic()).count() < 12 || info.confidence() < 0.5 {
        return None;
    }
    let code = match info.lang() {
        whatlang::Lang::Eng => "en",
        whatlang::Lang::Kor => "ko",
        whatlang::Lang::Jpn => "ja",
        whatlang::Lang::Ara => "ar",
        whatlang::Lang::Bul => "bg",
        whatlang::Lang::Ces => "cs",
        whatlang::Lang::Dan => "da",
        whatlang::Lang::Deu => "de",
        whatlang::Lang::Ell => "el",
        whatlang::Lang::Spa => "es",
        whatlang::Lang::Est => "et",
        whatlang::Lang::Fin => "fi",
        whatlang::Lang::Fra => "fr",
        whatlang::Lang::Hin => "hi",
        whatlang::Lang::Hrv => "hr",
        whatlang::Lang::Hun => "hu",
        whatlang::Lang::Ind => "id",
        whatlang::Lang::Ita => "it",
        whatlang::Lang::Lit => "lt",
        whatlang::Lang::Lav => "lv",
        whatlang::Lang::Nld => "nl",
        whatlang::Lang::Pol => "pl",
        whatlang::Lang::Por => "pt",
        whatlang::Lang::Ron => "ro",
        whatlang::Lang::Rus => "ru",
        whatlang::Lang::Slk => "sk",
        whatlang::Lang::Slv => "sl",
        whatlang::Lang::Swe => "sv",
        whatlang::Lang::Tur => "tr",
        whatlang::Lang::Ukr => "uk",
        whatlang::Lang::Vie => "vi",
        _ => return None,
    };
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepares_text_like_the_model_expects() {
        assert_eq!(
            prepare("Hi @ada — see “this” 🎉"),
            "Hi at ada - see \"this\""
        );
        assert_eq!(prepare("no end"), "no end.");
        assert_eq!(prepare("Done , thanks !"), "Done, thanks!");
        assert_eq!(prepare(""), "");
    }

    #[test]
    fn splits_into_sentence_chunks() {
        let text = "One two. Three four five. Six!";
        assert_eq!(chunks(text, 300), vec!["One two. Three four five. Six!"]);
        assert_eq!(
            chunks(text, 17),
            vec!["One two.", "Three four five.", "Six!"]
        );
        let long = "word ".repeat(100);
        assert!(
            chunks(&long, 50)
                .iter()
                .all(|chunk| chunk.chars().count() <= 50)
        );
    }

    #[test]
    fn detects_languages() {
        assert_eq!(
            detect_language("Guten Morgen, wir treffen uns um zehn Uhr auf der Veranda."),
            Some("de")
        );
        assert_eq!(
            detect_language("The deploy finished and everything looks good."),
            Some("en")
        );
        assert_eq!(detect_language("ok"), None);
    }
}
