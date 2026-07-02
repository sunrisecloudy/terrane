use std::collections::BTreeMap;

use terrane_cap_interface::AppId;

/// The engines a spec may target today.
pub const BACKENDS: [&str; 1] = ["llama_cpp"];

/// Backends reserved for later phases; registering them is refused with a
/// pointer to the roadmap instead of an "unknown backend" error.
pub const RESERVED_BACKENDS: [&str; 1] = ["mlx"];

/// A registered local model: where its weights live and how to run it.
///
/// Two specs with the same id but different backends are two engine targets,
/// not interchangeable engines — quantization, tokenizer/template handling,
/// and sampler differences all shift output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModelSpec {
    pub backend: String,
    pub format: String,
    pub local_path: String,
    pub context_length: Option<u32>,
    pub chat_template: Option<String>,
    pub max_tokens: Option<u32>,
    /// Sampling temperature in thousandths (700 = 0.7). Integral so the state
    /// slice stays `Eq` and replay-exact.
    pub temperature_milli: Option<u32>,
    /// Where the weights came from (e.g. `hf:<repo>/<file>`), when pulled.
    pub source: Option<String>,
    pub size_bytes: Option<u64>,
}

/// One recorded local inference exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModelTurn {
    pub model: String,
    pub prompt: String,
    pub response: String,
    pub ok: bool,
    pub constrained: bool,
    pub token_count: u32,
    pub duration_ms: u64,
}

/// This capability's slice of State: registered specs (global) plus per-app
/// transcripts of recorded turns, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalModelState {
    pub specs: BTreeMap<String, LocalModelSpec>,
    pub turns: BTreeMap<AppId, Vec<LocalModelTurn>>,
}
