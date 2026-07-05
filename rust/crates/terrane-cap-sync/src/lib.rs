//! Durable sync session facts: paired peers plus per-origin event cursors.
//!
//! Network discovery, bearer tokens, HTTP, long-polling, and blob byte transfer
//! are host-edge concerns. This capability records only the replayable facts
//! needed to make an accepted event batch deterministic.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_event, encode_event, ensure_app_exists, state_mut, state_ref, CapManifest,
    Capability, CommandCtx, CommandSpec, Decision, Error, EventRecord, EventSpec, QueryCtx,
    QuerySpec, QueryValue, Result, StateStore,
};

mod doc;

pub const MAX_SYNC_BATCH_EVENTS: usize = 5_000;
pub const MAX_SYNC_BATCH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncState {
    pub peers: BTreeMap<String, PeerInfo>,
    pub cursors: BTreeMap<(String, String), u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    pub display_name: String,
    pub paired: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SyncEnvelope {
    pub origin_peer: String,
    pub origin_seq: u64,
    pub kind: String,
    pub payload: Vec<u8>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct PeerPaired {
    peer: String,
    display_name: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct PeerUnpaired {
    peer: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Applied {
    peer: String,
    app: String,
    from_seq: u64,
    to_seq: u64,
}

pub struct SyncCapability;

impl Capability for SyncCapability {
    fn namespace(&self) -> &'static str {
        "sync"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "sync.pair" },
                CommandSpec {
                    name: "sync.unpair",
                },
                CommandSpec { name: "sync.apply" },
            ],
            events: vec![
                EventSpec {
                    kind: "sync.peer.paired",
                },
                EventSpec {
                    kind: "sync.peer.unpaired",
                },
                EventSpec {
                    kind: "sync.applied",
                },
            ],
            queries: vec![
                QuerySpec { name: "sync.peers" },
                QuerySpec {
                    name: "sync.cursor",
                },
            ],
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::sync_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "sync.pair" => decide_pair(ctx, args),
            "sync.unpair" => decide_unpair(ctx, args),
            "sync.apply" => decide_apply(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "peers" => Ok(QueryValue::Json(peers_json(
                state_ref::<SyncState>(ctx.state, "sync")?,
            ))),
            "cursor" => {
                let peer = arg(args, 0, "peer")?;
                let app = arg(args, 1, "app")?;
                Ok(QueryValue::U64(Some(
                    cursor_for(ctx.state, &peer, &app).unwrap_or(0),
                )))
            }
            other => Err(Error::InvalidInput(format!("unknown query: sync.{other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "sync.peer.paired" => {
                let e: PeerPaired = decode_event(record)?;
                state_mut::<SyncState>(state, "sync")?.peers.insert(
                    e.peer,
                    PeerInfo {
                        display_name: e.display_name,
                        paired: true,
                    },
                );
            }
            "sync.peer.unpaired" => {
                let e: PeerUnpaired = decode_event(record)?;
                if let Some(peer) = state_mut::<SyncState>(state, "sync")?.peers.get_mut(&e.peer) {
                    peer.paired = false;
                }
            }
            "sync.applied" => {
                let e: Applied = decode_event(record)?;
                state_mut::<SyncState>(state, "sync")?
                    .cursors
                    .insert((e.peer, e.app), e.to_seq);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "sync.peer.paired" => {
                let e: PeerPaired = decode_event(record).ok()?;
                Some(format!("sync.peer.paired {} {}", e.peer, e.display_name))
            }
            "sync.peer.unpaired" => {
                let e: PeerUnpaired = decode_event(record).ok()?;
                Some(format!("sync.peer.unpaired {}", e.peer))
            }
            "sync.applied" => {
                let e: Applied = decode_event(record).ok()?;
                Some(format!(
                    "sync.applied {} {} {}..{}",
                    e.peer, e.app, e.from_seq, e.to_seq
                ))
            }
            _ => None,
        }
    }
}

fn decide_pair(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let peer = validate_peer(&arg(args, 0, "peer_hex")?)?;
    let display_name = arg(args, 1, "display_name")?;
    if display_name.trim().is_empty() {
        return Err(Error::InvalidInput("display_name must not be empty".into()));
    }
    let existing = state_ref::<SyncState>(ctx.state, "sync")?.peers.get(&peer);
    if existing.is_some_and(|p| p.paired && p.display_name == display_name) {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![encode_event(
        "sync.peer.paired",
        &PeerPaired { peer, display_name },
    )?]))
}

fn decide_unpair(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let peer = validate_peer(&arg(args, 0, "peer_hex")?)?;
    let already_unpaired = state_ref::<SyncState>(ctx.state, "sync")?
        .peers
        .get(&peer)
        .map(|p| !p.paired)
        .unwrap_or(true);
    if already_unpaired {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![encode_event(
        "sync.peer.unpaired",
        &PeerUnpaired { peer },
    )?]))
}

fn decide_apply(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let peer = validate_peer(&arg(args, 0, "peer_hex")?)?;
    let app = arg(args, 1, "app")?;
    if app.trim().is_empty() {
        return Err(Error::InvalidInput("app must not be empty".into()));
    }
    ensure_app_exists(ctx.bus, &app)?;
    if !state_ref::<SyncState>(ctx.state, "sync")?
        .peers
        .get(&peer)
        .is_some_and(|p| p.paired)
    {
        return Err(Error::InvalidInput(format!("sync peer is not paired: {peer}")));
    }
    let from_seq = parse_seq(args, 2, "from_seq")?;
    let to_seq = parse_seq(args, 3, "to_seq")?;
    if to_seq < from_seq {
        return Err(Error::InvalidInput("to_seq must be >= from_seq".into()));
    }
    let batch_hex = arg(args, 4, "batch_hex")?;
    let batch_bytes = from_hex(&batch_hex)?;
    if batch_bytes.len() > MAX_SYNC_BATCH_BYTES {
        return Err(Error::InvalidInput(format!(
            "sync batch exceeds {MAX_SYNC_BATCH_BYTES} bytes"
        )));
    }
    let batch: Vec<SyncEnvelope> = borsh::from_slice(&batch_bytes)
        .map_err(|e| Error::InvalidInput(format!("sync.apply: bad batch: {e}")))?;
    if batch.len() > MAX_SYNC_BATCH_EVENTS {
        return Err(Error::InvalidInput(format!(
            "sync batch exceeds {MAX_SYNC_BATCH_EVENTS} events"
        )));
    }
    if batch.is_empty() {
        if from_seq != to_seq {
            return Err(Error::InvalidInput(
                "empty sync batch must have matching from_seq and to_seq".into(),
            ));
        }
        return Ok(Decision::Commit(Vec::new()));
    }
    let current = cursor_for(ctx.state, &peer, &app).unwrap_or(0);
    if from_seq <= current {
        return Err(Error::InvalidInput(format!(
            "sync cursor mismatch for {peer}/{app}: cursor is {current}, got {from_seq}"
        )));
    }
    if batch.first().map(|e| e.origin_seq) != Some(from_seq)
        || batch.last().map(|e| e.origin_seq) != Some(to_seq)
    {
        return Err(Error::InvalidInput(
            "sync from_seq/to_seq must match batch bounds".into(),
        ));
    }

    let mut previous_seq = current;
    let mut records = Vec::with_capacity(batch.len() + 1);
    records.push(encode_event(
        "sync.applied",
        &Applied {
            peer: peer.clone(),
            app: app.clone(),
            from_seq,
            to_seq,
        },
    )?);
    for envelope in batch {
        if envelope.origin_peer != peer {
            return Err(Error::InvalidInput(
                "sync envelope origin_peer does not match command peer".into(),
            ));
        }
        if envelope.origin_seq <= previous_seq {
            return Err(Error::InvalidInput(
                "sync envelope origin_seq is not strictly increasing".into(),
            ));
        }
        if !allowlisted_kind(&envelope.kind) {
            return Err(Error::InvalidInput(format!(
                "sync event kind is not allowlisted: {}",
                envelope.kind
            )));
        }
        validate_app_payload(&app, &envelope)?;
        records.push(EventRecord {
            kind: envelope.kind,
            payload: envelope.payload,
            actor: String::new(),
        });
        previous_seq = envelope.origin_seq;
    }
    Ok(Decision::Commit(records))
}

pub fn encode_batch_hex(batch: &[SyncEnvelope]) -> Result<String> {
    let bytes = borsh::to_vec(batch).map_err(|e| Error::Storage(e.to_string()))?;
    if bytes.len() > MAX_SYNC_BATCH_BYTES {
        return Err(Error::InvalidInput(format!(
            "sync batch exceeds {MAX_SYNC_BATCH_BYTES} bytes"
        )));
    }
    Ok(to_hex(&bytes))
}

pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

pub fn from_hex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(Error::InvalidInput("sync hex must have even length".into()));
    }
    (0..value.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&value[i..i + 2], 16)
                .map_err(|_| Error::InvalidInput("sync hex contains non-hex bytes".into()))
        })
        .collect()
}

fn parse_seq(args: &[String], index: usize, label: &str) -> Result<u64> {
    arg(args, index, label)?.parse::<u64>().map_err(|_| {
        Error::InvalidInput(format!("{label} must be a non-negative integer"))
    })
}

fn cursor_for(state: &dyn StateStore, peer: &str, app: &str) -> Result<u64> {
    Ok(*state_ref::<SyncState>(state, "sync")?
        .cursors
        .get(&(peer.to_string(), app.to_string()))
        .unwrap_or(&0))
}

fn validate_peer(peer: &str) -> Result<String> {
    if peer.is_empty() || peer.len() > 32 || !peer.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput(
            "peer_hex must be 1..32 ASCII hex characters".into(),
        ));
    }
    Ok(peer.to_ascii_lowercase())
}

fn allowlisted_kind(kind: &str) -> bool {
    matches!(kind, "kv.set" | "kv.deleted")
}

fn validate_app_payload(app: &str, envelope: &SyncEnvelope) -> Result<()> {
    match envelope.kind.as_str() {
        "kv.set" => {
            let record = EventRecord {
                kind: envelope.kind.clone(),
                payload: envelope.payload.clone(),
                actor: String::new(),
            };
            let payload: KvSet = decode_event(&record)?;
            if payload.app == app {
                Ok(())
            } else {
                Err(Error::InvalidInput(
                    "sync kv.set payload app does not match command app".into(),
                ))
            }
        }
        "kv.deleted" => {
            let record = EventRecord {
                kind: envelope.kind.clone(),
                payload: envelope.payload.clone(),
                actor: String::new(),
            };
            let payload: KvDeleted = decode_event(&record)?;
            if payload.app == app {
                Ok(())
            } else {
                Err(Error::InvalidInput(
                    "sync kv.deleted payload app does not match command app".into(),
                ))
            }
        }
        _ => Err(Error::InvalidInput(format!(
            "sync event kind is not allowlisted: {}",
            envelope.kind
        ))),
    }
}

#[derive(BorshDeserialize)]
struct KvSet {
    app: String,
    _key: String,
    _value: String,
}

#[derive(BorshDeserialize)]
struct KvDeleted {
    app: String,
    _key: String,
}

fn peers_json(state: &SyncState) -> String {
    let mut out = String::from("[");
    let mut first = true;
    for (peer, info) in &state.peers {
        if !first {
            out.push(',');
        }
        first = false;
        out.push_str(&format!(
            "{{\"peer\":\"{}\",\"displayName\":\"{}\",\"paired\":{}}}",
            json_escape(peer),
            json_escape(&info.display_name),
            info.paired
        ));
    }
    out.push(']');
    out
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}
