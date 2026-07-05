//! The `blob` capability — app-scoped binary metadata over a host-owned
//! content-addressed byte sidecar.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    AppId, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result,
    StateStore,
};

mod doc;
mod util;

pub const MAX_BLOB_SIZE: usize = 64 * 1024 * 1024;
pub const MAX_NAME_LEN: usize = 512;
pub const MAX_BLOBS_PER_APP: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BlobMeta {
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlobState {
    pub blobs: BTreeMap<AppId, BTreeMap<String, BlobMeta>>,
    pub refs: BTreeMap<String, u64>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Stored {
    app: String,
    name: String,
    hash: String,
    size: u64,
    mime: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Removed {
    app: String,
    name: String,
    hash: String,
}

pub fn stored_event(
    app: impl Into<String>,
    name: impl Into<String>,
    hash: impl Into<String>,
    size: u64,
    mime: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "blob.stored",
        &Stored {
            app: app.into(),
            name: name.into(),
            hash: hash.into(),
            size,
            mime: mime.into(),
        },
    )
}

pub fn removed_event(
    app: impl Into<String>,
    name: impl Into<String>,
    hash: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "blob.removed",
        &Removed {
            app: app.into(),
            name: name.into(),
            hash: hash.into(),
        },
    )
}

pub fn live_hashes_for_app(state: &BlobState, app: &str) -> Vec<String> {
    state
        .blobs
        .get(app)
        .map(|names| names.values().map(|meta| meta.hash.clone()).collect())
        .unwrap_or_default()
}

pub struct BlobCapability;

impl Capability for BlobCapability {
    fn namespace(&self) -> &'static str {
        "blob"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "blob.put" },
                CommandSpec { name: "blob.rm" },
                CommandSpec { name: "blob.link" },
            ],
            events: vec![
                EventSpec {
                    kind: "blob.stored",
                },
                EventSpec {
                    kind: "blob.removed",
                },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "put",
                    params: &["name", "base64", "mime"],
                },
                ResourceMethod::Read {
                    name: "get",
                    params: &["name"],
                },
                ResourceMethod::Read {
                    name: "stat",
                    params: &["name"],
                },
                ResourceMethod::Read {
                    name: "list",
                    params: &["prefix"],
                },
                ResourceMethod::Write {
                    name: "rm",
                    params: &["name"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "blob",
                &["read", "write", "call"],
                "App-scoped binary blob storage.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::blob_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "blob.put" => decide_put(ctx, args),
            "blob.rm" => decide_rm(ctx, args),
            "blob.link" => decide_link(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "put" => {
                let record = records
                    .first()
                    .ok_or_else(|| Error::Runtime("blob.put produced no event".into()))?;
                let stored: Stored = decode_event(record)?;
                Ok(ReadValue::OptString(Some(stored.hash)))
            }
            other => Err(Error::InvalidInput(format!(
                "blob.{other} is not a callable resource"
            ))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "get" => read_get(ctx, args),
            "stat" => read_stat(ctx.state, ctx.app, args),
            "list" => read_list(ctx.state, ctx.app, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: blob.{other}"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "blob.stored" => {
                let e: Stored = decode_event(record)?;
                let state = state_mut::<BlobState>(state, "blob")?;
                let old = state.blobs.entry(e.app).or_default().insert(
                    e.name,
                    BlobMeta {
                        hash: e.hash.clone(),
                        size: e.size,
                        mime: e.mime,
                    },
                );
                if let Some(old) = old {
                    decrement_ref(&mut state.refs, &old.hash);
                }
                increment_ref(&mut state.refs, &e.hash);
            }
            "blob.removed" => {
                let e: Removed = decode_event(record)?;
                let state = state_mut::<BlobState>(state, "blob")?;
                if let Some(names) = state.blobs.get_mut(&e.app) {
                    if let Some(old) = names.remove(&e.name) {
                        decrement_ref(&mut state.refs, &old.hash);
                    } else {
                        decrement_ref(&mut state.refs, &e.hash);
                    }
                    if names.is_empty() {
                        state.blobs.remove(&e.app);
                    }
                }
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                let state = state_mut::<BlobState>(state, "blob")?;
                if let Some(names) = state.blobs.remove(&e.id) {
                    for meta in names.values() {
                        decrement_ref(&mut state.refs, &meta.hash);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "blob.stored" => {
                let e: Stored = decode_event(record).ok()?;
                Some(format!(
                    "blob.stored {}/{} {} {} bytes {}",
                    e.app, e.name, e.hash, e.size, e.mime
                ))
            }
            "blob.removed" => {
                let e: Removed = decode_event(record).ok()?;
                Some(format!("blob.removed {}/{} {}", e.app, e.name, e.hash))
            }
            _ => None,
        }
    }
}

fn decide_put(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = arg(args, 1, "name")?;
    let mime = arg(args, 2, "mime")?;
    let bytes_base64 = arg(args, 3, "bytes_base64")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_name(&name)?;
    validate_mime(&mime)?;
    enforce_app_count(ctx.state, &app, &name)?;
    let bytes = util::decode_base64(&bytes_base64)?;
    if bytes.len() > MAX_BLOB_SIZE {
        return Err(Error::InvalidInput(format!(
            "blob size exceeds {MAX_BLOB_SIZE} bytes"
        )));
    }
    let hash = util::sha256_hex(&bytes);
    Ok(Decision::Effect(Effect::BlobStore {
        app,
        name,
        mime,
        hash,
        bytes,
    }))
}

fn decide_rm(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = arg(args, 1, "name")?;
    validate_name(&name)?;
    let meta = state_ref::<BlobState>(ctx.state, "blob")?
        .blobs
        .get(&app)
        .and_then(|names| names.get(&name))
        .cloned()
        .ok_or_else(|| Error::KeyNotFound(app.clone(), name.clone()))?;
    Ok(Decision::Commit(vec![removed_event(app, name, meta.hash)?]))
}

fn decide_link(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let name = arg(args, 1, "name")?;
    let hash = arg(args, 2, "hash")?;
    let size_raw = arg(args, 3, "size")?;
    let mime = arg(args, 4, "mime")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_name(&name)?;
    validate_hash(&hash)?;
    validate_mime(&mime)?;
    enforce_app_count(ctx.state, &app, &name)?;
    let size = size_raw.parse::<u64>().map_err(|_| {
        Error::InvalidInput(format!(
            "blob size must be a non-negative integer: {size_raw}"
        ))
    })?;
    if size > MAX_BLOB_SIZE as u64 {
        return Err(Error::InvalidInput(format!(
            "blob size exceeds {MAX_BLOB_SIZE} bytes"
        )));
    }
    Ok(Decision::Commit(vec![stored_event(
        app, name, hash, size, mime,
    )?]))
}

fn read_get(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let name = args.first().map(String::as_str).unwrap_or_default();
    validate_name(name)?;
    let meta = blob_meta(ctx.state, ctx.app, name)?;
    let host = ctx
        .host
        .ok_or_else(|| Error::Runtime("blob.get requires a live host".into()))?;
    let value = host.sample(
        "blob.get",
        &[
            ctx.app.to_string(),
            name.to_string(),
            meta.hash,
            meta.size.to_string(),
        ],
    )?;
    Ok(ReadValue::OptString(Some(value)))
}

fn read_stat(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let name = args.first().map(String::as_str).unwrap_or_default();
    validate_name(name)?;
    let meta = blob_meta(state, app, name)?;
    Ok(ReadValue::OptString(Some(meta_json(name, &meta))))
}

fn read_list(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    let mut out = String::from("[");
    let mut first = true;
    if let Some(names) = state_ref::<BlobState>(state, "blob")?.blobs.get(app) {
        for (name, meta) in names {
            if !name.starts_with(prefix) {
                continue;
            }
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str(&meta_json(name, meta));
        }
    }
    out.push(']');
    Ok(ReadValue::OptString(Some(out)))
}

fn blob_meta(state: &dyn StateStore, app: &str, name: &str) -> Result<BlobMeta> {
    state_ref::<BlobState>(state, "blob")?
        .blobs
        .get(app)
        .and_then(|names| names.get(name))
        .cloned()
        .ok_or_else(|| Error::KeyNotFound(app.to_string(), name.to_string()))
}

fn enforce_app_count(state: &dyn StateStore, app: &str, name: &str) -> Result<()> {
    let state = state_ref::<BlobState>(state, "blob")?;
    let count = state.blobs.get(app).map(BTreeMap::len).unwrap_or(0);
    let replacing = state
        .blobs
        .get(app)
        .map(|names| names.contains_key(name))
        .unwrap_or(false);
    if !replacing && count >= MAX_BLOBS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "blob count exceeds per-app cap of {MAX_BLOBS_PER_APP}"
        )));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidInput("blob name must not be empty".into()));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(Error::InvalidInput(format!(
            "blob name exceeds {MAX_NAME_LEN} bytes"
        )));
    }
    Ok(())
}

fn validate_mime(mime: &str) -> Result<()> {
    if mime.trim().is_empty() {
        return Err(Error::InvalidInput("blob mime must not be empty".into()));
    }
    Ok(())
}

fn validate_hash(hash: &str) -> Result<()> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidInput(
            "blob hash must be 64 lowercase SHA-256 hex chars".into(),
        ));
    }
    if !hash
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Error::InvalidInput(
            "blob hash must be lowercase SHA-256 hex".into(),
        ));
    }
    Ok(())
}

fn increment_ref(refs: &mut BTreeMap<String, u64>, hash: &str) {
    *refs.entry(hash.to_string()).or_insert(0) += 1;
}

fn decrement_ref(refs: &mut BTreeMap<String, u64>, hash: &str) {
    if let Some(count) = refs.get_mut(hash) {
        *count = count.saturating_sub(1);
    }
}

fn meta_json(name: &str, meta: &BlobMeta) -> String {
    format!(
        "{{\"name\":\"{}\",\"hash\":\"{}\",\"size\":{},\"mime\":\"{}\"}}",
        json_escape(name),
        meta.hash,
        meta.size,
        json_escape(&meta.mime)
    )
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
