//! terrane-mcp — a stdio MCP server exposing this terrane home's apps as tools.
//!
//! A thin host over the `terrane-host` spine, like the CLI and web hosts. It
//! speaks the Model Context Protocol over newline-delimited JSON-RPC on
//! stdin/stdout so an MCP client (e.g. Claude Code, opencode) can **select an
//! app** (`list_apps`) and **act on it** (`invoke`). The tools and their shapes
//! are the contract in [`terrane_api`].
//!
//! ## Architecture
//!
//! One thread owns the (`!Send`) `Core` and runs the event loop; a reader thread
//! forwards stdin lines over a channel. The single owner keeps the `Core` the
//! sole in-process writer — the exclusive home lock keeps *other* processes out.
//!
//! ## In-session approval
//!
//! When an `invoke`/`app_actions` touches a default-deny resource and the client
//! declared the MCP `elicitation` capability, the loop turns the
//! `permission_required` result into a **human** approval prompt on the client's
//! back-channel. On approval it grants **in-process** (trusted) against the live
//! `Core` and retries, so the model continues with no restart. The untrusted
//! model never gains a grant tool; approval is a human action, not a tool call.

use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use terrane_host::mcp;
use terrane_host::permission::{
    approve_permission_request, deny_permission_request, DEFAULT_ADMIN_BASE_URL,
};

const DEFAULT_ELICIT_TIMEOUT_MS: u64 = 120_000;

/// A message reaching the Core-owning event loop.
enum Incoming {
    Line(String),
    Closed,
}

fn main() {
    let mut core = match terrane_host::open() {
        Ok(core) => core,
        Err(e) => {
            eprintln!("terrane-mcp: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "terrane-mcp: ready (home {})",
        terrane_host::log_path().display()
    );

    let (tx, rx) = mpsc::channel::<Incoming>();
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    let _ = tx.send(Incoming::Closed); // EOF — client disconnected.
                    break;
                }
                Ok(_) => {
                    if tx.send(Incoming::Line(std::mem::take(&mut line))).is_err() {
                        break; // event loop gone.
                    }
                }
                Err(e) => {
                    eprintln!("terrane-mcp: read error: {e}");
                    let _ = tx.send(Incoming::Closed);
                    break;
                }
            }
        }
    });

    let mut stdout = io::stdout();
    let mut client_elicits = false;
    let mut elicit_seq: u64 = 0;
    let timeout = elicit_timeout();

    while let Ok(Incoming::Line(raw)) = rx.recv() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }

        // Track whether this client can be asked to approve in-session.
        if mcp::parsed_method(line).as_deref() == Some("initialize") {
            client_elicits = mcp::initialize_declares_elicitation(line);
        }

        let Some(response) = mcp::handle_json_rpc(&mut core, line) else {
            continue; // notification — no reply.
        };

        // Intercept a default-deny wall on an elicitation-capable client.
        if client_elicits {
            if let Some(info) = mcp::permission_required_from_tool_response(&response) {
                elicit_seq += 1;
                let elicit_id = format!("terrane-elicit-{elicit_seq}");
                write_line(&mut stdout, &mcp::elicitation_create_frame(&elicit_id, &info));
                match await_decision(&rx, &mut stdout, &elicit_id, timeout) {
                    Some(mcp::ElicitDecision::Approve) => {
                        // Grant in-process (trusted) against the live Core, then
                        // retry the original call — the model continues unbroken.
                        let _ = approve_permission_request(
                            &mut core,
                            &info.request_id,
                            "approved in session via elicitation",
                            DEFAULT_ADMIN_BASE_URL,
                        );
                        if let Some(retry) = mcp::handle_json_rpc(&mut core, line) {
                            write_line(&mut stdout, &retry);
                        }
                    }
                    Some(mcp::ElicitDecision::Deny) => {
                        let _ = deny_permission_request(
                            &mut core,
                            &info.request_id,
                            "denied in session via elicitation",
                            DEFAULT_ADMIN_BASE_URL,
                        );
                        write_line(&mut stdout, &response);
                    }
                    None => {
                        // Timed out or stream closed: fall back to the documented
                        // permission_required poll flow (request stays pending).
                        write_line(&mut stdout, &response);
                    }
                }
                continue;
            }
        }

        write_line(&mut stdout, &response);
    }
}

/// Wait for the human's decision on `elicit_id`. Stays responsive: an unrelated
/// request gets a "busy" error, a notification is ignored. `None` on timeout or
/// stream close (the caller then falls back to `permission_required`).
fn await_decision(
    rx: &Receiver<Incoming>,
    stdout: &mut io::Stdout,
    elicit_id: &str,
    timeout: Duration,
) -> Option<mcp::ElicitDecision> {
    loop {
        match rx.recv_timeout(timeout) {
            Ok(Incoming::Line(raw)) => {
                let line = raw.trim();
                if line.is_empty() {
                    continue;
                }
                if let Some(decision) = mcp::elicitation_decision(line, elicit_id) {
                    return Some(decision);
                }
                if let Some(busy) = mcp::busy_error(line) {
                    write_line(stdout, &busy);
                }
            }
            Ok(Incoming::Closed) => return None,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
}

fn elicit_timeout() -> Duration {
    let ms = std::env::var("TERRANE_ELICIT_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_ELICIT_TIMEOUT_MS);
    Duration::from_millis(ms)
}

fn write_line(stdout: &mut io::Stdout, msg: &str) {
    // A write failure means the client disconnected; the reader thread will
    // observe EOF and close the loop, so there is nothing to recover here.
    let _ = writeln!(stdout, "{msg}");
    let _ = stdout.flush();
}
