//! The resident MLX worker — Terrane's own serving layer over the bare
//! `mlx_lm` generation loop.
//!
//! `mlx_lm.server` routes requests through a continuous-batching engine that
//! decodes ~2.5× slower at batch size 1, so Terrane doesn't use it: Rust owns
//! the protocol (newline-delimited JSON over a Unix socket), the lifecycle,
//! and the timeouts, and the Python side is a ~100-line shim
//! ([`WORKER_PY`], written to `engines/mlx-worker.py`) that only calls
//! `mlx_lm.stream_generate` — the exact loop `mlx_lm.generate` runs.
//!
//! Lifecycle is "best of both worlds": the first request auto-starts a
//! detached worker (one per `$TERRANE_HOME`, shared by every host); a shell
//! watchdog kills it after an idle window and then exits itself, so an idle
//! machine has **zero** resident processes; the next request auto-restarts it.
//! Lifecycle state is pure edge plumbing — nothing here touches the event log.

use std::fs;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::time::Instant;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::setup::{engines_dir, MlxRuntime};
use crate::LlmError;

/// The Python engine shim, kept beside this file and installed into
/// `engines/mlx-worker.py` on every spawn so upgrades propagate.
#[cfg(unix)]
const WORKER_PY: &str = include_str!("mlx_worker.py");

/// How long an unused worker stays resident (override: `TERRANE_MLX_IDLE_MS`).
const DEFAULT_IDLE: Duration = Duration::from_secs(600);
/// How long to wait for a spawned worker to answer a ping.
#[cfg(unix)]
const STARTUP_WAIT: Duration = Duration::from_secs(60);

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
struct WorkerState {
    pid: u32,
    socket: String,
    started_unix: u64,
}

/// Legacy state from the retired `mlx_lm.server` transport; recognized only
/// to kill an orphaned server left by an older build.
#[derive(serde::Deserialize)]
struct LegacyServerState {
    pid: u32,
    port: u16,
}

/// What `server status` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MlxServerStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub socket: Option<String>,
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
    engines_dir(home).join("mlx-worker.log")
}

fn worker_script_path(home: &Path) -> PathBuf {
    engines_dir(home).join("mlx-worker.py")
}

fn socket_path(home: &Path) -> PathBuf {
    engines_dir(home).join("mlx-worker.sock")
}

#[cfg(unix)]
pub(crate) fn idle_limit() -> Duration {
    std::env::var("TERRANE_MLX_IDLE_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_IDLE)
}

/// Mark the worker as just-used (the watchdog reads this file's mtime).
pub(crate) fn touch(home: &Path) {
    let path = touch_path(home);
    if fs::write(&path, b"").is_err() {
        // Missing engines dir — nothing resident to keep alive anyway.
        let _ = fs::create_dir_all(engines_dir(home)).and_then(|()| fs::write(&path, b""));
    }
}

/// Return a healthy worker's socket path, starting one (plus its watchdog) if
/// needed.
#[cfg(unix)]
pub(crate) fn ensure_worker(home: &Path, runtime: &MlxRuntime) -> Result<PathBuf, LlmError> {
    if let Some(state) = read_state(home) {
        if ping(Path::new(&state.socket)).is_some() {
            touch(home);
            return Ok(PathBuf::from(state.socket));
        }
        // Stale record (crashed or idle-killed): clean and respawn.
        clear_state(home);
    }

    let engines = engines_dir(home);
    fs::create_dir_all(&engines)
        .map_err(|e| LlmError::Generate(format!("cannot create {}: {e}", engines.display())))?;
    let script = worker_script_path(home);
    fs::write(&script, WORKER_PY)
        .map_err(|e| LlmError::Generate(format!("cannot write {}: {e}", script.display())))?;
    let socket = socket_path(home);
    let _ = fs::remove_file(&socket);

    let python = worker_python(runtime);
    let log = fs::File::create(log_path(home))
        .map_err(|e| LlmError::Generate(format!("cannot create worker log: {e}")))?;
    let log_err = log
        .try_clone()
        .map_err(|e| LlmError::Generate(format!("cannot clone worker log handle: {e}")))?;
    let mut command = Command::new(&python);
    command
        .arg(&script)
        .arg(&socket)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    detach(&mut command);
    let child = command.spawn().map_err(|e| {
        LlmError::Generate(format!(
            "failed to start the mlx worker with `{python}`: {e}; run \
             `terrane local-model setup mlx` first"
        ))
    })?;
    let pid = child.id();

    // Wait for the socket to answer (model weights load lazily, per request).
    let deadline = Instant::now() + STARTUP_WAIT;
    while ping(&socket).is_none() {
        if Instant::now() >= deadline {
            return Err(LlmError::Generate(format!(
                "mlx worker did not come up on {} within {STARTUP_WAIT:?}; see {}",
                socket.display(),
                log_path(home).display()
            )));
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    write_state(
        home,
        &WorkerState {
            pid,
            socket: socket.display().to_string(),
            started_unix: unix_now(),
        },
    )?;
    touch(home);
    spawn_watchdog(home, pid)?;
    Ok(socket)
}

/// The Python interpreter that can import `mlx_lm`: the one inside the
/// runtime's own environment (override: `TERRANE_MLX_PYTHON`).
pub(crate) fn worker_python(runtime: &MlxRuntime) -> String {
    if let Ok(python) = std::env::var("TERRANE_MLX_PYTHON") {
        if !python.trim().is_empty() {
            return python;
        }
    }
    // `mlx_lm.generate` is an entry-point script inside a venv whose `bin/`
    // also holds the interpreter; follow symlinks (uv exposes tools through a
    // symlink farm) and prefer a sibling python.
    let entry = resolve_on_path(&runtime.generate_bin);
    if let Some(entry) = entry {
        if let Ok(real) = fs::canonicalize(&entry) {
            if let Some(bin_dir) = real.parent() {
                for name in ["python3", "python"] {
                    let candidate = bin_dir.join(name);
                    if candidate.is_file() {
                        return candidate.display().to_string();
                    }
                }
            }
            // Fall back to the script's shebang interpreter.
            if let Ok(script) = fs::read_to_string(&real) {
                if let Some(interpreter) = script
                    .lines()
                    .next()
                    .and_then(|line| line.strip_prefix("#!"))
                {
                    return interpreter.trim().to_string();
                }
            }
        }
    }
    "python3".to_string()
}

/// Resolve a bare command name against PATH; absolute/relative paths pass
/// through.
fn resolve_on_path(bin: &str) -> Option<PathBuf> {
    let direct = Path::new(bin);
    if direct.components().count() > 1 {
        return direct.exists().then(|| direct.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

pub fn server_status(home: &Path) -> MlxServerStatus {
    let Some(state) = read_state(home) else {
        return MlxServerStatus {
            running: false,
            pid: None,
            socket: None,
            idle_secs: None,
            models: Vec::new(),
        };
    };
    let models = ping(Path::new(&state.socket));
    let running = models.is_some();
    let idle_secs = fs::metadata(touch_path(home))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok())
        .map(|idle| idle.as_secs());
    MlxServerStatus {
        running,
        pid: running.then_some(state.pid),
        socket: running.then(|| state.socket.clone()),
        idle_secs: running.then_some(idle_secs.unwrap_or(0)),
        models: models.unwrap_or_default(),
    }
}

/// Kill the resident worker if any. Returns whether one was stopped.
pub fn stop_server(home: &Path) -> Result<bool, LlmError> {
    // An older build may have left the retired mlx_lm.server transport
    // running; recognize its state file and put it down too.
    if let Some(legacy) = read_legacy_state(home) {
        let was_running = legacy_probe(legacy.port);
        kill_pid(legacy.pid);
        clear_state(home);
        return Ok(was_running);
    }
    let Some(state) = read_state(home) else {
        return Ok(false);
    };
    let was_running = ping(Path::new(&state.socket)).is_some();
    kill_pid(state.pid);
    clear_state(home);
    Ok(was_running)
}

/// A detached `sh` loop: kill the worker once the touch file has been idle
/// past the limit, then exit. Exits on its own when the worker dies first, so
/// nothing stays resident after an idle shutdown.
#[cfg(unix)]
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
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0);
}

/// Ping the worker socket; returns the loaded model ids when healthy.
#[cfg(unix)]
pub(crate) fn ping(socket: &Path) -> Option<Vec<String>> {
    let stream = UnixStream::connect(socket).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .ok()?;
    let mut writer = stream.try_clone().ok()?;
    writer.write_all(b"{\"ping\": true}\n").ok()?;
    writer.flush().ok()?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let value: serde_json::Value = serde_json::from_str(&line).ok()?;
    if value["pong"].as_bool() != Some(true) {
        return None;
    }
    Some(
        value["models"]
            .as_array()
            .map(|models| {
                models
                    .iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    )
}

/// The MLX worker rides Unix sockets and a `sh` watchdog; other platforms
/// (Windows: compiles-by-design, unvalidated) report it as not running. MLX
/// itself is Apple-silicon-only, so no transport is wired for them.
#[cfg(not(unix))]
pub(crate) fn ping(_socket: &Path) -> Option<Vec<String>> {
    None
}

fn read_state(home: &Path) -> Option<WorkerState> {
    let raw = fs::read_to_string(state_path(home)).ok()?;
    serde_json::from_str(&raw).ok()
}

fn read_legacy_state(home: &Path) -> Option<LegacyServerState> {
    let raw = fs::read_to_string(state_path(home)).ok()?;
    if serde_json::from_str::<WorkerState>(&raw).is_ok() {
        return None;
    }
    serde_json::from_str(&raw).ok()
}

/// Terminate a recorded worker pid. The state file is removed right after
/// each call site, so a recycled pid is signalled at most once.
#[cfg(unix)]
fn kill_pid(pid: u32) {
    // SAFETY: plain kill(2) on a recorded pid.
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

/// Windows: compiles-by-design, unvalidated (no worker is ever spawned there,
/// but an inherited state file from a shared home is still cleaned up).
#[cfg(not(unix))]
fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status();
}

fn legacy_probe(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        Duration::from_millis(500),
    )
    .is_ok()
}

fn write_state(home: &Path, state: &WorkerState) -> Result<(), LlmError> {
    let raw = serde_json::to_string_pretty(state)
        .map_err(|e| LlmError::Generate(format!("worker state encode failed: {e}")))?;
    fs::write(state_path(home), raw)
        .map_err(|e| LlmError::Generate(format!("worker state write failed: {e}")))
}

fn clear_state(home: &Path) {
    let _ = fs::remove_file(state_path(home));
    let _ = fs::remove_file(touch_path(home));
    let _ = fs::remove_file(socket_path(home));
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
