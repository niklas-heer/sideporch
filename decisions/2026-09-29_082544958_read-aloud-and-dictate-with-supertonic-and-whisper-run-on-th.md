+++
schema_version = 1
id = "01M3P4A4QYKGT5AVR5FK4ZEJ1N"
title = "Read aloud and dictate with Supertonic and Whisper, run on the server with tract"
date = "2026-09-29"
status = "accepted"
tags = ["speech", "dependencies"]
supersedes = []
superseded_by = []
depends_on = []
related_to = []
+++
## Decision

Sideporch reads messages aloud and takes dictation on the server, with open models run by [tract](https://github.com/sonos/tract), Sonos's pure-Rust ONNX runtime:

- **Reading aloud** uses Supertonic 3 (Supertone, 99M parameters, OpenRAIL-M): characters in, no phonemizer, 31 languages, ten voices, 44.1 kHz out (sent as 22.05 kHz WAV). The language is guessed per message with `whatlang`. Measured on an Apple M-series CPU in a release build: 5 s of speech in 0.9 s.
- **Dictation** uses Whisper tiny, base or small (MIT) as exported by onnx-community: Sideporch computes the log-mel spectrogram itself (rustfft, Slaney mel filters), runs the encoder, and decodes greedily with the decoder without a key/value cache. Measured: 5 s of audio in about 0.9 s with base. The browser records, converts to 16 kHz WAV itself, and uploads.
- Models are not in the binary. An admin downloads them under Admin → Speech into `models/` in the data directory; each file is pinned to a Hugging Face revision and checked against its SHA-256. Models load on first use, unload after 15 minutes idle, and run one job at a time. Spoken messages are cached on disk (256 MB at most).
- Without a voice model, reading aloud uses the browser's speech synthesis, which is local on most devices. Without a dictation model, the microphone button is hidden (browser speech recognition sends audio to cloud services in Chrome).
- tract can't prove some symbolic reshapes in these graphs, and one of its optimizations fails on Supertonic's estimator; Sideporch gives every input a concrete shape per call (planning takes 40–120 ms) and runs the estimator unoptimized.

## Context

On 2026-09-29 Niklas asked for a CPU-bound text-to-speech model written in Rust, downloaded as the admin wants and kept in the data directory, to read messages aloud, and speech-to-text to dictate them.

Evaluated and not chosen:

- `any-tts` (Kokoro on candle): its Kokoro phonemizer embeds a Japanese dictionary (lindera with IPA dictionary) and its tokenizers dependency builds C and C++, against the small static binary.
- Piper voices: they need eSpeak NG phonemes, which is GPL and C; a pure-Rust English G2P would limit it to English.
- Crates reimplementing Piper or Kokoro (mercury, kukuryku) are weeks old with no users.
- candle for Whisper: works, but would add a second ML framework to the binary; tract runs both models.
- onnxruntime (`ort`): C++, dynamically loaded, against static musl binaries.

The two directions were validated against each other: Supertonic's output, transcribed by Whisper, returned the original English and German sentences, in the probe, through the server, and in Chromium with a fake microphone. `tests/speech.rs` repeats the round trip when models are available.

## Consequences

- The binary grows by tract's code (see the release notes for sizes); models stay out of it.
- Memory while loaded: roughly 1.2 GB for Supertonic, 0.6–3.5 GB for Whisper. Small servers should pick Whisper tiny or no models. The admin page shows each model's download and memory size.
- Dictation without a key/value cache is quadratic in the number of tokens; long recordings (up to the two-minute limit) take several seconds. Revisit with Whisper's decoder-with-past graphs if people dictate long texts.
- Model licenses are shown before download; Supertonic's OpenRAIL-M adds use restrictions that admins accept by downloading it.
- tract updates may fix the shape and optimization issues; then the per-call planning can go.
