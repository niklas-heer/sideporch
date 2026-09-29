//! 16-bit mono WAV in and out, and changing sample rates.

use crate::error::{AppError, AppResult};

/// Encodes samples in -1..1 as a 16-bit mono WAV file.
pub fn wav(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = u32::try_from(samples.len().saturating_mul(2)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(samples.len().saturating_mul(2).saturating_add(44));
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&data_len.saturating_add(36).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16_u32.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&1_u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&rate.saturating_mul(2).to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        let scaled = (sample.clamp(-1.0, 1.0) * 32767.0).round();
        // In range after clamping.
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            reason = "clamped to i16's range"
        )]
        out.extend_from_slice(&(scaled as i16).to_le_bytes());
    }
    out
}

/// Reads a 16-bit PCM WAV file, mixing channels down to mono. Returns the
/// samples and their rate.
pub fn read_wav(bytes: &[u8]) -> AppResult<(Vec<f32>, u32)> {
    let bad = || AppError::bad_request("The recording isn't a WAV file Sideporch can read.");
    if bytes.get(..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err(bad());
    }
    let mut offset = 12_usize;
    let mut format: Option<(u16, u16, u32, u16)> = None;
    while let Some(header) = bytes.get(offset..offset.saturating_add(8)) {
        let (id, size) = header.split_at(4);
        let size = usize::try_from(u32::from_le_bytes(size.try_into().map_err(|_| bad())?))
            .map_err(|_| bad())?;
        let body_start = offset.saturating_add(8);
        let body = bytes
            .get(body_start..body_start.saturating_add(size).min(bytes.len()))
            .ok_or_else(bad)?;
        match id {
            b"fmt " => {
                let word = |at: usize| -> Option<u16> {
                    Some(u16::from_le_bytes([
                        *body.get(at)?,
                        *body.get(at.saturating_add(1))?,
                    ]))
                };
                let rate = body
                    .get(4..8)
                    .and_then(|rate| rate.try_into().ok())
                    .map(u32::from_le_bytes)
                    .ok_or_else(bad)?;
                format = Some((
                    word(0).ok_or_else(bad)?,
                    word(2).ok_or_else(bad)?,
                    rate,
                    word(14).ok_or_else(bad)?,
                ));
            }
            b"data" => {
                let (kind, channels, rate, bits) = format.ok_or_else(bad)?;
                if kind != 1 || bits != 16 || channels == 0 || rate == 0 {
                    return Err(bad());
                }
                let channels = usize::from(channels);
                let samples = body
                    .chunks_exact(2)
                    .filter_map(|pair| match pair {
                        [low, high] => Some(f32::from(i16::from_le_bytes([*low, *high])) / 32768.0),
                        _ => None,
                    })
                    .collect::<Vec<f32>>()
                    .chunks(channels)
                    .map(|frame| {
                        frame.iter().sum::<f32>()
                            / f32::from(u16::try_from(frame.len()).unwrap_or(1))
                    })
                    .collect();
                return Ok((samples, rate));
            }
            _ => {}
        }
        // Chunks are padded to an even size.
        offset = body_start.saturating_add(size).saturating_add(size & 1);
    }
    Err(bad())
}

/// Changes the sample rate by linear interpolation, after averaging over
/// each output sample's span when shrinking, which keeps aliasing low for
/// speech.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() || from == 0 || to == 0 {
        return input.to_vec();
    }
    let ratio = f64::from(from) / f64::from(to);
    let count = (usize_to_f64(input.len()) / ratio).floor();
    let count = f64_to_usize(count);
    let last = input.len().saturating_sub(1);
    (0..count)
        .map(|index| {
            let position = usize_to_f64(index) * ratio;
            if ratio > 1.0 {
                let start = f64_to_usize(position.floor()).min(last);
                let end = f64_to_usize((position + ratio).ceil())
                    .clamp(start.saturating_add(1), input.len());
                let span = input.get(start..end).unwrap_or_default();
                span.iter().sum::<f32>() / f64_to_f32(usize_to_f64(span.len().max(1)))
            } else {
                let base = f64_to_usize(position.floor()).min(last);
                let next = base.saturating_add(1).min(last);
                let fraction = f64_to_f32(position - position.floor());
                let a = input.get(base).copied().unwrap_or(0.0);
                let b = input.get(next).copied().unwrap_or(a);
                (b - a).mul_add(fraction, a)
            }
        })
        .collect()
}

#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "sample counts fit f64 exactly"
)]
const fn usize_to_f64(value: usize) -> f64 {
    value as f64
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "callers pass non-negative sample positions"
)]
const fn f64_to_usize(value: f64) -> usize {
    value as usize
}

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    reason = "audio values"
)]
const fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_round_trips() {
        let samples = [0.0, 0.5, -0.5, 1.0, -1.0];
        let (read, rate) = read_wav(&wav(&samples, 16_000)).unwrap();
        assert_eq!(rate, 16_000);
        for (a, b) in samples.iter().zip(&read) {
            assert!((a - b).abs() < 0.001, "{a} {b}");
        }
        assert!(read_wav(b"not a wav").is_err());
    }

    #[test]
    fn resampling_keeps_length_and_level() {
        let tone: Vec<f32> = (0..44_100)
            .map(|i| (f32::from(u16::try_from(i % 100).unwrap()) / 100.0) - 0.5)
            .collect();
        let down = resample(&tone, 44_100, 16_000);
        assert_eq!(down.len(), 16_000);
        let up = resample(&[0.0, 1.0], 1, 4);
        assert_eq!(up, vec![0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0]);
        let mean = down.iter().sum::<f32>() / 16_000.0;
        assert!(mean.abs() < 0.02, "{mean}");
    }
}
