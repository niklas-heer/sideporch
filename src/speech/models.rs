//! The models Sideporch can download, pinned to a revision and checked
//! against their SHA-256, and where they live in the data directory.

use std::path::{Path, PathBuf};

/// What a model does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Reads messages aloud.
    Voice,
    /// Turns speech into text.
    Dictation,
}

#[derive(Debug)]
pub struct Model {
    pub key: &'static str,
    pub kind: Kind,
    pub name: &'static str,
    pub description: &'static str,
    pub license: &'static str,
    pub license_url: &'static str,
    /// Memory it takes while loaded, roughly, in MB.
    pub memory_mb: u32,
    pub repository: &'static str,
    pub revision: &'static str,
    /// Path, size in bytes, SHA-256.
    pub files: &'static [(&'static str, u64, &'static str)],
    /// The encoder's width, for dictation models.
    pub width: usize,
}

impl Model {
    pub fn size(&self) -> u64 {
        self.files.iter().map(|(_, size, _)| size).sum()
    }

    pub fn dir(&self, data_dir: &Path) -> PathBuf {
        data_dir.join("models").join(self.key)
    }

    /// Whether every file is in place at its expected size.
    pub fn installed(&self, data_dir: &Path) -> bool {
        let dir = self.dir(data_dir);
        self.files.iter().all(|(path, size, _)| {
            std::fs::metadata(dir.join(path)).is_ok_and(|meta| meta.len() == *size)
        })
    }

    pub fn url(&self, base: &str, path: &str) -> String {
        format!(
            "{base}/{}/resolve/{}/{path}",
            self.repository, self.revision
        )
    }
}

pub const MODELS: &[Model] = &[
    Model {
        key: "supertonic-3",
        kind: Kind::Voice,
        name: "Supertonic 3",
        description: "Natural voices in 31 languages, among them English, German, French, Spanish, Italian, Dutch, Polish, Japanese and Korean. Ten voices. Reads about five times faster than real time on a laptop.",
        license: "OpenRAIL-M: free to use, with use restrictions against harm",
        license_url: "https://huggingface.co/Supertone/supertonic-3/blob/main/LICENSE",
        memory_mb: 1200,
        repository: "Supertone/supertonic-3",
        revision: "3cadd1ee6394adea1bd021217a0e650ede09a323",
        files: &[
            (
                "onnx/duration_predictor.onnx",
                3_700_147,
                "c3eb91414d5ff8a7a239b7fe9e34e7e2bf8a8140d8375ffb14718b1c639325db",
            ),
            (
                "onnx/text_encoder.onnx",
                36_416_150,
                "c7befd5ea8c3119769e8a6c1486c4edc6a3bc8365c67621c881bbb774b9902ff",
            ),
            (
                "onnx/vector_estimator.onnx",
                256_534_781,
                "883ac868ea0275ef0e991524dc64f16b3c0376efd7c320af6b53f5b780d7c61c",
            ),
            (
                "onnx/vocoder.onnx",
                101_424_195,
                "085de76dd8e8d5836d6ca66826601f615939218f90e519f70ee8a36ed2a4c4ba",
            ),
            (
                "onnx/tts.json",
                8253,
                "42078d3aef1cd43ab43021f3c54f47d2d75ceb4e75f627f118890128b06a0d09",
            ),
            (
                "onnx/unicode_indexer.json",
                277_676,
                "9bf7346e43883a81f8645c81224f786d43c5b57f3641f6e7671a7d6c493cb24f",
            ),
            (
                "voice_styles/F1.json",
                292_046,
                "bbdec6ee00231c2c742ad05483df5334cab3b52fda3ba38e6a07059c4563dbc2",
            ),
            (
                "voice_styles/F2.json",
                292_423,
                "7c722c6a72707b1a77f035d67f0d1351ba187738e06f7683e8c72b1df3477fc6",
            ),
            (
                "voice_styles/F3.json",
                290_794,
                "12f6ef2573baa2defa1128069cb59f203e3ab67c92af77b42df8a0e3a2f7c6ab",
            ),
            (
                "voice_styles/F4.json",
                291_808,
                "c2fa764c1225a76dfc3e2c73e8aa4f70d9ee48793860eb34c295fff01c2e032b",
            ),
            (
                "voice_styles/F5.json",
                291_479,
                "45966e73316415626cf41a7d1c6f3b4c70dbc1ba2bee5c1978ef0ce33244fc8d",
            ),
            (
                "voice_styles/M1.json",
                291_748,
                "e35604687f5d23694b8e91593a93eec0e4eca6c0b02bb8ed69139ab2ea6b0a5b",
            ),
            (
                "voice_styles/M2.json",
                292_055,
                "b76cbf62bac707c710cf0ae5aba5e31eea1a6339a9734bfae33ab98499534a50",
            ),
            (
                "voice_styles/M3.json",
                290_198,
                "ea1ac35ccb91b0d7ecad533a2fbd0eec10c91513d8951e3b25fbba99954e159b",
            ),
            (
                "voice_styles/M4.json",
                291_522,
                "ca8eefad4fcd989c9379032ff3e50738adc547eeb5e221b82593a6d7b3bac303",
            ),
            (
                "voice_styles/M5.json",
                291_469,
                "dd22b92740314321f8ae11c5e87f8dd60d060f15dd3a632b5adf77f471f77af2",
            ),
            (
                "LICENSE",
                15_007,
                "0d944a9110fed9a9602d60e0423a272903e7bd21ab060490774efc77c2275e9f",
            ),
        ],
        width: 0,
    },
    Model {
        key: "whisper-tiny",
        kind: Kind::Dictation,
        name: "Whisper tiny",
        description: "Fastest, for small servers. Good for clear speech; makes more mistakes with names and accents. Understands 99 languages.",
        license: "MIT",
        license_url: "https://github.com/openai/whisper/blob/main/LICENSE",
        memory_mb: 600,
        repository: "onnx-community/whisper-tiny",
        revision: "ff4177021cc41f7db950912b73ea4fdf7d01d8e7",
        files: &[
            (
                "onnx/encoder_model.onnx",
                32_904_992,
                "6642befb640f950d4a8cbbd17834d59e7e75f575b81ccf213e06b050623ab1dd",
            ),
            (
                "onnx/decoder_model.onnx",
                118_397_483,
                "ab79e3f2a9a3d98f159f853a3172120a38af7eb5f7863d706aa7d39c228f009e",
            ),
            (
                "vocab.json",
                1_036_584,
                "50d6a919f0a0601d56a04eb583c780d18553aa388254ba3158eb6a00f13e2c1a",
            ),
            (
                "added_tokens.json",
                34_604,
                "9715fd2243b6f06a5858b5e32950d2853f73dd5bc201aafcf76f5082a2d8acd1",
            ),
        ],
        width: 384,
    },
    Model {
        key: "whisper-base",
        kind: Kind::Dictation,
        name: "Whisper base",
        description: "A good balance of speed and accuracy for most servers. Understands 99 languages.",
        license: "MIT",
        license_url: "https://github.com/openai/whisper/blob/main/LICENSE",
        memory_mb: 1100,
        repository: "onnx-community/whisper-base",
        revision: "1846881b6b3a3024392c1eea3ad983695bc23925",
        files: &[
            (
                "onnx/encoder_model.onnx",
                82_468_078,
                "a9f3b752833b49e880dec91ee5b6d936112be7c3ea07c221024ba493439f46fe",
            ),
            (
                "onnx/decoder_model.onnx",
                208_289_724,
                "70d26763610c0d6bb407373b7f30d415252ee470e62a0f816c8a46b2caca7326",
            ),
            (
                "vocab.json",
                1_036_584,
                "50d6a919f0a0601d56a04eb583c780d18553aa388254ba3158eb6a00f13e2c1a",
            ),
            (
                "added_tokens.json",
                34_604,
                "9715fd2243b6f06a5858b5e32950d2853f73dd5bc201aafcf76f5082a2d8acd1",
            ),
        ],
        width: 512,
    },
    Model {
        key: "whisper-small",
        kind: Kind::Dictation,
        name: "Whisper small",
        description: "Most accurate, but several times slower; for servers with CPU to spare. Understands 99 languages.",
        license: "MIT",
        license_url: "https://github.com/openai/whisper/blob/main/LICENSE",
        memory_mb: 3500,
        repository: "onnx-community/whisper-small",
        revision: "36050c46d777d46dc4b5f43f6d90574fc38f8732",
        files: &[
            (
                "onnx/encoder_model.onnx",
                352_825_870,
                "b37cd6625dc36f9178ec7539a1876b9680ea26a910097e092be39dc766320c7b",
            ),
            (
                "onnx/decoder_model.onnx",
                614_865_004,
                "12130ce1e82372a8e54e753d3fb4339f289470cbbe2eceb9c8bf89cb2cc6fe63",
            ),
            (
                "vocab.json",
                1_036_584,
                "50d6a919f0a0601d56a04eb583c780d18553aa388254ba3158eb6a00f13e2c1a",
            ),
            (
                "added_tokens.json",
                34_604,
                "9715fd2243b6f06a5858b5e32950d2853f73dd5bc201aafcf76f5082a2d8acd1",
            ),
        ],
        width: 768,
    },
];

pub fn find(key: &str) -> Option<&'static Model> {
    MODELS.iter().find(|model| model.key == key)
}
