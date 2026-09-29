//! Dictation with Whisper: a log-mel spectrogram of 30 seconds of 16 kHz
//! audio, the encoder, and greedy decoding with the decoder, both run with
//! tract. Longer recordings are transcribed 30 seconds at a time.

use std::{collections::HashMap, path::Path, sync::Arc};

use tract_onnx::prelude::*;

use crate::error::{AppError, AppResult};

pub const SAMPLE_RATE: u32 = 16_000;
const N_FFT: usize = 400;
const HOP: usize = 160;
const N_MELS: usize = 80;
const BINS: usize = N_FFT / 2 + 1;
const WINDOW_SAMPLES: usize = 480_000;
const FRAMES: usize = 3000;
/// Tokens per 30-second window, as Whisper was trained.
const MAX_TOKENS: usize = 224;

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(format!("dictation failed: {error}"))
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "small indexes"
)]
const fn float(value: usize) -> f64 {
    value as f64
}

fn hz_to_mel(hz: f64) -> f64 {
    let logstep = 6.4_f64.ln() / 27.0;
    if hz >= 1000.0 {
        15.0 + (hz / 1000.0).ln() / logstep
    } else {
        hz * 3.0 / 200.0
    }
}

fn mel_to_hz(mel: f64) -> f64 {
    let logstep = 6.4_f64.ln() / 27.0;
    if mel >= 15.0 {
        1000.0 * (logstep * (mel - 15.0)).exp()
    } else {
        mel * 200.0 / 3.0
    }
}

/// Slaney-style mel filters, like librosa's that Whisper was trained with.
fn mel_filters() -> Vec<Vec<f32>> {
    let frequencies: Vec<f64> = (0..BINS)
        .map(|bin| float(bin) * f64::from(SAMPLE_RATE) / float(N_FFT))
        .collect();
    let (low, high) = (hz_to_mel(0.0), hz_to_mel(f64::from(SAMPLE_RATE) / 2.0));
    let points: Vec<f64> = (0..N_MELS + 2)
        .map(|index| mel_to_hz(low + (high - low) * float(index) / float(N_MELS + 1)))
        .collect();
    points
        .windows(3)
        .filter_map(|edges| match edges {
            [left, center, right] => Some((*left, *center, *right)),
            _ => None,
        })
        .map(|(left, center, right)| {
            let norm = 2.0 / (right - left);
            frequencies
                .iter()
                .map(|&frequency| {
                    let rising = (frequency - left) / (center - left);
                    let falling = (right - frequency) / (right - center);
                    #[expect(
                        clippy::as_conversions,
                        clippy::cast_possible_truncation,
                        reason = "filter weights"
                    )]
                    let weight = (rising.min(falling).max(0.0) * norm) as f32;
                    weight
                })
                .collect()
        })
        .collect()
}

/// Whisper's input: 80 log-mel bands over 3000 frames of 30 seconds.
pub fn log_mel(audio: &[f32], filters: &[Vec<f32>]) -> Vec<f32> {
    let mut samples = audio.to_vec();
    samples.resize(WINDOW_SAMPLES, 0.0);
    let pad = N_FFT / 2;
    // Reflect padding, like torch.stft(center = true).
    let mut padded: Vec<f32> = (1..=pad)
        .rev()
        .map(|i| samples.get(i).copied().unwrap_or(0.0))
        .collect();
    padded.extend_from_slice(&samples);
    padded.extend((0..pad).map(|i| {
        samples
            .get(WINDOW_SAMPLES.saturating_sub(2).saturating_sub(i))
            .copied()
            .unwrap_or(0.0)
    }));
    let window: Vec<f32> = (0..N_FFT)
        .map(|i| {
            #[expect(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                reason = "window weights"
            )]
            let value = (-0.5_f64)
                .mul_add((std::f64::consts::TAU * float(i) / float(N_FFT)).cos(), 0.5)
                as f32;
            value
        })
        .collect();
    let fft = rustfft::FftPlanner::<f32>::new().plan_fft_forward(N_FFT);
    let mut buffer = vec![rustfft::num_complex::Complex32::default(); N_FFT];
    let mut mel = vec![0_f32; N_MELS * FRAMES];
    for frame in 0..FRAMES {
        let start = frame.saturating_mul(HOP);
        for (index, slot) in buffer.iter_mut().enumerate() {
            let sample = padded
                .get(start.saturating_add(index))
                .copied()
                .unwrap_or(0.0);
            *slot = rustfft::num_complex::Complex32::new(
                sample * window.get(index).copied().unwrap_or(0.0),
                0.0,
            );
        }
        fft.process(&mut buffer);
        let power: Vec<f32> = buffer
            .iter()
            .take(BINS)
            .map(rustfft::num_complex::Complex32::norm_sqr)
            .collect();
        for (band, filter) in filters.iter().enumerate() {
            let energy: f32 = filter
                .iter()
                .zip(&power)
                .map(|(weight, value)| weight * value)
                .sum();
            if let Some(slot) = mel.get_mut(band.saturating_mul(FRAMES).saturating_add(frame)) {
                *slot = energy.max(1e-10).log10();
            }
        }
    }
    let peak = mel.iter().copied().fold(f32::MIN, f32::max);
    mel.iter()
        .map(|value| (value.max(peak - 8.0) + 4.0) / 4.0)
        .collect()
}

/// The byte each character of GPT-2's byte-level vocabulary stands for.
fn byte_decoder() -> HashMap<char, u8> {
    let mut bytes: Vec<u8> = (b'!'..=b'~')
        .chain(0xA1..=0xAC)
        .chain(0xAE..=0xFF)
        .collect();
    let mut chars: Vec<u32> = bytes.iter().map(|byte| u32::from(*byte)).collect();
    let mut extra = 0_u32;
    for byte in 0..=u8::MAX {
        if !bytes.contains(&byte) {
            bytes.push(byte);
            chars.push(extra.saturating_add(256));
            extra = extra.saturating_add(1);
        }
    }
    bytes
        .into_iter()
        .zip(chars)
        .filter_map(|(byte, c)| Some((char::from_u32(c)?, byte)))
        .collect()
}

pub struct Engine {
    encoder: Arc<TypedSimplePlan>,
    decoder: Arc<TypedSimplePlan>,
    filters: Vec<Vec<f32>>,
    tokens: Vec<String>,
    bytes: HashMap<char, u8>,
    end: i64,
    start: i64,
    transcribe: i64,
    no_timestamps: i64,
    languages: Vec<(String, i64)>,
}

/// Loads a graph; `facts` names input shapes, made with the graph's own
/// symbols.
fn load_plan(
    path: &Path,
    facts: impl FnOnce(&SymbolScope) -> Vec<(&'static str, InferenceFact)>,
) -> AppResult<Arc<TypedSimplePlan>> {
    let mut model = tract_onnx::onnx()
        .with_ignore_value_info(true)
        .model_for_path(path)
        .map_err(internal)?;
    let facts = facts(&model.symbols);
    let names: Vec<String> = model
        .input_outlets()
        .map_err(internal)?
        .iter()
        .map(|outlet| model.node(outlet.node).name.clone())
        .collect();
    for (index, name) in names.iter().enumerate() {
        if let Some((_, fact)) = facts.iter().find(|(wanted, _)| wanted == name) {
            model
                .set_input_fact(index, fact.clone())
                .map_err(internal)?;
        }
    }
    model
        .into_optimized()
        .and_then(TypedModel::into_runnable)
        .map_err(internal)
}

impl Engine {
    pub fn load(dir: &Path, width: usize) -> AppResult<Self> {
        let encoder = load_plan(&dir.join("onnx/encoder_model.onnx"), |_| {
            vec![("input_features", f32::fact([1, N_MELS, FRAMES]).into())]
        })?;
        let decoder = load_plan(&dir.join("onnx/decoder_model.onnx"), |symbols| {
            let length = symbols.sym("tokens");
            vec![
                (
                    "input_ids",
                    i64::fact([TDim::from(1), length.into()]).into(),
                ),
                ("encoder_hidden_states", f32::fact([1, 1500, width]).into()),
            ]
        })?;
        let read = |name: &str| -> AppResult<HashMap<String, i64>> {
            serde_json::from_str(&std::fs::read_to_string(dir.join(name)).map_err(internal)?)
                .map_err(internal)
        };
        let mut vocabulary = read("vocab.json")?;
        vocabulary.extend(read("added_tokens.json")?);
        let size = vocabulary
            .values()
            .copied()
            .max()
            .and_then(|max| usize::try_from(max).ok())
            .unwrap_or(0)
            .saturating_add(1);
        let mut tokens = vec![String::new(); size];
        for (text, id) in &vocabulary {
            if let Some(slot) = usize::try_from(*id).ok().and_then(|id| tokens.get_mut(id)) {
                text.clone_into(slot);
            }
        }
        let id = |name: &str| {
            vocabulary
                .get(name)
                .copied()
                .ok_or_else(|| internal(format!("no {name} token")))
        };
        let start = id("<|startoftranscript|>")?;
        let translate = id("<|translate|>")?;
        let languages = vocabulary
            .iter()
            .filter(|(text, id)| **id > start && **id < translate && text.len() <= 9)
            .map(|(text, id)| {
                (
                    text.trim_start_matches("<|")
                        .trim_end_matches("|>")
                        .to_owned(),
                    *id,
                )
            })
            .collect();
        Ok(Self {
            encoder,
            decoder,
            filters: mel_filters(),
            tokens,
            bytes: byte_decoder(),
            end: id("<|endoftext|>")?,
            start,
            transcribe: id("<|transcribe|>")?,
            no_timestamps: id("<|notimestamps|>")?,
            languages,
        })
    }

    /// The last position's scores after `tokens`.
    fn next_scores(&self, hidden: &Tensor, tokens: &[i64]) -> AppResult<Vec<f32>> {
        let ids: Tensor = tract_ndarray::Array2::from_shape_vec((1, tokens.len()), tokens.to_vec())
            .map_err(internal)?
            .into();
        let out = self
            .decoder
            .run(tvec!(ids.into(), hidden.clone().into()))
            .map_err(internal)?;
        let logits = out.first().ok_or_else(|| internal("no scores"))?;
        let view = logits.to_plain_array_view::<f32>().map_err(internal)?;
        let vocabulary = view.shape().get(2).copied().unwrap_or(0);
        let all = view
            .as_slice()
            .ok_or_else(|| internal("scores aren't contiguous"))?;
        let from = tokens.len().saturating_sub(1).saturating_mul(vocabulary);
        Ok(all
            .get(from..from.saturating_add(vocabulary))
            .unwrap_or_default()
            .to_vec())
    }

    /// Transcribes 16 kHz mono audio. `language` is a code like `de`, or
    /// `None` to detect it. Returns the text and the language used.
    pub fn transcribe(&self, audio: &[f32], language: Option<&str>) -> AppResult<(String, String)> {
        let mut text = String::new();
        let mut chosen = language.map(ToOwned::to_owned);
        for window in audio.chunks(WINDOW_SAMPLES) {
            // Skip trailing near-silence.
            if window.iter().all(|sample| sample.abs() < 0.001) {
                continue;
            }
            let features: Tensor = tract_ndarray::Array3::from_shape_vec(
                (1, N_MELS, FRAMES),
                log_mel(window, &self.filters),
            )
            .map_err(internal)?
            .into();
            let hidden = self
                .encoder
                .run(tvec!(features.into()))
                .map_err(internal)?
                .into_iter()
                .next()
                .ok_or_else(|| internal("no encoding"))?
                .into_tensor();
            let known = chosen
                .as_deref()
                .and_then(|code| self.languages.iter().find(|(name, _)| name == code))
                .map(|(_, id)| *id);
            let language_id = if let Some(id) = known {
                id
            } else {
                let scores = self.next_scores(&hidden, &[self.start])?;
                let score = |id: i64| {
                    usize::try_from(id)
                        .ok()
                        .and_then(|id| scores.get(id))
                        .copied()
                        .unwrap_or(f32::MIN)
                };
                let (name, id) = self
                    .languages
                    .iter()
                    .max_by(|a, b| score(a.1).total_cmp(&score(b.1)))
                    .ok_or_else(|| internal("no languages"))?;
                chosen = Some(name.clone());
                *id
            };
            let mut tokens = vec![self.start, language_id, self.transcribe, self.no_timestamps];
            let prompt = tokens.len();
            for _ in 0..MAX_TOKENS {
                let scores = self.next_scores(&hidden, &tokens)?;
                // Only text tokens or the end; no timestamps or specials.
                let limit = usize::try_from(self.end).unwrap_or(0);
                let next = scores
                    .iter()
                    .take(limit.saturating_add(1))
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .and_then(|(id, _)| i64::try_from(id).ok())
                    .unwrap_or(self.end);
                // A token repeated many times means the model is stuck.
                let stuck = tokens.len() > prompt.saturating_add(8)
                    && tokens.iter().rev().take(8).all(|token| *token == next);
                if next == self.end || stuck {
                    break;
                }
                tokens.push(next);
            }
            let bytes: Vec<u8> = tokens
                .iter()
                .skip(prompt)
                .filter_map(|id| usize::try_from(*id).ok().and_then(|id| self.tokens.get(id)))
                .flat_map(|token| token.chars().filter_map(|c| self.bytes.get(&c).copied()))
                .collect();
            let piece = String::from_utf8_lossy(&bytes).trim().to_owned();
            if !piece.is_empty() {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&piece);
            }
        }
        Ok((text, chosen.unwrap_or_else(|| "en".to_owned())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mel_filters_cover_rising_bands() {
        let filters = mel_filters();
        assert_eq!(filters.len(), 80);
        assert_eq!(filters[0].len(), 201);
        // The first band peaks at its center bin; higher bands sit higher.
        assert!(filters[0][1] > filters[0][0] && filters[0][1] > filters[0][3]);
        let peak = |filter: &Vec<f32>| {
            filter
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .unwrap()
                .0
        };
        assert!(
            filters
                .windows(2)
                .all(|pair| peak(&pair[0]) <= peak(&pair[1]))
        );
        // Each filter covers a band.
        assert!(
            filters
                .iter()
                .all(|filter| filter.iter().any(|weight| *weight > 0.0))
        );
    }

    #[test]
    fn byte_level_tokens_decode() {
        let bytes = byte_decoder();
        assert_eq!(bytes.len(), 256);
        assert_eq!(bytes[&'Ġ'], b' ');
        assert_eq!(bytes[&'A'], b'A');
    }

    #[test]
    fn silence_has_a_flat_spectrogram() {
        let mel = log_mel(&vec![0.0; 16_000], &mel_filters());
        assert_eq!(mel.len(), N_MELS * FRAMES);
        assert!(mel.iter().all(|value| (value - mel[0]).abs() < 1e-6));
    }
}
