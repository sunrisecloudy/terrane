//! Loopback admin control-plane for `terrane-mcp`.
//!
//! A trusted human operator — a browser console, `curl`, or a headless script —
//! approves or denies permission requests against the server's **live** Core, so
//! the grant is seen with no restart: the same guarantee as elicitation, but
//! available to any MCP client (even one without elicitation) and to out-of-band
//! operators. Bound to loopback only, so it is a same-machine trusted surface.
//!
//! This module owns only the HTTP mechanics (parse a request into an [`AdminOp`],
//! write a JSON response). The wiring — running the op against the Core-owning
//! event loop — lives in `main`, which is the single writer.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;

/// The admin request path prefix, matching `permission::admin_url`.
pub const BASE: &str = "/__terrane/admin/requests";

/// One admin operation parsed from an HTTP request line + path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdminOp {
    ListRequests,
    Approve { id: String },
    Deny { id: String },
    NotFound,
}

/// Route a `(method, path)` to an operation. Only the three approval endpoints
/// are recognized; everything else is [`AdminOp::NotFound`].
pub fn route(method: &str, path: &str) -> AdminOp {
    match method {
        "GET" if path == BASE => AdminOp::ListRequests,
        "POST" => {
            let rest = match path.strip_prefix(BASE).and_then(|r| r.strip_prefix('/')) {
                Some(rest) => rest,
                None => return AdminOp::NotFound,
            };
            if let Some(id) = rest.strip_suffix("/approve") {
                if !id.is_empty() {
                    return AdminOp::Approve { id: id.to_string() };
                }
            }
            if let Some(id) = rest.strip_suffix("/deny") {
                if !id.is_empty() {
                    return AdminOp::Deny { id: id.to_string() };
                }
            }
            AdminOp::NotFound
        }
        _ => AdminOp::NotFound,
    }
}

/// Read one HTTP request from `stream` (request line + headers + drained body)
/// and route it. Returns `None` on a malformed request.
pub fn parse_request(stream: &TcpStream) -> Option<AdminOp> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);

    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    // Drain headers, noting Content-Length so we consume any body (kept bounded).
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 {
            break;
        }
        let trimmed = header.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some((name, value)) = trimmed.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length.min(64 * 1024)];
    if !body.is_empty() {
        reader.read_exact(&mut body).ok()?;
    }

    Some(route(&method, &path))
}

/// Write a minimal `Connection: close` JSON HTTP response.
pub fn write_response(stream: &mut TcpStream, status: u16, body: &str) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
