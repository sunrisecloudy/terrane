//! The MLX backend — the reference Apple-Silicon runtime, resident-first.
//!
//! Generations go to the shared resident `mlx_lm.server` (see [`crate::server`])
//! over the OpenAI-compatible streaming API, so tokens stream live and the
//! model stays loaded between calls; if the server cannot be used
//! (`TERRANE_MLX_RESIDENT=0`, or it fails to start) each call falls back to a
//! one-shot `mlx_lm.generate` subprocess.
//!
//! GGUF and MLX builds of the same weights are two engine targets, not
//! interchangeable engines. Constrained output here is *typed but not
//! mask-enforced*: a JSON schema becomes prompt guidance plus post-generation
//! extraction and validation with one retry, unlike llama.cpp's token-mask
//! llguidance path.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::server::{ensure_server, touch};
use crate::setup::{resolve_runtime, MlxRuntime};
use crate::{Constraint, GenerateRequest, GenerateResponse, LlmError, LocalLlm, StopReason};

/// `mlx_lm.generate` wraps the generated text between these marker lines and
/// prints token stats after the closing marker.
const MARKER: &str = "==========";

pub struct MlxBackend {
    runtime: MlxRuntime,
    home: PathBuf,
    /// A Hugging Face repo id (`mlx-community/...`) or a local model directory;
    /// `mlx_lm` resolves and caches it.
    model_ref: String,
}

impl MlxBackend {
    /// Resolve the MLX runtime for `home` (env override → engines manifest →
    /// PATH) and remember the model reference. The model itself is resolved
    /// by `mlx_lm` on first generation.
    pub fn load(home: &Path, model_ref: &str) -> Result<Self, LlmError> {
        if model_ref.trim().is_empty() {
            return Err(LlmError::Load("mlx model reference is empty".into()));
        }
        let runtime = resolve_runtime(home).ok_or_else(|| {
            LlmError::Load(
                "no MLX runtime found; run `terrane local-model setup mlx` \
                 (or set TERRANE_MLX_LM_BIN)"
                    .into(),
            )
        })?;
        Ok(MlxBackend {
            runtime,
            home: home.to_path_buf(),
            model_ref: model_ref.to_string(),
        })
    }

    /// One generation over whichever transport is available. `stream_to`
    /// receives pieces as they arrive (resident transport streams per token;
    /// the one-shot fallback delivers the whole body once).
    fn run_transport(
        &self,
        prompt: &str,
        request: &GenerateRequest,
        deadline: Option<Instant>,
        stream_to: Option<&mut dyn FnMut(&str)>,
    ) -> Result<MlxRun, LlmError> {
        if resident_enabled() {
            match ensure_server(&self.home, &self.runtime) {
                Ok(port) => return self.run_resident(port, prompt, request, deadline, stream_to),
                Err(error) => {
                    // The one-shot path gives a second chance (and its own,
                    // equally clear error when the runtime is truly absent).
                    eprintln!("mlx resident server unavailable ({error}); falling back to one-shot generation");
                }
            }
        }
        self.run_once(prompt, request, deadline, stream_to)
    }

    /// Stream one chat completion from the resident server.
    fn run_resident(
        &self,
        port: u16,
        prompt: &str,
        request: &GenerateRequest,
        deadline: Option<Instant>,
        mut stream_to: Option<&mut dyn FnMut(&str)>,
    ) -> Result<MlxRun, LlmError> {
        touch(&self.home);
        let read_timeout = deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
            .unwrap_or(Duration::from_secs(3600))
            .max(Duration::from_millis(100));
        let agent = ureq::AgentBuilder::new().timeout_read(read_timeout).build();
        let body = serde_json::json!({
            "model": self.model_ref,
            "messages": [{"role": "user", "content": prompt}],
            "max_tokens": request.config.max_tokens,
            "temperature": request.config.temperature,
            "stream": true,
            "stream_options": {"include_usage": true},
        });
        let response = agent
            .post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| LlmError::Generate(format!("mlx server request failed: {e}")))?;

        let mut text = String::new();
        let mut delta_count: u32 = 0;
        let mut usage_tokens: Option<u32> = None;
        let mut finish: Option<String> = None;
        let mut timed_out = false;
        let reader = BufReader::new(response.into_reader());
        for line in reader.lines() {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                timed_out = true;
                break;
            }
            let line = match line {
                Ok(line) => line,
                // A read timeout mid-stream is the deadline firing.
                Err(_) => {
                    timed_out = true;
                    break;
                }
            };
            match parse_sse_line(&line) {
                SseEvent::Done => break,
                SseEvent::Skip => {}
                SseEvent::Chunk {
                    content,
                    finish_reason,
                    usage_completion_tokens,
                } => {
                    if let Some(piece) = content {
                        if !piece.is_empty() {
                            if let Some(stream) = stream_to.as_deref_mut() {
                                stream(&piece);
                            }
                            text.push_str(&piece);
                            delta_count += 1;
                        }
                    }
                    if finish_reason.is_some() {
                        finish = finish_reason;
                    }
                    if usage_completion_tokens.is_some() {
                        usage_tokens = usage_completion_tokens;
                    }
                }
            }
        }
        touch(&self.home);
        Ok(MlxRun {
            body: text.trim().to_string(),
            token_count: usage_tokens.unwrap_or(delta_count),
            hit_token_budget: finish.as_deref() == Some("length"),
            timed_out,
        })
    }

    /// One-shot `mlx_lm.generate` subprocess (the pre-residency path).
    fn run_once(
        &self,
        prompt: &str,
        request: &GenerateRequest,
        deadline: Option<Instant>,
        stream_to: Option<&mut dyn FnMut(&str)>,
    ) -> Result<MlxRun, LlmError> {
        let mut command = Command::new(&self.runtime.generate_bin);
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
            .arg(request.config.seed.to_string())
            // Qwen-family templates default to a thinking preamble that eats
            // the token budget before any answer; templates without the flag
            // ignore the unused variable.
            .arg("--chat-template-config")
            .arg(r#"{"enable_thinking": false}"#);
        let (stdout, timed_out) = run_with_deadline(command, &self.runtime.generate_bin, deadline)?;
        let mut run = parse_mlx_output(&stdout)?;
        run.timed_out = timed_out;
        if let Some(stream) = stream_to {
            if !run.body.is_empty() {
                stream(&run.body);
            }
        }
        Ok(run)
    }
}

fn resident_enabled() -> bool {
    std::env::var("TERRANE_MLX_RESIDENT")
        .map(|raw| raw.trim() != "0")
        .unwrap_or(true)
}

struct MlxRun {
    body: String,
    token_count: u32,
    hit_token_budget: bool,
    timed_out: bool,
}

impl LocalLlm for MlxBackend {
    fn generate(
        &mut self,
        request: &GenerateRequest,
        on_token: &mut dyn FnMut(&str),
    ) -> Result<GenerateResponse, LlmError> {
        let started = Instant::now();
        let deadline = request.config.timeout.map(|budget| started + budget);

        let (run, body) = match &request.constraint {
            None => {
                let run = self.run_transport(&request.prompt, request, deadline, Some(on_token))?;
                let body = run.body.clone();
                (run, body)
            }
            Some(Constraint::Gbnf(_)) => {
                return Err(LlmError::Constraint(
                    "gbnf grammars are llama_cpp-only; use a JSON schema on mlx".into(),
                ));
            }
            Some(Constraint::JsonSchema(schema)) => {
                // Prompt-guided typed output with one corrective retry. Output
                // is collected (not streamed) because the recorded text is the
                // extracted JSON, not the raw body.
                let guided = format!(
                    "{}\n\nRespond with ONLY a single JSON object (no prose, no code fences) \
                     that matches this JSON schema:\n{schema}",
                    request.prompt
                );
                let first = self.run_transport(&guided, request, deadline, None)?;
                match extract_json_object(&first.body) {
                    Some(json) => {
                        on_token(&json);
                        (first, json)
                    }
                    None if first.timed_out => {
                        let body = first.body.clone();
                        (first, body)
                    }
                    None => {
                        let retry_prompt = format!(
                            "{guided}\n\nYour previous reply was not a valid JSON object. \
                             Reply again with ONLY the JSON object."
                        );
                        let mut second =
                            self.run_transport(&retry_prompt, request, deadline, None)?;
                        second.token_count = first.token_count.saturating_add(second.token_count);
                        match extract_json_object(&second.body) {
                            Some(json) => {
                                on_token(&json);
                                (second, json)
                            }
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

        let stop = if run.timed_out {
            StopReason::DeadlineExceeded
        } else if run.hit_token_budget || run.token_count >= request.config.max_tokens {
            StopReason::MaxTokens
        } else {
            StopReason::Eos
        };
        Ok(GenerateResponse {
            text: body,
            token_count: run.token_count,
            duration: started.elapsed(),
            stop,
        })
    }
}

enum SseEvent {
    Chunk {
        content: Option<String>,
        finish_reason: Option<String>,
        usage_completion_tokens: Option<u32>,
    },
    Done,
    Skip,
}

/// One SSE line from `mlx_lm.server`: `data: {chunk}`, `data: [DONE]`,
/// keepalive comments starting with `:`, or blank separators.
fn parse_sse_line(line: &str) -> SseEvent {
    let trimmed = line.trim();
    let Some(payload) = trimmed.strip_prefix("data:") else {
        return SseEvent::Skip; // keepalive comment, blank, or unknown field
    };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return SseEvent::Done;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return SseEvent::Skip;
    };
    let choice = &value["choices"][0];
    SseEvent::Chunk {
        content: choice["delta"]["content"].as_str().map(str::to_string),
        finish_reason: choice["finish_reason"].as_str().map(str::to_string),
        usage_completion_tokens: value["usage"]["completion_tokens"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok()),
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
        hit_token_budget: false,
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

#[cfg(test)]
pub(crate) fn parse_sse_line_for_tests(
    line: &str,
) -> (Option<String>, Option<String>, Option<u32>, bool, bool) {
    match parse_sse_line(line) {
        SseEvent::Chunk {
            content,
            finish_reason,
            usage_completion_tokens,
        } => (
            content,
            finish_reason,
            usage_completion_tokens,
            false,
            false,
        ),
        SseEvent::Done => (None, None, None, true, false),
        SseEvent::Skip => (None, None, None, false, true),
    }
}
