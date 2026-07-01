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
//! One thread owns the (`!Send`) `Core` and runs the event loop. Two front-ends
//! feed it over one channel: a reader thread forwarding stdin lines (the model),
//! and a loopback admin listener forwarding approve/deny requests (a human). The
//! single owner keeps the `Core` the sole in-process writer; the exclusive home
//! lock keeps *other* processes out.
//!
//! ## In-session approval
//!
//! Two ways for a human to approve a default-deny resource against the **live**
//! Core, so the model continues with no restart:
//! - **Elicitation** (in the model's client): on a `permission_required` result
//!   the loop sends `elicitation/create` up the stdio channel and, on approval,
//!   grants in-process (trusted) and retries.
//! - **Admin console** (loopback HTTP): a browser/curl/headless operator approves
//!   against the same live Core — for any client, even without elicitation.
//!
//! The untrusted model never gains a grant tool; approval is always a human act.

mod admin;

use std::io::{self, BufRead, Write};
use std::net::TcpListener;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use nanoserde::SerJson;
use terrane_host::mcp;
use terrane_host::permission::{
    approve_permission_request, deny_permission_request, permission_requests, DEFAULT_ADMIN_BASE_URL,
};
use terrane_host::HostCore;

const DEFAULT_ELICIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_ADMIN_ADDR: &str = "127.0.0.1:8780";

/// A message reaching the Core-owning event loop.
enum Incoming {
    Line(String),
    Admin(AdminMsg),
    Closed,
}

/// An admin operation plus the channel to send its HTTP response back on.
struct AdminMsg {
    op: admin::AdminOp,
    reply: Sender<AdminResponse>,
}

struct AdminResponse {
    status: u16,
    body: String,
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
    if let Some(addr) = spawn_admin_listener(tx.clone()) {
        eprintln!("terrane-mcp: admin console on http://{addr}{}", admin::BASE);
    }
    spawn_stdin_reader(tx.clone());
    drop(tx); // only the front-end threads keep the loop alive.

    let mut stdout = io::stdout();
    let mut client_elicits = false;
    let mut elicit_seq: u64 = 0;
    let timeout = elicit_timeout();

    loop {
        match rx.recv() {
            Ok(Incoming::Line(raw)) => {
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
                        match await_decision(&mut core, &rx, &mut stdout, &elicit_id, timeout) {
                            Some(mcp::ElicitDecision::Approve) => {
                                // Grant in-process (trusted) against the live Core,
                                // then retry — the model continues unbroken.
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
                                // Timed out or stream closed: fall back to the
                                // documented permission_required poll flow.
                                write_line(&mut stdout, &response);
                            }
                        }
                        continue;
                    }
                }

                write_line(&mut stdout, &response);
            }
            Ok(Incoming::Admin(msg)) => handle_admin(&mut core, msg),
            Ok(Incoming::Closed) | Err(_) => break,
        }
    }
}

fn spawn_stdin_reader(tx: Sender<Incoming>) {
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
}

/// Bind the loopback admin listener (unless `TERRANE_ADMIN_ADDR` is `off`/empty)
/// and spawn its accept loop. Returns the bound address. A bind failure is
/// non-fatal: the server keeps running with elicitation as the only approval path.
fn spawn_admin_listener(tx: Sender<Incoming>) -> Option<String> {
    let addr =
        std::env::var("TERRANE_ADMIN_ADDR").unwrap_or_else(|_| DEFAULT_ADMIN_ADDR.to_string());
    if addr.is_empty() || addr.eq_ignore_ascii_case("off") {
        return None;
    }
    let listener = match TcpListener::bind(&addr) {
        Ok(listener) => listener,
        Err(e) => {
            eprintln!("terrane-mcp: admin console disabled ({addr}: {e})");
            return None;
        }
    };
    let bound = listener
        .local_addr()
        .map(|a| a.to_string())
        .unwrap_or(addr);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let op = match admin::parse_request(&stream) {
                Some(op) => op,
                None => {
                    let _ = admin::write_response(&mut stream, 400, r#"{"error":"bad request"}"#);
                    continue;
                }
            };
            let (reply_tx, reply_rx) = mpsc::channel();
            if tx.send(Incoming::Admin(AdminMsg { op, reply: reply_tx })).is_err() {
                break; // event loop gone.
            }
            let (status, body) = match reply_rx.recv() {
                Ok(resp) => (resp.status, resp.body),
                Err(_) => (500, r#"{"error":"core unavailable"}"#.to_string()),
            };
            let _ = admin::write_response(&mut stream, status, &body);
        }
    });
    Some(bound)
}

/// Run one admin operation against the live Core (trusted, in-process) and reply.
fn handle_admin(core: &mut HostCore, msg: AdminMsg) {
    use admin::AdminOp;
    let base = DEFAULT_ADMIN_BASE_URL;
    let (status, body) = match msg.op {
        AdminOp::ListRequests => match permission_requests(core, base) {
            Ok(list) => (200, list.serialize_json()),
            Err(e) => (500, error_json(&e)),
        },
        AdminOp::Approve { id } => {
            match approve_permission_request(core, &id, "approved via admin console", base) {
                Ok(Some(view)) => (200, view.serialize_json()),
                Ok(None) => (404, error_json("permission request not found")),
                Err(e) => (400, error_json(&e)),
            }
        }
        AdminOp::Deny { id } => {
            match deny_permission_request(core, &id, "denied via admin console", base) {
                Ok(Some(view)) => (200, view.serialize_json()),
                Ok(None) => (404, error_json("permission request not found")),
                Err(e) => (400, error_json(&e)),
            }
        }
        AdminOp::NotFound => (404, error_json("not found")),
    };
    let _ = msg.reply.send(AdminResponse { status, body });
}

fn error_json(message: &str) -> String {
    format!(r#"{{"error":{}}}"#, message.to_string().serialize_json())
}

/// Wait for the human's decision on `elicit_id`. Stays responsive: admin console
/// requests are still serviced against the live Core, an unrelated stdio request
/// gets a "busy" error, a notification is ignored. `None` on timeout or close.
fn await_decision(
    core: &mut HostCore,
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
            Ok(Incoming::Admin(msg)) => handle_admin(core, msg),
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
