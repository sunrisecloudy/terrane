//! The CLI's real [`EffectRunner`]: a minimal, dependency-free HTTP runner.
//!
//! It handles plain `http://` URLs only (HTTP/1.0, `Connection: close`) — enough
//! to prove the architecture point: the GET happens here, at the edge, and its
//! result is handed back as an `Event::Fetched` for the core to record. Replay
//! never calls this. Swap in a TLS client (e.g. `ureq`) when https is needed.

use std::io::{Read, Write};
use std::net::TcpStream;

use terrane_core::{Effect, EffectRunner};
use terrane_domain::{Error, Event, Result};

pub struct HttpGetRunner;

impl EffectRunner for HttpGetRunner {
    fn run(&self, effect: &Effect) -> Result<Vec<Event>> {
        match effect {
            Effect::HttpGet { app, url } => {
                let (status, body) = http_get(url)?;
                Ok(vec![Event::Fetched {
                    app: app.clone(),
                    url: url.clone(),
                    status,
                    body,
                }])
            }
        }
    }
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
