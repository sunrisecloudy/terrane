//! On-device ASR for Terrane's host edge — whisper.cpp today, more backends later.
//!
//! This crate is **not** a capability and never touches the deterministic core:
//! the `stt` capability records finalized transcript text; the host's session
//! runner calls in here on closed utterances. Backends expose a pure transcribe
//! surface so the `terrane-host` bridge can adapt them to [`AsrEngine`] without
//! a circular dependency.

mod audio;

#[cfg(feature = "whisper")]
mod whisper;

pub use audio::{pcm_i16_to_f32_mono_16k, WHISPER_SAMPLE_RATE_HZ};

#[cfg(feature = "whisper")]
pub use whisper::{cached_whisper, clear_whisper_cache, ModelFile, WhisperEngine};

/// Errors from loading or transcribing. Typed, no panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsrError {
    /// The model could not be loaded (missing file, bad weights, no memory).
    Load(String),
    /// Transcription failed mid-flight.
    Transcribe(String),
}

impl std::fmt::Display for AsrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AsrError::Load(msg) => write!(f, "asr model load failed: {msg}"),
            AsrError::Transcribe(msg) => write!(f, "asr transcribe failed: {msg}"),
        }
    }
}

impl std::error::Error for AsrError {}

/// A finalized transcript for one closed utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsrOut {
    pub text: String,
    /// Confidence in thousandths (0-1000), when the engine reports one.
    pub confidence_milli: Option<u32>,
    /// BCP-47 language code, when detected or configured.
    pub lang: Option<String>,
}