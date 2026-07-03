//! Background assist jobs — the agent loop.
//!
//! The web host serves one request at a time, so a minutes-long agent run
//! cannot live inside the request loop: while it blocked, the agent's own tool
//! calls (which come back as ordinary `POST /mcp` requests) could never be
//! served, and the loop would deadlock. So `POST /__terrane/agents/{id}/assist`
//! starts a worker thread and returns a job id immediately;
//! `POST /__terrane/agents/assist/status` polls it.
//!
//! The worker shells out to the agent's harness (`opencode`) pointed at THIS
//! host's own MCP endpoint. The harness discovers the open app's verbs
//! (`list_apps` → `app_actions`) and drives it (`invoke`) through the single
//! live Core on the main loop — so the app visibly changes, and every mutation
//! still flows through the normal recorded invoke path.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use terrane_cap_agent::AgentDef;

/// How long a single agent run may take before it is killed.
const ASSIST_TIMEOUT: Duration = Duration::from_secs(300);

/// The plan an assist job runs, captured on the main loop before the worker
/// thread starts (it has no Core access of its own).
struct AssistPlan {
    model: String,
    personality: String,
    app: String,
    message: String,
    base_url: String,
    workdir: PathBuf,
}

struct Job {
    handle: Option<JoinHandle<Result<String, String>>>,
    result: Option<Result<String, String>>,
}

pub enum AssistPoll {
    Running,
    Done(String),
    Failed(String),
    Unknown,
}

pub struct AgentJobs {
    jobs: HashMap<String, Job>,
    seq: u64,
}

impl AgentJobs {
    pub fn new() -> Self {
        AgentJobs {
            jobs: HashMap::new(),
            seq: 0,
        }
    }

    /// Start an assist run for `agent` against `app` with the user's `message`.
    /// `base_url` is this host's own address so the harness's MCP tools loop
    /// back through the live Core. Returns the job id to poll.
    pub fn start(
        &mut self,
        agent: &AgentDef,
        app: &str,
        message: &str,
        base_url: &str,
    ) -> Result<String, String> {
        if agent.harness != "opencode" {
            return Err(format!(
                "unsupported harness {:?}; only opencode is wired today",
                agent.harness
            ));
        }
        self.seq += 1;
        let job_id = format!("assist-{}", self.seq);
        let plan = AssistPlan {
            model: agent.model.clone(),
            personality: agent.personality.clone(),
            app: app.to_string(),
            message: message.to_string(),
            base_url: base_url.trim_end_matches('/').to_string(),
            workdir: std::env::temp_dir().join(format!("terrane-{job_id}")),
        };
        let handle = thread::spawn(move || run_assist(plan));
        self.jobs.insert(
            job_id.clone(),
            Job {
                handle: Some(handle),
                result: None,
            },
        );
        Ok(job_id)
    }

    pub fn poll(&mut self, job_id: &str) -> AssistPoll {
        let Some(job) = self.jobs.get_mut(job_id) else {
            return AssistPoll::Unknown;
        };
        if let Some(handle) = job.handle.as_ref() {
            if handle.is_finished() {
                let handle = job.handle.take().expect("handle present");
                job.result = Some(
                    handle
                        .join()
                        .unwrap_or_else(|_| Err("assist worker panicked".to_string())),
                );
            } else {
                return AssistPoll::Running;
            }
        }
        match &job.result {
            Some(Ok(transcript)) => AssistPoll::Done(transcript.clone()),
            Some(Err(e)) => AssistPoll::Failed(e.clone()),
            None => AssistPoll::Running,
        }
    }
}

impl Default for AgentJobs {
    fn default() -> Self {
        Self::new()
    }
}

/// Run one agent turn to completion. Owns its temp workdir and cleans it up.
fn run_assist(plan: AssistPlan) -> Result<String, String> {
    std::fs::create_dir_all(&plan.workdir)
        .map_err(|e| format!("could not create assist workspace: {e}"))?;
    let config = opencode_config(&plan.base_url);
    std::fs::write(plan.workdir.join("opencode.json"), config)
        .map_err(|e| format!("could not write opencode config: {e}"))?;

    let prompt = assist_prompt(&plan.personality, &plan.app, &plan.message);
    let result = run_opencode(&plan.workdir, &plan.model, &prompt);
    let _ = std::fs::remove_dir_all(&plan.workdir);
    result
}

/// An opencode config that attaches THIS host's MCP endpoint as a tool server,
/// so the harness can discover and drive the open app.
fn opencode_config(base_url: &str) -> String {
    format!(
        r#"{{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {{
    "terrane": {{
      "type": "remote",
      "url": "{base_url}/mcp",
      "enabled": true
    }}
  }}
}}
"#
    )
}

/// The standing instruction handed to the harness: the agent's personality plus
/// how to drive the one app it is scoped to via the terrane MCP tools.
fn assist_prompt(personality: &str, app: &str, message: &str) -> String {
    let persona = if personality.trim().is_empty() {
        "You are a helpful Terrane assistant.".to_string()
    } else {
        personality.trim().to_string()
    };
    format!(
        "{persona}\n\n\
You are helping the user with a running Terrane app whose id is \"{app}\". \
You have MCP tools from the \"terrane\" server. To help, first call app_actions \
with app=\"{app}\" to see the app's verbs, then call the invoke tool \
(app=\"{app}\", verb, args) to actually change the app. Only ever act on the app \
\"{app}\" — never build, register, or modify other apps. Take real actions with \
the tools rather than only describing them, then finish with one short sentence \
saying what you did.\n\n\
User request: {message}"
    )
}

fn run_opencode(workdir: &Path, model: &str, prompt: &str) -> Result<String, String> {
    let mut command = Command::new("opencode");
    command
        .arg("run")
        .arg("--dir")
        .arg(workdir)
        .arg("-m")
        .arg(model)
        .arg(prompt)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(|e| {
        format!("could not launch opencode (is it installed and on PATH?): {e}")
    })?;

    let mut stdout_pipe = child
        .stdout
        .take()
        .ok_or_else(|| "could not capture opencode stdout".to_string())?;
    let mut stderr_pipe = child
        .stderr
        .take()
        .ok_or_else(|| "could not capture opencode stderr".to_string())?;
    let stdout_reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout_pipe.read_to_string(&mut buf);
        buf
    });
    let stderr_reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        buf
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > ASSIST_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "the agent timed out after {}s",
                        ASSIST_TIMEOUT.as_secs()
                    ));
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(format!("waiting on opencode failed: {e}")),
        }
    };

    let stdout = stdout_reader.join().unwrap_or_default();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        let transcript = clean_transcript(&stdout);
        if transcript.is_empty() {
            Ok("The agent finished without a message.".to_string())
        } else {
            Ok(transcript)
        }
    } else {
        let detail = if stderr.trim().is_empty() {
            clean_transcript(&stdout)
        } else {
            clean_transcript(&stderr)
        };
        Err(format!(
            "the agent stopped (exit {}): {}",
            status.code().unwrap_or(-1),
            truncate(&detail, 1200)
        ))
    }
}

/// Strip ANSI escape sequences and trim, then cap the length so the panel keeps
/// a readable summary rather than a wall of harness output.
fn clean_transcript(raw: &str) -> String {
    let stripped = strip_ansi(raw);
    truncate(stripped.trim(), 4000)
}

/// Remove `ESC[ … <final>` control sequences without pulling in a regex crate.
fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Consume the following CSI/OSC sequence up to its terminator.
            if chars.peek() == Some(&'[') {
                chars.next();
                for esc in chars.by_ref() {
                    if esc.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                // Drop a lone escape and whatever single char follows.
                chars.next();
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}
