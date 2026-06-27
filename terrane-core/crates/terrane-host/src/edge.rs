//! The CLI's real [`EffectRunner`] — where the engine's effects meet the world.
//!
//! It performs each [`Effect`] at the edge and hands the result back as the
//! owning capability's recorded event. Replay never calls this. Effects so far:
//! a minimal `http://` GET (`net`), an agent-CLI call (`model`), and minting this
//! home's replica id from OS entropy (`replica`).

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use terrane_core::cap::model::responded_event;
use terrane_core::cap::net::fetched_event;
use terrane_core::cap::replica::initialized_event;
use terrane_core::{Effect, EffectRunner};
use terrane_domain::{Error, EventRecord, Result};

pub struct EdgeRunner;

const DEFAULT_EDGE_TIMEOUT: Duration = Duration::from_secs(30);

impl EffectRunner for EdgeRunner {
    fn run(&self, effect: &Effect) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                let (status, body) = http_get(url)?;
                Ok(vec![fetched_event(app, url, status, body)?])
            }
            Effect::ModelCall { app, agent, prompt } => {
                let (response, exit_code) = run_agent(agent, prompt)?;
                Ok(vec![responded_event(
                    app, agent, prompt, response, exit_code,
                )?])
            }
            Effect::NewReplicaId => Ok(vec![initialized_event(new_peer_id()?)?]),
        }
    }
}

/// Mint a fresh replica PeerID from OS entropy. Masked to 53 bits and forced
/// nonzero — a valid, JS-safe (`Number`-representable) Loro PeerID.
fn new_peer_id() -> Result<u64> {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes)
        .map_err(|e| Error::Storage(format!("failed to read OS entropy for replica id: {e}")))?;
    Ok((u64::from_le_bytes(bytes) & ((1u64 << 53) - 1)) | 1)
}

/// Run an agent CLI non-interactively and capture its output.
/// `claude -p "<prompt>"` (Claude Code print mode) or `codex exec "<prompt>"`.
fn run_agent(agent: &str, prompt: &str) -> Result<(String, i32)> {
    let mut command = match agent {
        "claude" => {
            let mut c = Command::new("claude");
            c.arg("-p").arg(prompt);
            c
        }
        "codex" => {
            let mut c = Command::new("codex");
            c.arg("exec").arg(prompt);
            c
        }
        other => return Err(Error::InvalidInput(format!("unknown agent: {other}"))),
    };

    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| {
        Error::Storage(format!(
            "failed to run `{agent}` (is it installed and on PATH?): {e}"
        ))
    })?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| Error::Storage(format!("failed to capture `{agent}` stdout")))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| Error::Storage(format!("failed to capture `{agent}` stderr")))?;
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    let timeout = edge_timeout();
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| Error::Storage(e.to_string()))?
        {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(Error::Storage(format!(
                    "`{agent}` timed out after {timeout:?}"
                )));
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };

    let stdout = stdout_reader
        .join()
        .map_err(|_| Error::Storage(format!("failed to join `{agent}` stdout reader")))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| Error::Storage(format!("failed to join `{agent}` stderr reader")))??;

    let exit_code = status.code().unwrap_or(-1);
    let mut response = String::from_utf8_lossy(&stdout).into_owned();
    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        if !stderr.trim().is_empty() {
            response.push_str("\n[stderr] ");
            response.push_str(stderr.trim_end());
        }
    }
    Ok((response, exit_code))
}

fn http_get(url: &str) -> Result<(u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        Error::InvalidInput(format!(
            "the built-in runner supports only http:// URLs: {url}"
        ))
    })?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.parse::<u16>()
                .map_err(|_| Error::InvalidInput(format!("bad port in {url}")))?,
        ),
        None => (authority, 80u16),
    };

    let timeout = edge_timeout();
    let addrs: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::Storage(e.to_string()))?
        .collect();
    if addrs.is_empty() {
        return Err(Error::Storage(format!(
            "no socket address resolved for {host}:{port}"
        )));
    }
    let deadline = Instant::now() + timeout;
    let mut last_error = None;
    let mut stream = None;
    for addr in addrs {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match TcpStream::connect_timeout(&addr, remaining) {
            Ok(s) => {
                stream = Some(s);
                break;
            }
            Err(e) => last_error = Some(e),
        }
    }
    let mut stream = stream.ok_or_else(|| {
        Error::Storage(match last_error {
            Some(e) => format!("HTTP connect to {host}:{port} timed out or failed: {e}"),
            None => format!("HTTP connect to {host}:{port} timed out after {timeout:?}"),
        })
    })?;
    stream
        .set_read_timeout(Some(timeout))
        .map_err(|e| Error::Storage(e.to_string()))?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(|e| Error::Storage(e.to_string()))?;
    let request = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| Error::Storage(e.to_string()))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|e| Error::Storage(e.to_string()))?;
    let text = String::from_utf8_lossy(&raw).into_owned();

    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text.as_str(), ""));
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| Error::Storage("malformed HTTP status line".into()))?;
    Ok((status, body.to_string()))
}

fn read_pipe(mut pipe: impl Read) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    pipe.read_to_end(&mut out)
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(out)
}

fn edge_timeout() -> Duration {
    std::env::var("TERRANE_EDGE_TIMEOUT_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|millis| *millis > 0)
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_EDGE_TIMEOUT)
}
