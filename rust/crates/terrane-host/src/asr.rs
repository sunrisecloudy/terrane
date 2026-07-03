//! Edge glue for ambient STT: adapt the `terrane-asr` whisper engine to the
//! session runner's [`AsrEngine`] trait. Compiled with the `asr-engine`
//! feature; a build without it exposes only the shutdown no-op.

#[cfg(feature = "asr-engine")]
use terrane_core::Result;

#[cfg(feature = "asr-engine")]
use crate::stt_runner::{AsrEngine, AsrOutput};

#[cfg(feature = "asr-engine")]
pub(crate) fn shutdown() {
    terrane_asr::clear_whisper_cache();
}

#[cfg(not(feature = "asr-engine"))]
pub(crate) fn shutdown() {}

/// whisper.cpp-backed ASR for [`crate::stt_runner::SttRunner`].
#[cfg(feature = "asr-engine")]
pub struct HostWhisper {
    engine: terrane_asr::WhisperEngine,
}

#[cfg(feature = "asr-engine")]
impl HostWhisper {
    /// Load the model from `TERRANE_STT_MODEL`, or an `hf:repo/file` reference
    /// resolved through the Hugging Face hub cache (same helper as local-model).
    pub fn from_env() -> Result<Self> {
        let path = resolve_model_path()?;
        let engine = terrane_asr::WhisperEngine::new(&terrane_asr::ModelFile { path })
            .map_err(|e| terrane_core::Error::Runtime(e.to_string()))?;
        Ok(Self { engine })
    }

    /// Load an explicit on-disk model path (tests and host wiring).
    pub fn from_path(path: std::path::PathBuf) -> Result<Self> {
        let engine = terrane_asr::WhisperEngine::new(&terrane_asr::ModelFile { path })
            .map_err(|e| terrane_core::Error::Runtime(e.to_string()))?;
        Ok(Self { engine })
    }
}

#[cfg(feature = "asr-engine")]
impl AsrEngine for HostWhisper {
    fn transcribe(&self, pcm: &[i16], sample_rate_hz: u32) -> Result<AsrOutput> {
        let out = self
            .engine
            .transcribe(pcm, sample_rate_hz)
            .map_err(|e| terrane_core::Error::Runtime(e.to_string()))?;
        Ok(AsrOutput {
            text: out.text,
            confidence_milli: out.confidence_milli,
            lang: out.lang,
        })
    }
}

#[cfg(feature = "asr-engine")]
fn resolve_model_path() -> Result<std::path::PathBuf> {
    let raw = std::env::var("TERRANE_STT_MODEL")
        .map_err(|_| terrane_core::Error::Runtime("TERRANE_STT_MODEL is not set".into()))?;
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(terrane_core::Error::Runtime(
            "TERRANE_STT_MODEL is empty".into(),
        ));
    }
    if let Some(path) = hf_source_parts(raw)
        .and_then(|(repo, file)| terrane_local_llm::cached_hf_model_file(repo, file))
    {
        return Ok(path);
    }
    Ok(std::path::PathBuf::from(raw))
}

#[cfg(feature = "asr-engine")]
fn hf_source_parts(source: &str) -> Option<(&str, &str)> {
    let source = source.strip_prefix("hf:")?;
    let (repo, file) = source.rsplit_once('/')?;
    if repo.is_empty() || file.is_empty() {
        return None;
    }
    Some((repo, file))
}