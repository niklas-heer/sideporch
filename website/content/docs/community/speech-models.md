+++
title = "Speech models"
description = "Download the models that read messages aloud and take dictation, and what they need."
weight = 8
+++

Reading aloud and dictation run on your server's processor with open models, run by [tract](https://github.com/sonos/tract), a Rust library: nothing is sent to a speech service. An admin downloads the models under **Admin → Speech**.

| Model | Does | Languages | Memory while in use |
| --- | --- | --- | --- |
| **Supertonic 3** | reads aloud, with ten voices | 31, picked from each message | about 1.2 GB |
| **Whisper tiny** | dictation; fastest, for small servers | 99 | about 0.6 GB |
| **Whisper base** | dictation; a balance of speed and accuracy | 99 | about 1.1 GB |
| **Whisper small** | dictation; most accurate, several times slower | 99 | about 3.5 GB |

Downloads are checked against pinned hashes and kept in the data directory. They come from Hugging Face.

The models load when first used and leave memory after 15 minutes without use, so they cost nothing while nobody speaks. They run one at a time. On a server with 1 GB of memory or less, use Whisper tiny, or no speech models.

Without a voice model, reading aloud uses the voices of each person's own device. Without a Whisper model, the microphone isn't shown.

People choose their voice and reading speed under **Appearance**. See [Read aloud and dictate](@/docs/using/read-aloud-and-dictation.md).
