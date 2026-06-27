//! Networked sync — direct peer-to-peer over TCP.
//!
//! One instance runs [`run_serve`] and listens; another runs [`run_sync_peer`]
//! and connects. Per app they exchange version vectors and the deltas each side
//! lacks — the same conflict-free, idempotent merge as the file-based `--from`
//! path ([`crate::run_sync`]), just over a socket instead of a log file. Raw TCP
//! mirrors the `net` capability's edge.
//!
//! Wire protocol (one app per connection), each message a `u32`-LE length prefix
//! then that many bytes:
//!
//! ```text
//! client → server:  app name | client version vector
//! server → client:  delta the client lacks | server version vector
//! client → server:  delta the server lacks
//! ```
//!
//! Both sides merge what they receive and converge on deltas only.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

use terrane_core::cap::crdt::{crdt_export_from_vv, crdt_vv, to_hex};
use terrane_core::Core;
use terrane_domain::Request;

use crate::{ensure_identity, open, EdgeRunner};

/// Cap on a single framed message, so a malformed/hostile length can't make us
/// allocate unbounded memory.
const MAX_FRAME: usize = 64 * 1024 * 1024;

/// Serve one connection against `core`: send the peer the ops it lacks, then
/// merge the ops it sends back. Sequential — one connection mutates `core` at a
/// time. Exposed for in-process tests over a real socket.
pub fn serve_conn(core: &mut Core<EdgeRunner>, stream: &mut TcpStream) -> Result<(), String> {
    let app =
        String::from_utf8(read_frame(stream)?).map_err(|_| "sync: bad app name".to_string())?;
    let client_vv = read_frame(stream)?;

    let delta = crdt_export_from_vv(core.state(), &app, &client_vv).map_err(|e| e.to_string())?;
    write_frame(stream, &delta)?;
    write_frame(stream, &crdt_vv(core.state(), &app))?;

    let client_delta = read_frame(stream)?;
    if !client_delta.is_empty() {
        merge(core, &app, &client_delta)?;
    }
    Ok(())
}

/// Drive the client side of one sync against `core` over `stream`. Returns
/// whether we merged anything new. Exposed for in-process tests.
pub fn sync_conn(
    core: &mut Core<EdgeRunner>,
    app: &str,
    stream: &mut TcpStream,
) -> Result<bool, String> {
    write_frame(stream, app.as_bytes())?;
    write_frame(stream, &crdt_vv(core.state(), app))?;

    let server_delta = read_frame(stream)?;
    let server_vv = read_frame(stream)?;

    let mut changed = false;
    if !server_delta.is_empty() {
        changed = merge(core, app, &server_delta)?;
    }
    // Send the server exactly what it's missing.
    let our_delta =
        crdt_export_from_vv(core.state(), app, &server_vv).map_err(|e| e.to_string())?;
    write_frame(stream, &our_delta)?;
    Ok(changed)
}

/// `terrane serve [--addr <addr>]`: listen and sync each incoming connection
/// against this home. Blocks until interrupted.
pub fn run_serve(addr: &str) -> Result<(), String> {
    let mut core = open()?;
    ensure_identity(&mut core)?;
    let listener = TcpListener::bind(addr).map_err(|e| format!("serve bind {addr}: {e}"))?;
    let local = listener.local_addr().map_err(io)?;
    println!("terrane serve: listening on {local} (ctrl-c to stop)");
    for stream in listener.incoming() {
        let mut stream = stream.map_err(io)?;
        if let Err(e) = serve_conn(&mut core, &mut stream) {
            eprintln!("sync connection error: {e}");
        }
    }
    Ok(())
}

/// `terrane sync <app> --peer <addr>`: connect to a serving peer and sync `app`.
pub fn run_sync_peer(app: &str, addr: &str) -> Result<(), String> {
    let mut core = open()?;
    ensure_identity(&mut core)?;
    let mut stream = TcpStream::connect(addr).map_err(|e| format!("sync connect {addr}: {e}"))?;
    if sync_conn(&mut core, app, &mut stream)? {
        println!("synced '{app}' from {addr}");
    } else {
        println!("(already up to date with {addr})");
    }
    Ok(())
}

/// Merge a peer's raw update bytes via the standard `crdt.merge` command (so it's
/// recorded + replayable). Returns whether it added anything new.
fn merge(core: &mut Core<EdgeRunner>, app: &str, bytes: &[u8]) -> Result<bool, String> {
    let records = core
        .dispatch(Request::new(
            "crdt.merge",
            vec![app.to_string(), to_hex(bytes)],
        ))
        .map_err(|e| e.to_string())?;
    Ok(!records.is_empty())
}

fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), String> {
    let len = u32::try_from(bytes.len()).map_err(|_| "sync: frame too large".to_string())?;
    stream.write_all(&len.to_le_bytes()).map_err(io)?;
    stream.write_all(bytes).map_err(io)?;
    Ok(())
}

fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len).map_err(io)?;
    let n = u32::from_le_bytes(len) as usize;
    if n > MAX_FRAME {
        return Err(format!("sync: frame too large ({n} bytes)"));
    }
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf).map_err(io)?;
    Ok(buf)
}

fn io(e: std::io::Error) -> String {
    format!("sync io: {e}")
}
