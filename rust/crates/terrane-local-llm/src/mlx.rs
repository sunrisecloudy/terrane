//! The MLX backend — drives `mlx_lm.generate` (the reference Apple-Silicon
//! runtime) as a subprocess, one generation per call.
//!
//! GGUF and MLX builds of the same weights are two engine targets, not
//! interchangeable engines: quantization, template handling, and samplers all
//! shift output. Constrained output here is *typed but not mask-enforced*:
//! a JSON schema becomes prompt guidance plus post-generation extraction and
//! validation with one retry, unlike llama.cpp's token-mask llguidance path.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::{Constraint, GenerateRequest, GenerateResponse, LlmError, LocalLlm, StopReason};

/// `mlx_lm.generate` wraps the generated text between these marker lines and
/// prints token stats after the closing marker.
const MARKER: &str = "==========";

pub struct MlxBackend {
    binary: String,
    /// A Hugging Face repo id (`mlx-community/...`) or a local model directory;
    /// `mlx_lm` resolves and caches it.
    model_ref: String,
}

impl MlxBackend {
    /// Resolve the `mlx_lm.generate` binary (override with
    /// `TERRANE_MLX_LM_BIN`) and remember the model reference. The model
    /// itself is resolved by `mlx_lm` on first generation.
    pub fn load(model_ref: &str) -> Result<Self, LlmError> {
        if model_ref.trim().is_empty() {
            return Err(LlmError::Load("mlx model reference is empty".into()));
        }
        let binary = std::env::var("TERRANE_MLX_LM_BIN")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "mlx_lm.generate".to_string());
        if Command::new(&binary)
            .arg("--help")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return Err(LlmError::Load(format!(
                "`{binary}` not found; install the MLX runtime with `uv tool install mlx-lm` \
                 (or set TERRANE_MLX_LM_BIN)"
            )));
        }
        Ok(MlxBackend {
            binary,
            model_ref: model_ref.to_string(),
        })
    }

    fn run_once(
        &self,
        prompt: &str,
        request: &GenerateRequest,
        deadline: Option<Instant>,
    ) -> Result<MlxRun, LlmError> {
        let mut command = Command::new(&self.binary);
        command
            .arg("--model")
            .arg(&self.model_ref)
            .arg("--prompt")
            .arg(prompt)
            .arg("--max-tokens")
            .arg(request.config.max_tokens.to_string())
            .arg("--temp")
            .arg(request.config.temperature.to_string())
            .arg("--seed")
            .arg(request.config.seed.to_string());
        let (stdout, timed_out) = run_with_deadline(command, &self.binary, deadline)?;
        let mut run = parse_mlx_output(&stdout)?;
        run.timed_out = timed_out;
        Ok(run)
    }
}

struct MlxRun {
    body: String,
    token_count: u32,
    timed_out: bool,
}

impl LocalLlm for MlxBackend {
    /// Generate once. The subprocess boundary means tokens arrive as one
    /// callback with the whole body rather than piece-by-piece.
    fn generate(
        &mut self,
        request: &GenerateRequest,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateResponse, LlmError> {
        let started = Instant::now();
        let deadline = request.config.timeout.map(|budget| started + budget);

        let (body, token_count, timed_out) = match &request.constraint {
            None => {
                let run = self.run_once(&request.prompt, request, deadline)?;
                (run.body, run.token_count, run.timed_out)
            }
            Some(Constraint::Gbnf(_)) => {
                return Err(LlmError::Constraint(
                    "gbnf grammars are llama_cpp-only; use a JSON schema on mlx".into(),
                ));
            }
            Some(Constraint::JsonSchema(schema)) => {
                // Prompt-guided typed output with one corrective retry.
                let guided = format!(
                    "{}\n\nRespond with ONLY a single JSON object (no prose, no code fences) \
                     that matches this JSON schema:\n{schema}",
                    request.prompt
                );
                let first = self.run_once(&guided, request, deadline)?;
                match extract_json_object(&first.body) {
                    Some(json) => (json, first.token_count, first.timed_out),
                    None if first.timed_out => (first.body, first.token_count, true),
                    None => {
                        let retry_prompt = format!(
                            "{guided}\n\nYour previous reply was not a valid JSON object. \
                             Reply again with ONLY the JSON object."
                        );
                        let second = self.run_once(&retry_prompt, request, deadline)?;
                        let tokens = first.token_count.saturating_add(second.token_count);
                        match extract_json_object(&second.body) {
                            Some(json) => (json, tokens, second.timed_out),
                            None => {
                                return Err(LlmError::Constraint(
                                    "mlx output was not a valid JSON object after a retry".into(),
                                ))
                            }
                        }
                    }
                }
            }
        };

        if !body.is_empty() {
            on_token(&body);
        }
        let stop = if timed_out {
            StopReason::DeadlineExceeded
        } else if token_count >= request.config.max_tokens {
            StopReason::MaxTokens
        } else {
            StopReason::Eos
        };
        Ok(GenerateResponse {
            text: body,
            token_count,
            duration: started.elapsed(),
            stop,
        })
    }
}

/// Run to completion or deadline (kill on deadline; partial stdout is kept).
fn run_with_deadline(
    mut command: Command,
    label: &str,
    deadline: Option<Instant>,
) -> Result<(String, bool), LlmError> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| LlmError::Generate(format!("failed to run `{label}`: {e}")))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| LlmError::Generate(format!("failed to capture `{label}` stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| LlmError::Generate(format!("failed to capture `{label}` stderr")))?;
    let (sender, receiver) = mpsc::channel::<Vec<u8>>();
    let reader = thread::spawn(move || {
        let mut stdout = stdout;
        let mut buffer = [0u8; 8192];
        loop {
            match stdout.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    if sender.send(buffer[..read].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut stderr = stderr;
        let mut out = Vec::new();
        let _ = stderr.read_to_end(&mut out);
        out
    });

    let mut collected = Vec::new();
    let mut timed_out = false;
    loop {
        let wait = Duration::from_millis(50);
        match receiver.recv_timeout(wait) {
            Ok(chunk) => collected.extend_from_slice(&chunk),
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    let _ = child.kill();
                    timed_out = true;
                    break;
                }
            }
        }
    }
    // Drain whatever arrived between the last recv and process exit.
    while let Ok(chunk) = receiver.recv_timeout(Duration::from_millis(200)) {
        collected.extend_from_slice(&chunk);
    }
    let status = child
        .wait()
        .map_err(|e| LlmError::Generate(format!("failed to wait for `{label}`: {e}")))?;
    let _ = reader.join();
    let stderr_bytes = stderr_reader.join().unwrap_or_default();

    if !timed_out && !status.success() {
        let stderr_text = String::from_utf8_lossy(&stderr_bytes);
        return Err(LlmError::Generate(format!(
            "`{label}` exited with {}: {}",
            status.code().unwrap_or(-1),
            stderr_text.trim().chars().take(400).collect::<String>()
        )));
    }
    Ok((String::from_utf8_lossy(&collected).into_owned(), timed_out))
}

/// Parse `mlx_lm.generate` stdout: the body sits between two marker lines and
/// a `Generation: N tokens, X tokens-per-sec` stat line follows.
fn parse_mlx_output(stdout: &str) -> Result<MlxRun, LlmError> {
    let Some(open) = stdout.find(MARKER) else {
        return Err(LlmError::Generate(format!(
            "unrecognized mlx_lm output (no marker): {}",
            stdout.trim().chars().take(200).collect::<String>()
        )));
    };
    let after_open = &stdout[open + MARKER.len()..];
    let (body, tail) = match after_open.find(MARKER) {
        Some(close) => (&after_open[..close], &after_open[close + MARKER.len()..]),
        // Killed mid-generation: everything after the marker is partial body.
        None => (after_open, ""),
    };
    let token_count = parse_generation_tokens(tail).unwrap_or(0);
    Ok(MlxRun {
        body: body.trim().to_string(),
        token_count,
        timed_out: false,
    })
}

pub(crate) fn parse_generation_tokens(stats: &str) -> Option<u32> {
    let line = stats
        .lines()
        .find(|line| line.trim_start().starts_with("Generation:"))?;
    line.split(':')
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// Pull the first balanced-looking JSON object out of model prose (Qwen-style
/// thinking preambles, code fences, trailing chatter).
pub(crate) fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    let candidate = &text[start..=end];
    serde_json::from_str::<serde_json::Value>(candidate)
        .ok()
        .filter(serde_json::Value::is_object)?;
    Some(candidate.to_string())
}
