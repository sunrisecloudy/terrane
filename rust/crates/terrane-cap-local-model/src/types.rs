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

/// The recommended zero-config *embedding* model: what `local-model.pull
/// --embed` fetches and what `embed` resolves to when no embedding default is
/// set. nomic-embed-text-v1.5 is small, Apache-2.0, mean-pooled, and only needs
/// its `search_query:` / `search_document:` prefixes.
pub const RECOMMENDED_EMBED_MODEL_ID: &str = "nomic_embed_text_v1_5";
pub const RECOMMENDED_EMBED_GGUF_REPO: &str = "nomic-ai/nomic-embed-text-v1.5-GGUF";
pub const RECOMMENDED_EMBED_GGUF_FILE: &str = "nomic-embed-text-v1.5.Q8_0.gguf";
/// The preset a bare `--embed` pull applies.
pub const RECOMMENDED_EMBED_PRESET: &str = "nomic";

/// How an embedding model must be driven: pooling, the required per-side
/// prefixes, whether to L2-normalize, and an optional Matryoshka truncation
/// dimension. Recognized presets fill this in so callers can't misconfigure a
/// known encoder ([`embed_preset`]); it rides on the model spec so the edge and
/// replay see the same, recorded configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddingConfig {
    /// `"mean"`, `"cls"`, or `"last"`.
    pub pooling: String,
    /// Prepended to text embedded as a search query.
    pub query_prefix: String,
    /// Prepended to text embedded as an indexed document.
    pub document_prefix: String,
    /// L2-normalize each vector (cosine similarity becomes a dot product).
    pub normalize: bool,
    /// Truncate each vector to this many leading dims (Matryoshka), then
    /// renormalize; `None` keeps the model's native dimension.
    pub dim: Option<u32>,
}

/// The embedding config for a recognized preset name, if any. Add a new arm per
/// encoder family rather than letting callers hand-tune pooling/prefixes.
pub fn embed_preset(name: &str) -> Option<EmbeddingConfig> {
    match name {
        "nomic" => Some(EmbeddingConfig {
            pooling: "mean".to_string(),
            query_prefix: "search_query: ".to_string(),
            document_prefix: "search_document: ".to_string(),
            normalize: true,
            dim: None,
        }),
        _ => None,
    }
}

/// The preset names [`embed_preset`] recognizes, for error messages.
pub const EMBED_PRESETS: [&str; 1] = ["nomic"];

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
    /// A smaller same-tokenizer model for speculative decoding (mlx only;
    /// requires a model whose caches can rewind — standard attention).
    pub draft_model: Option<String>,
    /// Present when this spec is an embedding model: how to pool, prefix, and
    /// normalize. `None` means a generation model (the default).
    pub embedding: Option<EmbeddingConfig>,
}

/// One recorded local inference exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalModelTurn {
    pub model: String,
    pub prompt: String,
    pub system: Option<String>,
    /// Whether prior recorded turns were fed back as conversation context.
    pub continued: bool,
    pub response: String,
    pub ok: bool,
    /// `"schema-mask"`, `"schema-guided"`, or `"grammar"` when constrained.
    pub constraint: Option<String>,
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
    /// first registered *generation* model; cleared when that model is removed.
    pub default_model: Option<String>,
    /// The model `embed` resolves to when none is named. Tracked separately
    /// from `default_model` because chat and embedding models are distinct;
    /// set automatically by the first registered embedding model.
    pub default_embed_model: Option<String>,
    pub turns: BTreeMap<AppId, Vec<LocalModelTurn>>,
}
