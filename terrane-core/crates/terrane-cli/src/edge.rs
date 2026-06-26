//! The CLI's real [`EffectRunner`] — where the engine's effects meet the world.
//!
//! It performs each [`Effect`] at the edge and hands the result back as the
//! owning capability's recorded event. Replay never calls this. Two effects so
//! far: a minimal `http://` GET (`net`) and an agent-CLI call (`model`).

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::Command;

use terrane_core::cap::model::responded_event;
use terrane_core::cap::net::fetched_event;
use terrane_core::{Effect, EffectRunner};
use terrane_domain::{Error, EventRecord, Result};

pub struct EdgeRunner;

impl EffectRunner for EdgeRunner {
    fn run(&self, effect: &Effect) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                let (status, body) = http_get(url)?;
                Ok(vec![fetched_event(app, url, status, body)?])
            }
            Effect::ModelCall { app, agent, prompt } => {
                let (response, exit_code) = run_agent(agent, prompt)?;
                Ok(vec![responded_event(app, agent, prompt, response, exit_code)?])
            }
        }
    }
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

    let output = command.output().map_err(|e| {
        Error::Storage(format!("failed to run `{agent}` (is it installed and on PATH?): {e}"))
    })?;
    let exit_code = output.status.code().unwrap_or(-1);
    let mut response = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !stderr.trim().is_empty() {
            response.push_str("\n[stderr] ");
            response.push_str(stderr.trim_end());
        }
    }
    Ok((response, exit_code))
}

fn http_get(url: &str) -> Result<(u16, String)> {
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        Error::InvalidInput(format!("the built-in runner supports only http:// URLs: {url}"))
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

    let mut stream = TcpStream::connect((host, port)).map_err(|e| Error::Storage(e.to_string()))?;
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
