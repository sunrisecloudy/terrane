//! Edge execution for AppleScript — `/usr/bin/osascript` and `osacompile`.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use terrane_core::{Error, Result};

const DEFAULT_TIMEOUT_MS: u64 = 30_000;
const OSASCRIPT: &str = "/usr/bin/osascript";
const OSACOMPILE: &str = "/usr/bin/osacompile";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptOutcome {
    pub ok: bool,
    pub output: String,
    pub error: String,
    pub exit_code: i32,
    pub duration_ms: u64,
}

pub fn run(script: &str) -> Result<ScriptOutcome> {
    ensure_macos_tool(OSASCRIPT)?;
    run_with_stdin(OSASCRIPT, &[], script, "osascript")
}

pub fn check(script: &str) -> Result<ScriptOutcome> {
    ensure_macos_tool(OSACOMPILE)?;
    let mut tempfile = std::env::temp_dir();
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    tempfile.push(format!(
        "terrane-applescript-{}-{}.scpt",
        std::process::id(),
        unique
    ));
    let path = tempfile
        .to_str()
        .ok_or_else(|| Error::Runtime("temp compile path is not valid UTF-8".into()))?
        .to_string();
    let outcome = run_with_stdin(OSACOMPILE, &["-o", &path], script, "osacompile")?;
    let _ = std::fs::remove_file(&tempfile);
    Ok(outcome)
}

fn ensure_macos_tool(path: &str) -> Result<()> {
    if PathBuf::from(path).is_file() {
        return Ok(());
    }
    Err(Error::Runtime(
        "applescript requires macOS (osascript not found)".into(),
    ))
}

fn run_with_stdin(
    program: &str,
    extra_args: &[&str],
    script: &str,
    label: &str,
) -> Result<ScriptOutcome> {
    let timeout = applescript_timeout();
    let started = Instant::now();

    let mut command = Command::new(program);
    command.args(extra_args);
    command.stdin(Stdio::piped());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        command.process_group(0);
    }

    let mut child = command
        .spawn()
        .map_err(|e| Error::Runtime(format!("failed to spawn `{label}`: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(script.as_bytes())
            .map_err(|e| Error::Runtime(format!("failed to write script to `{label}`: {e}")))?;
    }

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Runtime(format!("failed to capture `{label}` stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Runtime(format!("failed to capture `{label}` stderr")))?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let deadline = started + timeout;
    let status = loop {
        match child.try_wait().map_err(|e| Error::Runtime(e.to_string()))? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                kill_child(&mut child);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                let duration_ms = started.elapsed().as_millis() as u64;
                return Ok(ScriptOutcome {
                    ok: false,
                    output: String::new(),
                    error: format!("timed out after {duration_ms}ms"),
                    exit_code: -1,
                    duration_ms,
                });
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| Error::Runtime(format!("failed to join `{label}` stdout reader")))?
        .map_err(Error::Runtime)?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| Error::Runtime(format!("failed to join `{label}` stderr reader")))?
        .map_err(Error::Runtime)?;

    let exit_code = status.code().unwrap_or(-1);
    let duration_ms = started.elapsed().as_millis() as u64;
    Ok(ScriptOutcome {
        ok: status.success(),
        output: trim_line_endings(&stdout),
        error: trim_line_endings(&stderr),
        exit_code,
        duration_ms,
    })
}

fn kill_child(child: &mut std::process::Child) {
    #[cfg(unix)]
    unsafe {
        libc::killpg(child.id() as i32, libc::SIGKILL);
    }
    let _ = child.kill();
}

fn read_pipe(mut pipe: impl Read + Send + 'static) -> std::result::Result<String, String> {
    let mut buf = Vec::new();
    pipe.read_to_end(&mut buf)
        .map_err(|e| format!("read failed: {e}"))?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn trim_line_endings(s: &str) -> String {
    s.trim_end_matches(['\r', '\n']).to_string()
}

fn applescript_timeout() -> Duration {
    std::env::var("TERRANE_APPLESCRIPT_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ms| *ms > 0)
        .map(Duration::from_millis)
        .unwrap_or_else(|| Duration::from_millis(DEFAULT_TIMEOUT_MS))
}
