//! Local LLM inference for Terrane's edge — llama.cpp today, MLX later.
//!
//! This crate is **not** a capability and never touches the deterministic
//! core: the `local-model` capability decides; the host's effect runner calls
//! in here exactly once per effect and records the result as events. Backends
//! implement [`LocalLlm`] so the runtime choice stays hidden from callers.

use std::time::Duration;

mod download;
mod llama;
mod mlx;
mod server;
mod setup;

pub use download::{download_model, download_url};
pub use llama::{LlamaCppBackend, ModelFile};
pub use mlx::MlxBackend;
pub use server::{server_status, stop_server, MlxServerStatus};
pub use setup::{
    resolve_runtime, setup_mlx, MlxRuntime, RuntimeSource, SetupReport, MLX_LM_VERSION,
};

/// Errors from loading, generating, or downloading. Typed, no panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmError {
    /// The model could not be loaded (missing file, bad weights, no memory).
    Load(String),
    /// A constraint (JSON schema / GBNF grammar) failed to compile.
    Constraint(String),
    /// Generation failed mid-flight.
    Generate(String),
    /// A model download failed.
    Download(String),
}

impl std::fmt::Display for LlmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LlmError::Load(msg) => write!(f, "model load failed: {msg}"),
            LlmError::Constraint(msg) => write!(f, "constraint failed: {msg}"),
            LlmError::Generate(msg) => write!(f, "generation failed: {msg}"),
            LlmError::Download(msg) => write!(f, "model download failed: {msg}"),
        }
    }
}

impl std::error::Error for LlmError {}

/// How decoding is constrained to typed output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    /// A JSON schema (object) the output must satisfy. Lowered through
    /// llguidance, which is backend-independent — the same path a future MLX
    /// backend will share.
    JsonSchema(String),
    /// A raw GBNF grammar with a `root` rule — the llama.cpp-native escape
    /// hatch when a schema cannot express the shape.
    Gbnf(String),
}

/// Sampling and budget knobs for one generation.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerationConfig {
    pub max_tokens: u32,
    /// 0.0 selects greedy decoding.
    pub temperature: f32,
    pub seed: u32,
    /// Wall-clock budget; exceeding it stops cleanly with partial text.
    pub timeout: Option<Duration>,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        GenerationConfig {
            max_tokens: 512,
            temperature: 0.7,
            seed: 42,
            timeout: None,
        }
    }
}

/// One generation request against a loaded model.
#[derive(Debug, Clone, PartialEq)]
pub struct GenerateRequest {
    pub prompt: String,
    /// Optional system prompt rendered ahead of the conversation.
    pub system: Option<String>,
    /// Prior (user, assistant) exchanges to continue from, oldest first.
    pub history: Vec<(String, String)>,
    pub constraint: Option<Constraint>,
    pub config: GenerationConfig,
}

/// Why decoding stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The model emitted end-of-generation.
    Eos,
    /// The `max_tokens` budget was reached.
    MaxTokens,
    /// The wall-clock budget was reached; `text` holds what was generated.
    DeadlineExceeded,
}

/// The observed result of one generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerateResponse {
    pub text: String,
    pub token_count: u32,
    pub duration: Duration,
    pub stop: StopReason,
    /// How decoding was constrained, when it was: `"schema-mask"` (token-mask
    /// enforced), `"schema-guided"` (prompt-guided + validated), `"grammar"`.
    pub constraint: Option<String>,
}

impl GenerateResponse {
    /// A run that ended by EOS or token budget completed cleanly.
    pub fn ok(&self) -> bool {
        !matches!(self.stop, StopReason::DeadlineExceeded)
    }
}

/// One local inference backend. Object-safe so the edge can pick a backend
/// per model spec (`llama_cpp` today, `mlx` in a later phase).
pub trait LocalLlm {
    /// Generate once, streaming detokenized pieces to `on_token` as they are
    /// sampled. The full text is also returned.
    fn generate(
        &mut self,
        request: &GenerateRequest,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateResponse, LlmError>;
}

/// Parse a (schema-constrained) generation into a typed value.
pub fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, LlmError> {
    serde_json::from_str(text.trim())
        .map_err(|e| LlmError::Generate(format!("output was not the requested JSON shape: {e}")))
}

#[cfg(test)]
mod tests;
