use loro::{ExportMode, LoroDoc, VersionVector};
use terrane_cap_interface::{state_ref, Error, Result, StateStore};

use crate::state::CrdtState;

/// Export `app`'s document from `source` as a hex update containing only the ops
/// `since` lacks (a delta), ready to feed to `crdt.merge` on the `since`
/// replica. `None` if `source` has no document for the app.
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
/// yet). A peer sends this so we can export exactly the ops it's missing.
pub fn crdt_vv(state: &dyn StateStore, app: &str) -> Vec<u8> {
    state_ref::<CrdtState>(state, "crdt")
        .ok()
        .and_then(|state| state.docs.get(app).map(|d| d.oplog_vv().encode()))
        .unwrap_or_default()
}

/// Export `app`'s ops that a peer (identified by its encoded version vector
/// `peer_vv`, empty = has nothing) is missing — the raw Loro update bytes. Hex
/// them for `crdt.merge`, or frame them straight onto a socket.
pub fn crdt_export_from_vv(state: &dyn StateStore, app: &str, peer_vv: &[u8]) -> Result<Vec<u8>> {
    let crdt = state_ref::<CrdtState>(state, "crdt")?;
    let Some(doc) = crdt.docs.get(app) else {
        return Ok(Vec::new());
    };
    let from = decode_vv(peer_vv)?;
    doc.export(ExportMode::updates_owned(from))
        .map_err(|e| Error::Storage(format!("crdt export: {e}")))
}

pub(crate) fn fork_or_new(state: &dyn StateStore, app: &str) -> Result<LoroDoc> {
    Ok(match state_ref::<CrdtState>(state, "crdt")?.docs.get(app) {
        Some(existing) => existing.fork(),
        None => LoroDoc::new(),
    })
}

pub(crate) fn decode_vv(peer_vv: &[u8]) -> Result<VersionVector> {
    if peer_vv.is_empty() {
        Ok(VersionVector::default())
    } else {
        VersionVector::decode(peer_vv)
            .map_err(|e| Error::InvalidInput(format!("crdt sync: bad version vector: {e}")))
    }
}

/// Lower-case hex encoding (the wire form for a Loro update on the command
/// line).
pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

pub(crate) fn from_hex(s: &str) -> Result<Vec<u8>> {
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
