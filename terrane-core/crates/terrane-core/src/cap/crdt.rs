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
use loro::{ExportMode, LoroDoc, LoroError, LoroValue};
use terrane_domain::{AppId, Error, EventRecord, Result};

use super::{arg, Capability, ReadValue, ResourceMethod};
use crate::{decode_event, encode_event, Decision, State};

/// This capability's slice of State: one Loro document per app. Reacts to
/// `app.removed` by dropping that app's document.
#[derive(Default)]
pub struct CrdtState {
    pub docs: BTreeMap<AppId, LoroDoc>,
}

// A `LoroDoc`'s derived `clone()` is a *reference* clone (it aliases the same
// underlying document), which would let a backend run mutate live State. State
// is cloned into every `host.run`, so CrdtState must deep-copy: `fork()` gives an
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
    fn resource_api(&self) -> Vec<ResourceMethod> {
        vec![
            // Map — a merging key/value document.
            ResourceMethod::Write { name: "mapSet", params: &["doc", "key", "value"] },
            ResourceMethod::Read { name: "mapGet", params: &["doc", "key"], read: read_map_get },
            ResourceMethod::Read { name: "mapAll", params: &["doc"], read: read_map_all },
            ResourceMethod::Write { name: "mapDel", params: &["doc", "key"] },
            // List — an ordered sequence.
            ResourceMethod::Write { name: "listPush", params: &["doc", "value"] },
            ResourceMethod::Write { name: "listInsert", params: &["doc", "index", "value"] },
            ResourceMethod::Write { name: "listDel", params: &["doc", "index"] },
            ResourceMethod::Read { name: "listAll", params: &["doc"], read: read_list_all },
            // Text — a collaborative string.
            ResourceMethod::Write { name: "textInsert", params: &["doc", "index", "text"] },
            ResourceMethod::Write { name: "textDel", params: &["doc", "index", "len"] },
            ResourceMethod::Read { name: "textGet", params: &["doc"], read: read_text_get },
        ]
    }

    fn decide(&self, state: &State, name: &str, args: &[String]) -> Result<Decision> {
        let app = arg(args, 0, "app")?;
        if !state.app.apps.contains_key(&app) {
            return Err(Error::AppNotFound(app));
        }

        // Apply the op to a fork (never to live State), then export just the new
        // delta. Authoring under a stable per-app PeerID keeps the recorded bytes
        // reproducible run-to-run; if Loro refuses the id we keep the fork's own
        // peer — replay is unaffected either way, since we record the bytes.
        let doc = match state.crdt.docs.get(&app) {
            Some(existing) => existing.fork(),
            None => doc_for(&app),
        };
        let _ = doc.set_peer_id(peer_for(&app));
        let before = doc.oplog_vv();

        match name {
            "crdt.mapSet" => {
                let cname = arg(args, 1, "doc")?;
                let key = arg(args, 2, "key")?;
                let value = rest(args, 3);
                doc.get_map(cname.as_str()).insert(key.as_str(), value).map_err(crdt_err)?;
            }
            "crdt.mapDel" => {
                let cname = arg(args, 1, "doc")?;
                let key = arg(args, 2, "key")?;
                doc.get_map(cname.as_str()).delete(key.as_str()).map_err(crdt_err)?;
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
                doc.get_list(cname.as_str()).insert(index, value).map_err(crdt_err)?;
            }
            "crdt.listDel" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                doc.get_list(cname.as_str()).delete(index, 1).map_err(crdt_err)?;
            }
            "crdt.textInsert" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                let text = rest(args, 3);
                doc.get_text(cname.as_str()).insert(index, &text).map_err(crdt_err)?;
            }
            "crdt.textDel" => {
                let cname = arg(args, 1, "doc")?;
                let index = index_arg(args, 2, "index")?;
                let len = index_arg(args, 3, "len")?;
                doc.get_text(cname.as_str()).delete(index, len).map_err(crdt_err)?;
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

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "crdt.update" => {
                let e: Update = decode_event(record)?;
                let doc = state
                    .crdt
                    .docs
                    .entry(e.app.clone())
                    .or_insert_with(|| doc_for(&e.app));
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
                state.crdt.docs.remove(&e.id);
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
}

/// A fresh document for `app`, authored under its stable PeerID.
fn doc_for(app: &str) -> LoroDoc {
    let doc = LoroDoc::new();
    let _ = doc.set_peer_id(peer_for(app));
    doc
}

/// A stable, valid (nonzero, sub-`2^47`) PeerID derived from the app id via
/// FNV-1a — so the local replica always authors under the same peer.
fn peer_for(app: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in app.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h & ((1u64 << 47) - 1)) | 1
}

/// Join the trailing args from `from` into one value (so a value may contain
/// spaces, matching `kv.set`).
fn rest(args: &[String], from: usize) -> String {
    args.get(from..).unwrap_or_default().join(" ")
}

/// Parse a positional non-negative integer arg (Loro positions/lengths).
fn index_arg(args: &[String], index: usize, what: &str) -> Result<usize> {
    let s = arg(args, index, what)?;
    s.parse::<usize>()
        .map_err(|_| Error::InvalidInput(format!("{what} must be a non-negative integer, got {s:?}")))
}

fn crdt_err(e: LoroError) -> Error {
    Error::Runtime(format!("crdt: {e}"))
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
fn read_map_get(state: &State, app: &str, args: &[String]) -> ReadValue {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let key = args.get(1).map(String::as_str).unwrap_or_default();
    let value = state.crdt.docs.get(app).and_then(|doc| {
        match doc.get_map(cname).get_deep_value() {
            LoroValue::Map(m) => m.get(key).and_then(loro_string),
            _ => None,
        }
    });
    ReadValue::OptString(value)
}

/// `ctx.resource.crdt.mapAll(doc)` — every string entry in `app`'s named map.
fn read_map_all(state: &State, app: &str, args: &[String]) -> ReadValue {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let mut out = BTreeMap::new();
    if let Some(doc) = state.crdt.docs.get(app) {
        if let LoroValue::Map(m) = doc.get_map(cname).get_deep_value() {
            for (k, v) in m.iter() {
                if let Some(s) = loro_string(v) {
                    out.insert(k.clone(), s);
                }
            }
        }
    }
    ReadValue::StringMap(out)
}

/// `ctx.resource.crdt.listAll(doc)` — the ordered elements of `app`'s named list.
fn read_list_all(state: &State, app: &str, args: &[String]) -> ReadValue {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let mut out = Vec::new();
    if let Some(doc) = state.crdt.docs.get(app) {
        if let LoroValue::List(l) = doc.get_list(cname).get_deep_value() {
            out.extend(l.iter().map(stringify));
        }
    }
    ReadValue::StringList(out)
}

/// `ctx.resource.crdt.textGet(doc)` — the current contents of `app`'s named text
/// (none only if the app has no document yet).
fn read_text_get(state: &State, app: &str, args: &[String]) -> ReadValue {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let text = state.crdt.docs.get(app).map(|doc| doc.get_text(cname).to_string());
    ReadValue::OptString(text)
}
