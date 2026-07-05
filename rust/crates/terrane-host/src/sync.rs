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

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_crdt::{crdt_export_from_vv, crdt_vv, to_hex};
use terrane_cap_sync::{encode_batch_hex, SyncEnvelope, MAX_SYNC_BATCH_EVENTS};
use terrane_core::Core;
use terrane_core::Request;

use crate::{ensure_identity, home_dir, open, EdgeRunner, HostCore};

/// Cap on a single framed message, so a malformed/hostile length can't make us
/// allocate unbounded memory.
const MAX_FRAME: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct VvResponse {
    pub peer_hex: String,
    pub delta: Vec<u8>,
    pub vv: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EventBatchResponse {
    pub peer_hex: String,
    pub from_seq: u64,
    pub to_seq: u64,
    pub batch_hex: String,
    pub count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlobRef {
    pub name: String,
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PairRequest {
    pub peer_hex: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PairResponse {
    pub peer_hex: String,
    pub display_name: String,
}

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
    if addr.starts_with("http://") || addr.starts_with("https://") {
        return run_sync_http(app, addr, false);
    }
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

pub fn run_sync_http(app: &str, base_url: &str, watch: bool) -> Result<(), String> {
    let mut core = open()?;
    ensure_identity(&mut core)?;
    let mut backoff_ms = 1_000u64;
    loop {
        match sync_http_once(&mut core, app, base_url) {
            Ok(changed) => {
                if changed {
                    println!("synced '{app}' from {base_url}");
                } else {
                    println!("(already up to date with {base_url})");
                }
                if !watch {
                    return Ok(());
                }
                backoff_ms = 1_000;
            }
            Err(err) if watch => {
                eprintln!("sync watch: {err}; retrying in {}s", backoff_ms / 1000);
                std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                backoff_ms = (backoff_ms * 2).min(60_000);
            }
            Err(err) => return Err(err),
        }
        if watch {
            wait_http_once(app, base_url)?;
        }
    }
}

pub fn run_pair_http(base_url: &str, code: &str) -> Result<(), String> {
    let _ = code;
    let mut core = open()?;
    ensure_identity(&mut core)?;
    let peer_hex = local_peer_hex(&core)?;
    let display_name = local_display_name();
    let request = PairRequest {
        peer_hex: peer_hex.clone(),
        display_name: display_name.clone(),
    };
    let response = http_post_borsh::<PairResponse>(
        &format!("{}/sync/pair", base_url.trim_end_matches('/')),
        borsh::to_vec(&request).map_err(|e| e.to_string())?,
    )?;
    pair_peer(&mut core, &response.peer_hex, &response.display_name)?;
    println!("paired {} ({})", response.peer_hex, response.display_name);
    Ok(())
}

pub fn pair_request(core: &mut HostCore, request: PairRequest) -> Result<PairResponse, String> {
    pair_peer(core, &request.peer_hex, &request.display_name)?;
    Ok(PairResponse {
        peer_hex: local_peer_hex(core)?,
        display_name: local_display_name(),
    })
}

pub fn pair_peer(core: &mut HostCore, peer_hex: &str, display_name: &str) -> Result<(), String> {
    core.dispatch(Request::new(
        "sync.pair",
        vec![peer_hex.to_string(), display_name.to_string()],
    ))
    .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn sync_http_once(core: &mut HostCore, app: &str, base_url: &str) -> Result<bool, String> {
    let base = base_url.trim_end_matches('/');
    let vv_response = http_post_borsh::<VvResponse>(
        &format!("{base}/sync/{app}/vv"),
        crdt_vv(core.state(), app),
    )?;
    let mut changed = false;
    if !vv_response.delta.is_empty() {
        changed |= merge(core, app, &vv_response.delta)?;
    }
    let our_delta =
        crdt_export_from_vv(core.state(), app, &vv_response.vv).map_err(|e| e.to_string())?;
    http_post_empty(&format!("{base}/sync/{app}/delta"), our_delta)?;

    let cursor = match core
        .query("sync", "cursor", &[vv_response.peer_hex.clone(), app.to_string()])
        .map_err(|e| e.to_string())?
    {
        terrane_core::QueryValue::U64(Some(value)) => value,
        _ => 0,
    };
    let events = http_post_borsh::<EventBatchResponse>(
        &format!("{base}/sync/{app}/events"),
        cursor.to_le_bytes().to_vec(),
    )?;
    changed |= apply_event_batch(core, app, &events)?;

    let local_peer = local_peer_hex(core)?;
    let server_cursor = http_get_u64(&format!("{base}/sync/{app}/cursor/{local_peer}"))?;
    let local_events = event_batch_since(core, app, server_cursor)?;
    http_post_empty(
        &format!("{base}/sync/{app}/apply-events"),
        borsh::to_vec(&local_events).map_err(|e| e.to_string())?,
    )?;
    let refs = http_get_borsh::<Vec<BlobRef>>(&format!("{base}/sync/{app}/blobs"))?;
    changed |= apply_blob_refs(core, app, &refs)?;
    copy_missing_blobs_http(core, app, base)?;
    Ok(changed || local_events.count > 0)
}

pub fn local_peer_hex(core: &HostCore) -> Result<String, String> {
    let peer = core
        .state()
        .replica
        .peer
        .ok_or_else(|| "sync: local replica identity missing".to_string())?;
    Ok(format!("{peer:x}"))
}

pub fn vv_response(core: &HostCore, app: &str, peer_vv: &[u8]) -> Result<VvResponse, String> {
    Ok(VvResponse {
        peer_hex: local_peer_hex(core)?,
        delta: crdt_export_from_vv(core.state(), app, peer_vv).map_err(|e| e.to_string())?,
        vv: crdt_vv(core.state(), app),
    })
}

pub fn vv_response_for_grantee(
    core: &HostCore,
    app: &str,
    grantee: &str,
    peer_vv: &[u8],
) -> Result<VvResponse, String> {
    crate::share::ensure_read(core, app, grantee)?;
    vv_response(core, app, peer_vv)
}

pub fn ingest_crdt_delta(core: &mut HostCore, app: &str, bytes: &[u8]) -> Result<bool, String> {
    if bytes.is_empty() {
        return Ok(false);
    }
    merge(core, app, bytes)
}

pub fn ingest_crdt_delta_for_grantee(
    core: &mut HostCore,
    app: &str,
    grantee: &str,
    bytes: &[u8],
) -> Result<bool, String> {
    crate::share::ensure_write(core, app, grantee)?;
    ingest_crdt_delta(core, app, bytes)
}

pub fn event_batch_since(
    core: &HostCore,
    app: &str,
    cursor: u64,
) -> Result<EventBatchResponse, String> {
    let peer_hex = local_peer_hex(core)?;
    let mut envelopes = Vec::new();
    for (index, record) in core.log_records().map_err(|e| e.to_string())?.into_iter().enumerate() {
        let origin_seq = u64::try_from(index + 1).map_err(|_| "sync: log too long".to_string())?;
        if origin_seq <= cursor || !is_sync_event_for_app(&record, app)? {
            continue;
        }
        envelopes.push(SyncEnvelope {
            origin_peer: peer_hex.clone(),
            origin_seq,
            kind: record.kind,
            payload: record.payload,
        });
        if envelopes.len() >= MAX_SYNC_BATCH_EVENTS {
            break;
        }
    }
    let from_seq = envelopes.first().map(|e| e.origin_seq).unwrap_or(cursor);
    let to_seq = envelopes.last().map(|e| e.origin_seq).unwrap_or(cursor);
    let batch_hex = encode_batch_hex(&envelopes).map_err(|e| e.to_string())?;
    let count = u32::try_from(envelopes.len()).map_err(|_| "sync: batch too large".to_string())?;
    Ok(EventBatchResponse {
        peer_hex,
        from_seq,
        to_seq,
        batch_hex,
        count,
    })
}

pub fn event_batch_since_for_grantee(
    core: &HostCore,
    app: &str,
    grantee: &str,
    cursor: u64,
) -> Result<EventBatchResponse, String> {
    crate::share::ensure_read(core, app, grantee)?;
    event_batch_since(core, app, cursor)
}

pub fn apply_event_batch(
    core: &mut HostCore,
    app: &str,
    batch: &EventBatchResponse,
) -> Result<bool, String> {
    if batch.count == 0 {
        return Ok(false);
    }
    let records = core
        .dispatch(Request::new(
            "sync.apply",
            vec![
                batch.peer_hex.clone(),
                app.to_string(),
                batch.from_seq.to_string(),
                batch.to_seq.to_string(),
                batch.batch_hex.clone(),
            ],
        ))
        .map_err(|e| e.to_string())?;
    Ok(!records.is_empty())
}

pub fn apply_event_batch_for_grantee(
    core: &mut HostCore,
    app: &str,
    grantee: &str,
    batch: &EventBatchResponse,
) -> Result<bool, String> {
    crate::share::ensure_write(core, app, grantee)?;
    apply_event_batch(core, app, batch)
}

pub fn blob_bytes(core: &HostCore, hash: &str) -> Result<Vec<u8>, String> {
    let home = core_home(core)?;
    crate::blob_store::read_verified(&home, hash).map_err(|e| e.to_string())
}

pub fn copy_missing_blob(core: &mut HostCore, hash: &str, bytes: &[u8]) -> Result<bool, String> {
    let home = core_home(core)?;
    let before = crate::blob_store::verify_hash(&home, hash).map_err(|e| e.to_string())?;
    crate::blob_store::insert_if_absent(&home, hash, bytes).map_err(|e| e.to_string())?;
    Ok(!matches!(before, crate::blob_store::BlobHealth::Ok { .. }))
}

pub fn blob_refs(core: &HostCore, app: &str) -> Vec<BlobRef> {
    core.state()
        .blob
        .blobs
        .get(app)
        .map(|names| {
            names
                .iter()
                .map(|(name, meta)| BlobRef {
                    name: name.clone(),
                    hash: meta.hash.clone(),
                    size: meta.size,
                    mime: meta.mime.clone(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn blob_refs_for_grantee(core: &HostCore, app: &str, grantee: &str) -> Result<Vec<BlobRef>, String> {
    crate::share::ensure_read(core, app, grantee)?;
    Ok(blob_refs(core, app))
}

pub fn apply_blob_refs(core: &mut HostCore, app: &str, refs: &[BlobRef]) -> Result<bool, String> {
    let mut changed = false;
    for blob in refs {
        let same = core
            .state()
            .blob
            .blobs
            .get(app)
            .and_then(|names| names.get(&blob.name))
            .map(|local| {
                local.hash == blob.hash && local.size == blob.size && local.mime == blob.mime
            })
            .unwrap_or(false);
        if same {
            continue;
        }
        let records = core
            .dispatch(Request::new(
                "blob.link",
                vec![
                    app.to_string(),
                    blob.name.clone(),
                    blob.hash.clone(),
                    blob.size.to_string(),
                    blob.mime.clone(),
                ],
            ))
            .map_err(|e| e.to_string())?;
        changed |= !records.is_empty();
    }
    Ok(changed)
}

fn core_home(_core: &HostCore) -> Result<std::path::PathBuf, String> {
    Ok(home_dir())
}

fn is_sync_event_for_app(record: &terrane_core::EventRecord, app: &str) -> Result<bool, String> {
    match record.kind.as_str() {
        "kv.set" | "kv.deleted" => {
            let value = terrane_cap_kv::event_payload_json(record).map_err(|e| e.to_string())?;
            Ok(value
                .and_then(|v| v.get("app").and_then(|app| app.as_str()).map(str::to_string))
                .as_deref()
                == Some(app))
        }
        _ => Ok(false),
    }
}

fn http_post_borsh<T: BorshDeserialize>(url: &str, body: Vec<u8>) -> Result<T, String> {
    let response = ureq::post(url)
        .send_bytes(&body)
        .map_err(|e| format!("sync http post {url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(io)?;
    borsh::from_slice(&bytes).map_err(|e| format!("sync http decode {url}: {e}"))
}

fn http_get_borsh<T: BorshDeserialize>(url: &str) -> Result<T, String> {
    let response = ureq::get(url)
        .call()
        .map_err(|e| format!("sync http get {url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut bytes)
        .map_err(io)?;
    borsh::from_slice(&bytes).map_err(|e| format!("sync http decode {url}: {e}"))
}

fn http_post_empty(url: &str, body: Vec<u8>) -> Result<(), String> {
    ureq::post(url)
        .send_bytes(&body)
        .map_err(|e| format!("sync http post {url}: {e}"))?;
    Ok(())
}

fn http_get_u64(url: &str) -> Result<u64, String> {
    let text = ureq::get(url)
        .call()
        .map_err(|e| format!("sync http get {url}: {e}"))?
        .into_string()
        .map_err(|e| format!("sync http read {url}: {e}"))?;
    text.trim()
        .parse::<u64>()
        .map_err(|_| format!("sync http bad u64 from {url}: {text}"))
}

fn wait_http_once(app: &str, base_url: &str) -> Result<(), String> {
    let url = format!("{}/sync/{app}/wait", base_url.trim_end_matches('/'));
    match ureq::get(&url).call() {
        Ok(_) => Ok(()),
        Err(ureq::Error::Status(204, _)) => Ok(()),
        Err(e) => Err(format!("sync wait {url}: {e}")),
    }
}

fn copy_missing_blobs_http(core: &mut HostCore, app: &str, base: &str) -> Result<bool, String> {
    let mut changed = false;
    let hashes = terrane_cap_blob::live_hashes_for_app(&core.state().blob, app);
    for hash in hashes {
        if matches!(
            crate::blob_store::verify_hash(&home_dir(), &hash).map_err(|e| e.to_string())?,
            crate::blob_store::BlobHealth::Ok { .. }
        ) {
            continue;
        }
        let url = format!("{base}/sync/{app}/blob/{hash}");
        let response = ureq::get(&url)
            .call()
            .map_err(|e| format!("sync blob get {url}: {e}"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .read_to_end(&mut bytes)
            .map_err(io)?;
        changed |= copy_missing_blob(core, &hash, &bytes)?;
    }
    Ok(changed)
}

fn local_display_name() -> String {
    std::env::var("TERRANE_SYNC_NAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "Terrane Home".to_string())
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
