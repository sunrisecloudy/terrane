use std::collections::BTreeMap;

use terrane_cap_interface::AppId;

/// The engines a spec may target: llama.cpp over GGUF weights, and the MLX
/// runtime on Apple Silicon.
pub const BACKENDS: [&str; 2] = ["llama_cpp", "mlx"];

/// The recommended zero-config model: what a bare `local-model.pull` fetches
/// and what new homes default to once it is registered.
pub const RECOMMENDED_MODEL_ID: &str = "qwen3_5_0_8b";
pub const RECOMMENDED_GGUF_REPO: &str = "unsloth/Qwen3.5-0.8B-GGUF";
pub const RECOMMENDED_GGUF_FILE: &str = "Qwen3.5-0.8B-Q4_K_M.gguf";
/// The Apple-acceleration build of the same recommended model.
pub const RECOMMENDED_MLX_MODEL_ID: &str = "qwen3_5_0_8b_mlx";
pub const RECOMMENDED_MLX_REPO: &str = "mlx-community/Qwen3.5-0.8B-MLX-4bit";

/// A registered local model: where its weights live and how to run it.
///
/// Two specs with the same id but different backends are two engine targets,
/// not interchangeable engines — quantization, tokenizer/template handling,
/// and sampler differences all shift output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModelSpec {
    pub backend: String,
    pub format: String,
    /// For `llama_cpp`: path to the GGUF weights file. For `mlx`: a local
    /// model directory or a Hugging Face repo id the MLX runtime resolves.
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

/// This capability's slice of State: registered specs (global), the default
/// model asks resolve to when none is named, plus per-app transcripts of
/// recorded turns, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalModelState {
    pub specs: BTreeMap<String, LocalModelSpec>,
    /// Set explicitly via `local-model.default`, or automatically by the
    /// first registered model; cleared when that model is removed.
    pub default_model: Option<String>,
    pub turns: BTreeMap<AppId, Vec<LocalModelTurn>>,
}
