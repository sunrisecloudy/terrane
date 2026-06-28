//! The `crdt` capability — per-app conflict-free replicated documents backed by
//! [Loro](https://loro.dev). Where `kv` is a last-writer-wins string store, a
//! `crdt` document *merges*: two replicas that edited concurrently converge to
//! the same value with no lost writes. One Loro document per app holds named
//! Map, List, and Text containers.
//!
//! ## Determinism (Option A, the same shape as `net`/`model`)
//!
//! Loro is non-deterministic at the edge (a fresh op carries an author PeerID and
//! the export embeds it), so we treat a write exactly like a recorded effect: the
//! op runs *once* in [`decide`](CrdtCapability::decide) — on a `fork()` of the
//! current doc, so State is never mutated there — and the binary delta Loro
//! produces is the event payload. [`fold`](CrdtCapability::fold) only `import`s
//! that delta. Replay re-imports the *same recorded bytes* in the *same order*,
//! and CRDT import is deterministic, so the rebuilt document is identical — the
//! replay-identity contract holds. JS/Loro authoring never re-runs on replay.
//!
//! This is also the sync foundation: a remote replica's update is just another
//! `crdt.update` event folded through the same `import` path.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use loro::{ExportMode, LoroDoc, LoroError, LoroValue, VersionVector};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_event, encode_event, ensure_app_exists, replica_peer, state_mut, state_ref, AppId,
    CapManifest, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord, EventSpec,
    ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

/// This capability's slice of State: one Loro document per app. Reacts to
/// `app.removed` by dropping that app's document.
#[derive(Default)]
pub struct CrdtState {
    pub docs: BTreeMap<AppId, LoroDoc>,
}

// A `LoroDoc`'s derived `clone()` is a *reference* clone (it aliases the same
// underlying document), which would let a backend run mutate live State. State
// is cloned into every `js-runtime.run`, so CrdtState must deep-copy: `fork()` gives an
// independent document. (The fork's own PeerID is irrelevant — a live document is
// only ever imported into, never authored on directly.)
impl Clone for CrdtState {
    fn clone(&self) -> Self {
        CrdtState {
            docs: self
                .docs
                .iter()
                .map(|(app, doc)| (app.clone(), doc.fork()))
                .collect(),
        }
    }
}

// Replay-identity compares live State to State folded from the log. Two documents
// that imported the same updates converge to the same *value* (the CRDT
// guarantee), even if internal bytes/peer metadata differ — so equality is over
// the documents' deep values, which is exactly what replay must reproduce.
impl PartialEq for CrdtState {
    fn eq(&self, other: &Self) -> bool {
        self.docs.len() == other.docs.len()
            && self.docs.iter().all(|(app, doc)| {
                other
                    .docs
                    .get(app)
                    .is_some_and(|o| doc.get_deep_value() == o.get_deep_value())
            })
    }
}

impl std::fmt::Debug for CrdtState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut m = f.debug_map();
        for (app, doc) in &self.docs {
            m.entry(app, &doc.get_deep_value());
        }
        m.finish()
    }
}

/// A recorded CRDT op: the binary Loro update for `app`'s document. Opaque on
/// purpose — the bytes are Loro's own update format, re-imported verbatim.
#[derive(BorshSerialize, BorshDeserialize)]
struct Update {
    app: String,
    bytes: Vec<u8>,
}

pub struct CrdtCapability;

impl Capability for CrdtCapability {
    fn namespace(&self) -> &'static str {
        "crdt"
    }

    /// The app-scoped CRDT surface backends get on `ctx.resource.crdt`. Every
    /// method's first arg is a container *name* (an app may hold many named
    /// documents); numeric args (`index`, `len`) arrive as strings — the runtime
    /// passes JS args verbatim with no coercion — and are parsed here.
    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "crdt.mapSet",
                },
                CommandSpec {
                    name: "crdt.mapDel",
                },
                CommandSpec {
                    name: "crdt.listPush",
                },
                CommandSpec {
                    name: "crdt.listInsert",
                },
                CommandSpec {
                    name: "crdt.listDel",
                },
                CommandSpec {
                    name: "crdt.textInsert",
                },
                CommandSpec {
                    name: "crdt.textDel",
                },
                CommandSpec { name: "crdt.merge" },
            ],
            events: vec![EventSpec {
                kind: "crdt.update",
            }],
            queries: Vec::new(),
            resources: resource_methods(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        let app = arg(args, 0, "app")?;
        ensure_app_exists(ctx.bus, &app)?;

        // `crdt.merge` ingests another replica's update rather than authoring one.
        if name == "crdt.merge" {
            return decide_merge(ctx.state, app, args);
        }

        // Apply the op to a fork (never to live State), then export just the new
        // delta. `fork()` gives the op a fresh, distinct PeerID — exactly what a
        // CRDT needs: two replicas of the same app must NOT share a peer, or their
        // concurrent ops would collide on `(peer, counter)` and a merge would
        // silently drop one. The randomness is frozen into the recorded bytes
        // (Option A), so replay re-imports it and replay-identity still holds.
        let doc = match state_ref::<CrdtState>(ctx.state, "crdt")?.docs.get(&app) {
            Some(existing) => existing.fork(),
            None => LoroDoc::new(),
        };
        // Author under this home's stable replica PeerID when it has minted one
        // (so all its edits share one peer); otherwise the fork's fresh peer still
        // keeps the op distinct. Either way the bytes are recorded (Option A).
        if let Some(peer) = replica_peer(ctx.bus)? {
            let _ = doc.set_peer_id(peer);
        }
        let before = doc.oplog_vv();

        match name {
            "crdt.mapSet" => {
                let cname = arg(args, 1, "doc")?;
                let key = arg(args, 2, "key")?;
                let value = rest(args, 3);
                doc.get_map(cname.as_str())
                    .insert(key.as_str(), value)
                    .map_err(crdt_err)?;
            }
            "crdt.mapDel" => {
                let cname = arg(args, 1, "doc")?;
                let key = arg(args, 2, "key")?;
                doc.get_map(cname.as_str())
                    .delete(key.as_str())
                    .map_err(crdt_err)?;
            }
            "crdt.listPush" => {
                let cname = arg(args, 1, "doc")?;
                let value = rest(args, 2);
                doc.get_list(cname.as_str()).push(value).map_err(crdt_err)?;
            }
            "crdt.listInsert" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                let value = rest(args, 3);
                doc.get_list(cname.as_str())
                    .insert(index, value)
                    .map_err(crdt_err)?;
            }
            "crdt.listDel" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                doc.get_list(cname.as_str())
                    .delete(index, 1)
                    .map_err(crdt_err)?;
            }
            "crdt.textInsert" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                let text = rest(args, 3);
                doc.get_text(cname.as_str())
                    .insert(index, &text)
                    .map_err(crdt_err)?;
            }
            "crdt.textDel" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                let len = index_arg(args, 3, "len")?;
                doc.get_text(cname.as_str())
                    .delete(index, len)
                    .map_err(crdt_err)?;
            }
            other => return Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }

        doc.commit();
        let bytes = doc
            .export(ExportMode::updates_owned(before))
            .map_err(|e| Error::Storage(format!("crdt export: {e}")))?;
        Ok(Decision::Commit(vec![encode_event(
            "crdt.update",
            &Update { app, bytes },
        )?]))
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "crdt.update" => {
                let e: Update = decode_event(record)?;
                let doc = state_mut::<CrdtState>(state, "crdt")?
                    .docs
                    .entry(e.app.clone())
                    .or_default();
                doc.import(&e.bytes)
                    .map_err(|err| Error::Storage(format!("crdt import: {err}")))?;
            }
            // React to another capability's event: drop a removed app's document.
            "app.removed" => {
                #[derive(BorshDeserialize)]
                struct Removed {
                    id: String,
                }
                let e: Removed = decode_event(record)?;
                state_mut::<CrdtState>(state, "crdt")?.docs.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "crdt.update" => {
                let e: Update = decode_event(record).ok()?;
                Some(format!("crdt.update {} ({} bytes)", e.app, e.bytes.len()))
            }
            _ => None,
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "mapGet" => read_map_get(ctx.state, ctx.app, args),
            "mapAll" => read_map_all(ctx.state, ctx.app, args),
            "listAll" => read_list_all(ctx.state, ctx.app, args),
            "textGet" => read_text_get(ctx.state, ctx.app, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: crdt.{other}"
            ))),
        }
    }
}

/// `crdt.merge <app> <hex>` — ingest another replica's exported Loro update.
/// Validates by importing into a fork (a malformed blob is rejected here, never
/// committed, so the log can't be poisoned) and dedups (an update that adds
/// nothing new is a no-op), then records the bytes as an ordinary `crdt.update`
/// so the merge replays like any other write.
fn decide_merge(state: &dyn StateStore, app: String, args: &[String]) -> Result<Decision> {
    let hex = arg(args, 1, "update")?;
    let bytes = from_hex(&hex)?;
    let doc = match state_ref::<CrdtState>(state, "crdt")?.docs.get(&app) {
        Some(existing) => existing.fork(),
        None => LoroDoc::new(),
    };
    let before = doc.oplog_vv();
    doc.import(&bytes)
        .map_err(|e| Error::InvalidInput(format!("crdt.merge: invalid update: {e}")))?;
    if doc.oplog_vv() == before {
        // We already have every op in this update — nothing to record.
        return Ok(Decision::Commit(vec![]));
    }
    Ok(Decision::Commit(vec![encode_event(
        "crdt.update",
        &Update { app, bytes },
    )?]))
}

/// Export `app`'s document from `source` as a hex update containing only the ops
/// `since` lacks (a delta), ready to feed to `crdt.merge` on the `since` replica.
/// `None` if `source` has no document for the app. This is the outbound half of
/// sync — a host operation, deliberately NOT on the backend `ctx.resource`.
pub fn crdt_export_hex(
    source: &dyn StateStore,
    app: &str,
    since: &dyn StateStore,
) -> Result<Option<String>> {
    let Some(doc) = state_ref::<CrdtState>(source, "crdt")?.docs.get(app) else {
        return Ok(None);
    };
    let from = state_ref::<CrdtState>(since, "crdt")?
        .docs
        .get(app)
        .map(|d| d.oplog_vv())
        .unwrap_or_default();
    let bytes = doc
        .export(ExportMode::updates_owned(from))
        .map_err(|e| Error::Storage(format!("crdt export: {e}")))?;
    Ok(Some(to_hex(&bytes)))
}

/// This replica's encoded version vector for `app` (empty if it has no document
/// yet). A peer sends this so we can export exactly the ops it's missing — the
/// inbound half of a networked sync.
pub fn crdt_vv(state: &dyn StateStore, app: &str) -> Vec<u8> {
    state_ref::<CrdtState>(state, "crdt")
        .ok()
        .and_then(|state| state.docs.get(app).map(|d| d.oplog_vv().encode()))
        .unwrap_or_default()
}

/// Export `app`'s ops that a peer (identified by its encoded version vector
/// `peer_vv`, empty = has nothing) is missing — the raw Loro update bytes. Hex
/// them for `crdt.merge`, or frame them straight onto a socket. Empty if we have
/// no document for the app.
pub fn crdt_export_from_vv(state: &dyn StateStore, app: &str, peer_vv: &[u8]) -> Result<Vec<u8>> {
    let crdt = state_ref::<CrdtState>(state, "crdt")?;
    let Some(doc) = crdt.docs.get(app) else {
        return Ok(Vec::new());
    };
    let from = if peer_vv.is_empty() {
        VersionVector::default()
    } else {
        VersionVector::decode(peer_vv)
            .map_err(|e| Error::InvalidInput(format!("crdt sync: bad version vector: {e}")))?
    };
    doc.export(ExportMode::updates_owned(from))
        .map_err(|e| Error::Storage(format!("crdt export: {e}")))
}

/// The string elements of `app`'s named list — a read accessor for hosts/tests.
pub fn crdt_list_strings(state: &dyn StateStore, app: &str, container: &str) -> Vec<String> {
    match read_list_all(state, app, &[container.to_string()]) {
        Ok(ReadValue::StringList(items)) => items,
        _ => Vec::new(),
    }
}

/// Lower-case hex encoding (the wire form for a Loro update on the command line).
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn from_hex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(Error::InvalidInput(
            "crdt.merge: odd-length update hex".into(),
        ));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|_| Error::InvalidInput("crdt.merge: invalid update hex".into()))
        })
        .collect()
}

/// Join the trailing args from `from` into one value (so a value may contain
/// spaces, matching `kv.set`).
fn rest(args: &[String], from: usize) -> String {
    args.get(from..).unwrap_or_default().join(" ")
}

/// Parse a positional non-negative integer arg (Loro positions/lengths).
fn index_arg(args: &[String], index: usize, what: &str) -> Result<usize> {
    let s = arg(args, index, what)?;
    s.parse::<usize>().map_err(|_| {
        Error::InvalidInput(format!("{what} must be a non-negative integer, got {s:?}"))
    })
}

fn crdt_err(e: LoroError) -> Error {
    Error::Runtime(format!("crdt: {e}"))
}

fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        // Map — a merging key/value document.
        ResourceMethod::Write {
            name: "mapSet",
            params: &["doc", "key", "value"],
        },
        ResourceMethod::Read {
            name: "mapGet",
            params: &["doc", "key"],
        },
        ResourceMethod::Read {
            name: "mapAll",
            params: &["doc"],
        },
        ResourceMethod::Write {
            name: "mapDel",
            params: &["doc", "key"],
        },
        // List — an ordered sequence.
        ResourceMethod::Write {
            name: "listPush",
            params: &["doc", "value"],
        },
        ResourceMethod::Write {
            name: "listInsert",
            params: &["doc", "index", "value"],
        },
        ResourceMethod::Write {
            name: "listDel",
            params: &["doc", "index"],
        },
        ResourceMethod::Read {
            name: "listAll",
            params: &["doc"],
        },
        // Text — a collaborative string.
        ResourceMethod::Write {
            name: "textInsert",
            params: &["doc", "index", "text"],
        },
        ResourceMethod::Write {
            name: "textDel",
            params: &["doc", "index", "len"],
        },
        ResourceMethod::Read {
            name: "textGet",
            params: &["doc"],
        },
    ]
}

/// A Loro scalar as a string (`None` if it isn't a string — map reads are
/// string-typed like `kv`).
fn loro_string(v: &LoroValue) -> Option<String> {
    match v {
        LoroValue::String(s) => Some(s.as_ref().to_string()),
        _ => None,
    }
}

/// A lenient string view of any Loro scalar, for ordered reads (list/text).
fn stringify(v: &LoroValue) -> String {
    match v {
        LoroValue::String(s) => s.as_ref().to_string(),
        LoroValue::I64(n) => n.to_string(),
        LoroValue::Double(n) => n.to_string(),
        LoroValue::Bool(b) => b.to_string(),
        LoroValue::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

/// `ctx.resource.crdt.mapGet(doc, key)` — the value for `key` in `app`'s named
/// map, or none.
fn read_map_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let key = args.get(1).map(String::as_str).unwrap_or_default();
    let value = state_ref::<CrdtState>(state, "crdt")?
        .docs
        .get(app)
        .and_then(|doc| match doc.get_map(cname).get_deep_value() {
            LoroValue::Map(m) => m.get(key).and_then(loro_string),
            _ => None,
        });
    Ok(ReadValue::OptString(value))
}

/// `ctx.resource.crdt.mapAll(doc)` — every string entry in `app`'s named map.
fn read_map_all(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let mut out = BTreeMap::new();
    if let Some(doc) = state_ref::<CrdtState>(state, "crdt")?.docs.get(app) {
        if let LoroValue::Map(m) = doc.get_map(cname).get_deep_value() {
            for (k, v) in m.iter() {
                if let Some(s) = loro_string(v) {
                    out.insert(k.clone(), s);
                }
            }
        }
    }
    Ok(ReadValue::StringMap(out))
}

/// `ctx.resource.crdt.listAll(doc)` — the ordered elements of `app`'s named list.
fn read_list_all(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let mut out = Vec::new();
    if let Some(doc) = state_ref::<CrdtState>(state, "crdt")?.docs.get(app) {
        if let LoroValue::List(l) = doc.get_list(cname).get_deep_value() {
            out.extend(l.iter().map(stringify));
        }
    }
    Ok(ReadValue::StringList(out))
}

/// `ctx.resource.crdt.textGet(doc)` — the current contents of `app`'s named text
/// (none only if the app has no document yet).
fn read_text_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let text = state_ref::<CrdtState>(state, "crdt")?
        .docs
        .get(app)
        .map(|doc| doc.get_text(cname).to_string());
    Ok(ReadValue::OptString(text))
}

#[cfg(test)]
mod tests;
