//! The whisper.cpp backend — loads GGML/bin weights and transcribes one utterance
//! at a time. Metal offload is enabled on macOS builds.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::audio::pcm_i16_to_f32_mono_16k;
use crate::{AsrError, AsrOut};

/// A resolved whisper model file on disk.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelFile {
    pub path: PathBuf,
}

/// A loaded whisper.cpp context. One fresh [`whisper_rs::WhisperState`] is
/// created per utterance so cached engines stay stateless between calls.
pub struct WhisperEngine {
    context: Arc<Mutex<WhisperContext>>,
}

/// A process-global cache of loaded contexts, keyed by the resolved model path.
/// Long-lived hosts skip the weight reload per utterance; entries live for the
/// life of the process. Hosts MUST call [`clear_whisper_cache`] before exit on
/// macOS — Metal buffers still resident when ggml destructors run abort the
/// process (same hazard as llama.cpp).
pub fn cached_whisper(file: &ModelFile) -> Result<Arc<Mutex<WhisperContext>>, AsrError> {
    let mut cache = whisper_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(context) = cache.get(&file.path) {
        return Ok(context.clone());
    }
    let context = Arc::new(Mutex::new(load_whisper_context(&file.path)?));
    cache.insert(file.path.clone(), context.clone());
    Ok(context)
}

/// Drop every cached context. Hosts MUST call this before a normal process exit.
pub fn clear_whisper_cache() {
    whisper_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

#[allow(clippy::type_complexity)]
fn whisper_cache() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<WhisperContext>>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<WhisperContext>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn load_whisper_context(path: &Path) -> Result<WhisperContext, AsrError> {
    if !path.is_file() {
        return Err(AsrError::Load(format!(
            "model file not found: {} (set TERRANE_STT_MODEL or re-run model pull)",
            path.display()
        )));
    }
    WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|e| AsrError::Load(format!("{}: {e}", path.display())))
}

impl WhisperEngine {
    /// Load (or hit the process-global cache for) the given model file.
    pub fn new(file: &ModelFile) -> Result<Self, AsrError> {
        Ok(Self {
            context: cached_whisper(file)?,
        })
    }

    /// Transcribe one closed utterance of mono i16 PCM.
    pub fn transcribe(&self, pcm: &[i16], sample_rate_hz: u32) -> Result<AsrOut, AsrError> {
        let audio = pcm_i16_to_f32_mono_16k(pcm, sample_rate_hz)?;
        if audio.is_empty() {
            return Ok(AsrOut {
                text: String::new(),
                confidence_milli: None,
                lang: None,
            });
        }

        let context = self
            .context
            .lock()
            .map_err(|_| AsrError::Transcribe("whisper context lock poisoned".into()))?;
        let mut state = context
            .create_state()
            .map_err(|e| AsrError::Transcribe(format!("create state failed: {e}")))?;

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(default_thread_count());
        params.set_translate(false);
        params.set_language(None);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        state
            .full(params, &audio)
            .map_err(|e| AsrError::Transcribe(format!("inference failed: {e}")))?;

        let num_segments = state.full_n_segments();
        let mut parts = Vec::with_capacity(num_segments as usize);
        for idx in 0..num_segments {
            let segment = state.get_segment(idx).ok_or_else(|| {
                AsrError::Transcribe(format!("segment {idx} missing after inference"))
            })?;
            let text = segment
                .to_str_lossy()
                .map_err(|e| AsrError::Transcribe(format!("segment text failed: {e}")))?;
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                parts.push(trimmed.to_string());
            }
        }

        let lang_id = state.full_lang_id_from_state();
        let lang = whisper_rs::get_lang_str(lang_id).map(str::to_string);

        Ok(AsrOut {
            text: parts.join(" ").trim().to_string(),
            confidence_milli: None,
            lang,
        })
    }
}

fn default_thread_count() -> i32 {
    std::thread::available_parallelism()
        .map(|count| i32::try_from(count.get()).unwrap_or(4).clamp(1, 8))
        .unwrap_or(4)
}