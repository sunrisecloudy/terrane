//! The llama.cpp backend — loads GGUF weights and runs one generation at a
//! time. Metal offload is enabled on macOS builds.

use std::collections::HashMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::llama_batch::LlamaBatch;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::{AddBos, LlamaChatMessage, LlamaChatTemplate, LlamaModel};
use llama_cpp_2::sampling::LlamaSampler;

use crate::{Constraint, GenerateRequest, GenerateResponse, LlmError, LocalLlm, StopReason};

/// A resolved model file plus per-model overrides from the spec.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelFile {
    pub path: PathBuf,
    pub context_length: Option<u32>,
    /// A chat template name (`"chatml"`) or full template string; defaults to
    /// the template embedded in the GGUF.
    pub chat_template_override: Option<String>,
}

/// When no context length is configured, cap the model's trained context to
/// keep memory use sane for small local models.
const DEFAULT_MAX_CONTEXT: u32 = 8192;

/// llama.cpp initializes process-globally exactly once; hold the proof of
/// initialization forever so repeated effects share it.
fn backend() -> Result<&'static LlamaBackend, LlmError> {
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| LlamaBackend::init().map_err(|e| e.to_string()))
        .as_ref()
        .map_err(|e| LlmError::Load(format!("llama backend init failed: {e}")))
}

pub struct LlamaCppBackend {
    model: LlamaModel,
    context_length: u32,
    chat_template: Option<LlamaChatTemplate>,
}

/// A process-global cache of loaded engines, keyed by the resolved model file.
/// Long-lived hosts (macOS app, MCP, serve) skip the GGUF reload per ask; the
/// cache holds only the weights — each generation still gets a fresh context,
/// so cached engines stay stateless between asks. Entries live for the life of
/// the process (weights for local models are expected to be few and reused).
pub fn cached_llama(file: &ModelFile) -> Result<Arc<Mutex<LlamaCppBackend>>, LlmError> {
    let mut cache = llama_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(engine) = cache.get(file) {
        return Ok(engine.clone());
    }
    let engine = Arc::new(Mutex::new(LlamaCppBackend::load(file)?));
    cache.insert(file.clone(), engine.clone());
    Ok(engine)
}

/// Drop every cached engine. Hosts MUST call this before a normal process
/// exit: a model still holding Metal buffers when ggml's static destructors
/// run trips `GGML_ASSERT(residency sets empty)` and aborts the process.
pub fn clear_llama_cache() {
    llama_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clear();
}

#[allow(clippy::type_complexity)]
fn llama_cache() -> &'static Mutex<HashMap<ModelFile, Arc<Mutex<LlamaCppBackend>>>> {
    static CACHE: OnceLock<Mutex<HashMap<ModelFile, Arc<Mutex<LlamaCppBackend>>>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

impl LlamaCppBackend {
    /// Load GGUF weights. Fails fast (before touching llama.cpp) when the
    /// file is missing — the registered spec's path is machine-local claim,
    /// verified only here at the edge.
    pub fn load(file: &ModelFile) -> Result<Self, LlmError> {
        if !file.path.is_file() {
            return Err(LlmError::Load(format!(
                "model file not found: {} (re-run local-model pull, or fix the registered path)",
                file.path.display()
            )));
        }
        let backend = backend()?;

        let mut model_params = LlamaModelParams::default();
        if cfg!(target_os = "macos") {
            // Offload every layer to Metal; llama.cpp falls back to CPU when
            // the build has no GPU backend.
            model_params = model_params.with_n_gpu_layers(1_000_000);
        }
        let model = LlamaModel::load_from_file(backend, &file.path, &model_params)
            .map_err(|e| LlmError::Load(format!("{}: {e}", file.path.display())))?;

        let chat_template = match &file.chat_template_override {
            Some(template) => Some(LlamaChatTemplate::new(template).map_err(|e| {
                LlmError::Load(format!("invalid chat template override {template:?}: {e}"))
            })?),
            // A missing embedded template is fine — prompts go in raw.
            None => model.chat_template(None).ok(),
        };
        let context_length = file
            .context_length
            .unwrap_or_else(|| model.n_ctx_train().min(DEFAULT_MAX_CONTEXT))
            .max(1);

        Ok(LlamaCppBackend {
            model,
            context_length,
            chat_template,
        })
    }

    /// Render one user message through the model's chat template (leaving the
    /// assistant tag open), or pass the conversation through as plain text
    /// when the model has no template.
    fn render_prompt(&self, request: &GenerateRequest) -> Result<String, LlmError> {
        let Some(template) = &self.chat_template else {
            // No template: flatten the conversation deterministically.
            let mut plain = String::new();
            if let Some(system) = &request.system {
                plain.push_str(system);
                plain.push_str("\n\n");
            }
            for (user, assistant) in &request.history {
                plain.push_str("User: ");
                plain.push_str(user);
                plain.push_str("\nAssistant: ");
                plain.push_str(assistant);
                plain.push('\n');
            }
            if !plain.is_empty() {
                plain.push_str("User: ");
                plain.push_str(&request.prompt);
                plain.push_str("\nAssistant:");
                return Ok(plain);
            }
            return Ok(request.prompt.clone());
        };

        let mut messages = Vec::new();
        let message = |role: &str, content: &str| {
            LlamaChatMessage::new(role.to_string(), content.to_string())
                .map_err(|e| LlmError::Generate(format!("bad chat message: {e}")))
        };
        if let Some(system) = &request.system {
            messages.push(message("system", system)?);
        }
        for (user, assistant) in &request.history {
            messages.push(message("user", user)?);
            messages.push(message("assistant", assistant)?);
        }
        messages.push(message("user", &request.prompt)?);
        let mut prompt = self
            .model
            .apply_chat_template(template, &messages, true)
            .map_err(|e| LlmError::Generate(format!("chat template failed: {e}")))?;
        // Thinking-capable templates (Qwen3-family): llama.cpp cannot pass
        // enable_thinking=false, so pre-fill the empty think block the HF
        // template would render — the model then answers directly instead of
        // spending the token budget on (or leaking) reasoning.
        if template
            .to_str()
            .is_ok_and(|source| source.contains("<think>"))
            && !prompt.trim_end().ends_with("<think>")
        {
            prompt.push_str("<think>\n\n</think>\n\n");
        }
        Ok(prompt)
    }

    fn build_sampler(&self, request: &GenerateRequest) -> Result<LlamaSampler, LlmError> {
        let mut chain = Vec::new();
        match &request.constraint {
            Some(Constraint::JsonSchema(schema)) => {
                chain.push(
                    LlamaSampler::llguidance(&self.model, "json_schema", schema)
                        .map_err(|e| LlmError::Constraint(format!("json schema: {e}")))?,
                );
            }
            Some(Constraint::Gbnf(grammar)) => {
                chain.push(
                    LlamaSampler::grammar(&self.model, grammar, "root")
                        .map_err(|e| LlmError::Constraint(format!("gbnf grammar: {e}")))?,
                );
            }
            None => {}
        }
        if request.config.temperature <= 0.0 {
            chain.push(LlamaSampler::greedy());
        } else {
            chain.push(LlamaSampler::temp(request.config.temperature));
            chain.push(LlamaSampler::dist(request.config.seed));
        }
        Ok(LlamaSampler::chain_simple(chain))
    }
}

impl LocalLlm for LlamaCppBackend {
    fn generate(
        &mut self,
        request: &GenerateRequest,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateResponse, LlmError> {
        let started = Instant::now();
        let deadline = request.config.timeout.map(|budget| started + budget);

        let prompt = self.render_prompt(request)?;
        let tokens = self
            .model
            .str_to_token(&prompt, AddBos::Always)
            .map_err(|e| LlmError::Generate(format!("tokenize failed: {e}")))?;
        let prompt_len = u32::try_from(tokens.len())
            .map_err(|_| LlmError::Generate("prompt too long".into()))?;
        if prompt_len + request.config.max_tokens > self.context_length {
            return Err(LlmError::Generate(format!(
                "prompt ({prompt_len} tokens) plus max_tokens ({}) exceeds the context length ({})",
                request.config.max_tokens, self.context_length
            )));
        }

        let batch_capacity = (tokens.len().max(512)) as u32;
        let context_params = LlamaContextParams::default()
            .with_n_ctx(NonZeroU32::new(self.context_length))
            .with_n_batch(batch_capacity);
        let mut context = self
            .model
            .new_context(backend()?, context_params)
            .map_err(|e| LlmError::Generate(format!("context init failed: {e}")))?;
        let mut sampler = self.build_sampler(request)?;

        // Feed the whole prompt, asking for logits on its last token only.
        let mut batch = LlamaBatch::new(batch_capacity as usize, 1);
        for (i, token) in tokens.iter().enumerate() {
            batch
                .add(*token, i as i32, &[0], i + 1 == tokens.len())
                .map_err(|e| LlmError::Generate(format!("batch add failed: {e}")))?;
        }
        context
            .decode(&mut batch)
            .map_err(|e| LlmError::Generate(format!("prompt decode failed: {e}")))?;

        // One token per step: sample (the chain also accepts it), emit, decode.
        let mut text = String::new();
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut position = tokens.len() as i32;
        let mut token_count: u32 = 0;
        let stop = loop {
            if token_count >= request.config.max_tokens {
                break StopReason::MaxTokens;
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                break StopReason::DeadlineExceeded;
            }

            let token = sampler.sample(&context, batch.n_tokens() - 1);
            if self.model.is_eog_token(token) {
                break StopReason::Eos;
            }
            let piece = self
                .model
                .token_to_piece(token, &mut decoder, false, None)
                .map_err(|e| LlmError::Generate(format!("detokenize failed: {e}")))?;
            if !piece.is_empty() {
                on_token(&piece);
                text.push_str(&piece);
            }
            token_count += 1;

            batch.clear();
            batch
                .add(token, position, &[0], true)
                .map_err(|e| LlmError::Generate(format!("batch add failed: {e}")))?;
            position += 1;
            context
                .decode(&mut batch)
                .map_err(|e| LlmError::Generate(format!("decode failed: {e}")))?;
        };

        Ok(GenerateResponse {
            text: strip_think_prefix(&text).to_string(),
            token_count,
            duration: started.elapsed(),
            stop,
            // Both llama.cpp constraint paths are token-mask enforced.
            constraint: request.constraint.as_ref().map(|constraint| {
                match constraint {
                    Constraint::JsonSchema(_) => "schema-mask",
                    Constraint::Gbnf(_) => "grammar",
                }
                .to_string()
            }),
        })
    }
}

/// Drop a leading `<think>…</think>` block (plus surrounding whitespace) from
/// generated text: reasoning is not part of the answer an app or user asked
/// for. An unclosed block is left untouched — a budget-truncated response
/// should stay visibly truncated rather than become silently empty.
pub fn strip_think_prefix(text: &str) -> &str {
    let trimmed = text.trim_start();
    let Some(rest) = trimmed.strip_prefix("<think>") else {
        return text;
    };
    match rest.find("</think>") {
        Some(end) => rest[end + "</think>".len()..].trim_start(),
        None => text,
    }
}
