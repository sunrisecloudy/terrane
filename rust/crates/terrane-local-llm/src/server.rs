//! The resident MLX server — `mlx_lm.server` kept warm behind Rust.
//!
//! Lifecycle is "best of both worlds": the first request auto-starts a
//! detached server (one per `$TERRANE_HOME`, shared by every host); a shell
//! watchdog kills it after an idle window and then exits itself, so an idle
//! machine has **zero** resident processes; the next request auto-restarts it.
//! Lifecycle state is pure edge plumbing — nothing here touches the event log.

use std::fs;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::setup::{engines_dir, MlxRuntime};
use crate::LlmError;

/// How long an unused server stays resident (override:
/// `TERRANE_MLX_IDLE_MS`).
const DEFAULT_IDLE: Duration = Duration::from_secs(600);
/// How long to wait for a spawned server to answer HTTP.
const STARTUP_WAIT: Duration = Duration::from_secs(60);

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
struct ServerState {
    pid: u32,
    port: u16,
    started_unix: u64,
}

/// What `server status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlxServerStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub port: Option<u16>,
    pub idle_secs: Option<u64>,
    pub models: Vec<String>,
}

fn state_path(home: &Path) -> PathBuf {
    engines_dir(home).join("mlx-server.json")
}

fn touch_path(home: &Path) -> PathBuf {
    engines_dir(home).join("mlx-server.touch")
}

fn log_path(home: &Path) -> PathBuf {
    engines_dir(home).join("mlx-server.log")
}

pub(crate) fn idle_limit() -> Duration {
    std::env::var("TERRANE_MLX_IDLE_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_IDLE)
}

/// Mark the server as just-used (the watchdog reads this file's mtime).
pub(crate) fn touch(home: &Path) {
    let path = touch_path(home);
    if fs::write(&path, b"").is_err() {
        // Missing engines dir — nothing resident to keep alive anyway.
        let _ = fs::create_dir_all(engines_dir(home)).and_then(|()| fs::write(&path, b""));
    }
}

/// Return a healthy server's port, starting one (plus its watchdog) if needed.
pub(crate) fn ensure_server(home: &Path, runtime: &MlxRuntime) -> Result<u16, LlmError> {
    if let Some(state) = read_state(home) {
        if probe_models(state.port).is_some() {
            touch(home);
            return Ok(state.port);
        }
        // Stale record (crashed or idle-killed): clean and respawn.
        clear_state(home);
    }

    let engines = engines_dir(home);
    fs::create_dir_all(&engines)
        .map_err(|e| LlmError::Generate(format!("cannot create {}: {e}", engines.display())))?;
    let port = free_port()?;
    let log = fs::File::create(log_path(home))
        .map_err(|e| LlmError::Generate(format!("cannot create server log: {e}")))?;
    let log_err = log
        .try_clone()
        .map_err(|e| LlmError::Generate(format!("cannot clone server log handle: {e}")))?;

    let mut command = Command::new(&runtime.server_bin);
    command
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        // Qwen-family templates default to a thinking preamble that eats the
        // token budget; templates without the flag ignore the unused variable.
        .arg("--chat-template-args")
        .arg(r#"{"enable_thinking": false}"#)
        .arg("--log-level")
        .arg("WARNING")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    detach(&mut command);
    let child = command.spawn().map_err(|e| {
        LlmError::Generate(format!(
            "failed to start `{}`: {e}; run `terrane local-model setup mlx` first",
            runtime.server_bin
        ))
    })?;
    let pid = child.id();

    // Wait for HTTP to come up (model weights load lazily, per request).
    let deadline = Instant::now() + STARTUP_WAIT;
    while probe_models(port).is_none() {
        if Instant::now() >= deadline {
            return Err(LlmError::Generate(format!(
                "mlx server did not come up on 127.0.0.1:{port} within {STARTUP_WAIT:?}; see {}",
                log_path(home).display()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    write_state(
        home,
        &ServerState {
            pid,
            port,
            started_unix: unix_now(),
        },
    )?;
    touch(home);
    spawn_watchdog(home, pid)?;
    Ok(port)
}

pub fn server_status(home: &Path) -> MlxServerStatus {
    let Some(state) = read_state(home) else {
        return MlxServerStatus {
            running: false,
            pid: None,
            port: None,
            idle_secs: None,
            models: Vec::new(),
        };
    };
    let models = probe_models(state.port);
    let running = models.is_some();
    let idle_secs = fs::metadata(touch_path(home))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|idle| idle.as_secs());
    MlxServerStatus {
        running,
        pid: running.then_some(state.pid),
        port: running.then_some(state.port),
        idle_secs: running.then_some(idle_secs.unwrap_or(0)),
        models: models.unwrap_or_default(),
    }
}

/// Kill the resident server if any. Returns whether one was stopped.
pub fn stop_server(home: &Path) -> Result<bool, LlmError> {
    let Some(state) = read_state(home) else {
        return Ok(false);
    };
    let was_running = probe_models(state.port).is_some();
    // SAFETY: plain kill(2) on a recorded pid; a recycled pid would receive a
    // spurious SIGTERM, which the stale-state probe above makes unlikely.
    unsafe {
        libc::kill(state.pid as i32, libc::SIGTERM);
    }
    clear_state(home);
    Ok(was_running)
}

/// A detached `sh` loop: kill the server once the touch file has been idle
/// past the limit, then exit. Exits on its own when the server dies first, so
/// nothing stays resident after an idle shutdown.
fn spawn_watchdog(home: &Path, pid: u32) -> Result<(), LlmError> {
    let idle_secs = idle_limit().as_secs().max(1);
    let interval = (idle_secs / 3).clamp(1, 30);
    let script = r#"
T="$1"; S="$2"; P="$3"; I="$4"
while :; do
  sleep "$5"
  kill -0 "$P" 2>/dev/null || { rm -f "$T" "$S"; exit 0; }
  [ -f "$T" ] || { kill "$P" 2>/dev/null; rm -f "$S"; exit 0; }
  m=$(stat -f %m "$T" 2>/dev/null || stat -c %Y "$T" 2>/dev/null) || exit 0
  age=$(( $(date +%s) - m ))
  if [ "$age" -gt "$I" ]; then kill "$P" 2>/dev/null; rm -f "$T" "$S"; exit 0; fi
done
"#;
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(script)
        .arg("mlx-watchdog")
        .arg(touch_path(home))
        .arg(state_path(home))
        .arg(pid.to_string())
        .arg(idle_secs.to_string())
        .arg(interval.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map_err(|e| LlmError::Generate(format!("failed to start idle watchdog: {e}")))?;
    Ok(())
}

/// Detach a child from our process group so it survives CLI exit.
fn detach(command: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }
}

/// `GET /v1/models` as the health probe; returns the served model ids.
pub(crate) fn probe_models(port: u16) -> Option<Vec<String>> {
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(2))
        .build();
    let response = agent
        .get(&format!("http://127.0.0.1:{port}/v1/models"))
        .call()
        .ok()?;
    let raw = response.into_string().ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(
        value["data"]
            .as_array()
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m["id"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

fn free_port() -> Result<u16, LlmError> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| LlmError::Generate(format!("cannot pick a local port: {e}")))?;
    let port = listener
        .local_addr()
        .map_err(|e| LlmError::Generate(format!("cannot read local port: {e}")))?
        .port();
    drop(listener);
    Ok(port)
}

fn read_state(home: &Path) -> Option<ServerState> {
    let raw = fs::read_to_string(state_path(home)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn write_state(home: &Path, state: &ServerState) -> Result<(), LlmError> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| LlmError::Generate(format!("server state encode failed: {e}")))?;
    fs::write(state_path(home), raw)
        .map_err(|e| LlmError::Generate(format!("server state write failed: {e}")))
}

fn clear_state(home: &Path) {
    let _ = fs::remove_file(state_path(home));
    let _ = fs::remove_file(touch_path(home));
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
