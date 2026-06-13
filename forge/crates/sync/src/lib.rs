//! forge-sync: the in-process CRDT chunk-diff sync seam (SS-1 / SS-2, M0b).
//!
//! Normative spec: `forge/spec/sync-protocol.md` (prd-merged/03 SS-1/SS-2,
//! prd-merged/02 DL-3/DL-4/DL-6/DL-9). This crate converges two workspace
//! [`Store`]s **in one process** — the local CI seam *before* WebSocket
//! transport, account auth, relay, or server-side RBAC (SS-7) exist. Those are
//! deferred; here both peers are assumed already authorized.
//!
//! ## The model (the simplest faithful SS-1/2 form)
//!
//! Each records collection is one CRDT document with `doc_id =
//! "collection/<name>"`. A peer's **frontier** for a `doc_id` is the *set* of
//! immutable exchanged chunk ids it holds (not a scalar revision — a set makes
//! duplicate and out-of-order delivery idempotent, matching Loro's property that
//! re-importing a seen update is a no-op). Sync per `doc_id`:
//!
//! 1. Each peer advertises its frontier (the chunk-id set it holds).
//! 2. The other peer sends the chunks the first lacks.
//! 3. The receiver `put_chunk`s them (append-only, idempotent, order-independent
//!    because chunks are immutable Loro updates) and rebuilds its projection.
//!
//! [`sync_stores`] runs this **both directions** over the union of doc ids, so
//! the two workspaces converge: concurrent edits merge by Loro CRDT semantics
//! (distinct peer ids), and after exchanging chunk sets both peers rebuild to the
//! same converged state (DL-9 / §9). [`pull`] is the one-directional half.
//!
//! ## Exchanged chunk identity: content-addressed (the load-bearing choice)
//!
//! `forge-storage` mints **local** chunk ids like `chunk-0001` per `(doc_id)`.
//! That is a safe *local* sequence but NOT a safe *network* frontier: two
//! disconnected peers each minting `chunk-0001` for `collection/tasks` produce
//! the same id over *different* Loro payloads (different peer ids). Diffing by
//! that id would (a) falsely conclude the frontiers match and skip a real
//! exchange, or (b) try to `put_chunk` a conflicting payload under an existing id
//! and trip the append-only guard with a `StorageError`.
//!
//! So the **exchanged** chunk id is content-addressed: `sha256:<hex>` over the
//! chunk's `(format, payload)` bytes (see [`exchanged_chunk_id`]). Identical
//! payloads → identical id (idempotent, frontiers genuinely match), different
//! payloads → different id (no false match, no append-only conflict). The
//! foreign chunk is stored under its content id, so a re-sync of the same chunk
//! is a no-op and a peer never overwrites another peer's history. This is exactly
//! the "peer-scoped or content-addressed" identity the spec mandates.

use forge_crdt::RecordsDoc;
use forge_domain::Result;
use forge_storage::{IndexManager, Store, CHUNK_FORMAT};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Counts of what one [`sync_stores`] exchange moved (SS-2 observable result).
///
/// `chunks_a_to_b` + `chunks_b_to_a` being zero on a *second* sync of the same
/// pair is the idempotence signal the fixtures assert: once converged, no chunks
/// move. `docs_synced` is the number of distinct `doc_id`s in the union the
/// runner considered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncReport {
    /// Chunks `a` sent to `b` (chunks `b` lacked), across all docs.
    pub chunks_a_to_b: usize,
    /// Chunks `b` sent to `a` (chunks `a` lacked), across all docs.
    pub chunks_b_to_a: usize,
    /// Distinct `doc_id`s in the union of the two stores that were considered.
    pub docs_synced: usize,
}

impl SyncReport {
    /// Total chunks moved in either direction — zero exactly when the two stores
    /// already held the same chunk sets for every doc (the no-op / converged case).
    pub fn total_chunks_moved(&self) -> usize {
        self.chunks_a_to_b + self.chunks_b_to_a
    }
}

/// One immutable chunk addressed by its content id, ready to hand to a peer.
struct ExchangedChunk {
    /// Content-addressed exchanged id (`sha256:<hex>` of `format`+`payload`).
    id: String,
    /// The chunk encoding tag (`loro`); preserved verbatim through the exchange.
    format: String,
    /// The opaque immutable Loro update bytes.
    payload: Vec<u8>,
}

/// The content-addressed exchanged id for a chunk's `(format, payload)`:
/// `"sha256:" + hex(sha256(format_bytes || 0x00 || payload))`.
///
/// Including `format` (with a separator that cannot appear inside the short ASCII
/// format tag) means two chunks that differ only by encoding tag never collide.
/// This is the network-safe chunk identity the diff keys on (see module docs):
/// deterministic, collision-resistant, and independent of any peer's local
/// `chunk-NNNN` sequence.
pub fn exchanged_chunk_id(format: &str, payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format.as_bytes());
    hasher.update([0u8]);
    hasher.update(payload);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        // Lowercase hex, two chars per byte — deterministic and platform-stable.
        // The nibble is a constant 0..=15, so `from_digit` never fails here.
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap());
        out.push(char::from_digit((byte & 0xf) as u32, 16).unwrap());
    }
    out
}

/// Read every chunk a store holds for `doc_id`, keyed by content-addressed
/// exchanged id (the store's local `chunk-NNNN` id is not network-safe). A
/// `BTreeMap` gives a deterministic, set-like frontier per doc.
fn frontier_for_doc(store: &Store, doc_id: &str) -> Result<BTreeMap<String, ExchangedChunk>> {
    let mut out = BTreeMap::new();
    for row in store.get_chunks(doc_id)? {
        let id = exchanged_chunk_id(&row.format, &row.payload);
        out.insert(
            id.clone(),
            ExchangedChunk { id, format: row.format, payload: row.payload },
        );
    }
    Ok(out)
}

/// Send the chunks `from` holds for `doc_id` that `into` lacks — one direction,
/// one doc. Returns how many chunks were newly imported into `into`.
///
/// `into` stores each foreign chunk under its **content-addressed** exchanged id
/// (not the origin's local id), so the append-only [`Store::put_chunk`] guard is
/// never tripped by two peers' colliding `chunk-NNNN` sequences: a chunk `into`
/// already has (same content id) is an idempotent no-op, and a chunk with new
/// content lands under a fresh id. Projection rebuild is the caller's job (done
/// once after the whole exchange), so this only moves immutable history.
fn pull_doc(into: &mut Store, from: &Store, doc_id: &str) -> Result<usize> {
    let have = frontier_for_doc(into, doc_id)?;
    let theirs = frontier_for_doc(from, doc_id)?;
    let mut moved = 0usize;
    for (id, chunk) in &theirs {
        if have.contains_key(id) {
            continue; // frontier already covers this chunk — nothing to send
        }
        // Append-only + idempotent: re-putting an identical content id is a
        // no-op, and the content address guarantees a new id only for new bytes.
        into.put_chunk(doc_id, &chunk.id, &chunk.format, &chunk.payload)?;
        moved += 1;
    }
    Ok(moved)
}

/// One-directional catch-up: import into `into` every chunk `from` holds that
/// `into` lacks, across the **union** of both stores' doc ids, then rebuild
/// `into`'s records projection from its (now-augmented) chunk history.
///
/// This is the building block [`sync_stores`] runs in each direction. It is
/// sufficient on its own when `from` already holds every write (the
/// `one_directional_catchup` / `empty_peer_catchup` cases): `into` ends with a
/// superset of `from`'s chunks for every doc and rebuilds to include them.
/// Returns the number of chunks imported into `into`.
///
/// `indexes` is the active [`IndexManager`] whose physical indexes are rebuilt
/// with the projection (DL-6); pass [`IndexManager::new`] when none are active.
pub fn pull(into: &mut Store, from: &Store, indexes: &IndexManager) -> Result<usize> {
    let doc_ids = union_doc_ids(into, from)?;
    let mut moved = 0usize;
    for doc_id in &doc_ids {
        moved += pull_doc(into, from, doc_id)?;
    }
    // Rebuild once after importing all docs: cheaper than per-doc, and the
    // projection is derived purely from the now-complete chunk history (DL-6).
    into.rebuild_projection(indexes)?;
    Ok(moved)
}

/// The sorted union of the doc ids that hold chunks in either store.
fn union_doc_ids(a: &Store, b: &Store) -> Result<Vec<String>> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for id in a.list_doc_ids()? {
        set.insert(id);
    }
    for id in b.list_doc_ids()? {
        set.insert(id);
    }
    Ok(set.into_iter().collect())
}

/// Bidirectional in-process sync (SS-1/SS-2): converge two workspace [`Store`]s.
///
/// For the union of `doc_id`s across `a` and `b`, diff the per-doc chunk-id sets;
/// send `a`'s missing chunks to `b` and `b`'s missing chunks to `a`
/// ([`Store::put_chunk`] each — append-only, idempotent), then rebuild the
/// records projection on **both**. Afterwards the two workspaces hold the same
/// chunk set per doc and their projections converge (DL-9 / §9): concurrent edits
/// merge by Loro CRDT semantics, and a same-scalar conflict resolves to one
/// agreed LWW winner on both peers.
///
/// Idempotent: a second `sync_stores` over an already-converged pair moves zero
/// chunks ([`SyncReport::total_chunks_moved`] is `0`). `indexes` rebuilds active
/// physical indexes with each projection (DL-6).
///
/// The exchange is staged so each direction diffs against the *pre-exchange*
/// frontier: `a`'s missing chunks are computed from `a`'s original frontier, not
/// one already mutated by `b`'s push. Both directions only move immutable
/// history; the single projection rebuild per store happens after both pushes.
pub fn sync_stores(a: &mut Store, b: &mut Store, indexes: &IndexManager) -> Result<SyncReport> {
    let doc_ids = union_doc_ids(a, b)?;

    let mut chunks_a_to_b = 0usize;
    let mut chunks_b_to_a = 0usize;
    for doc_id in &doc_ids {
        // Snapshot both frontiers BEFORE moving anything for this doc, so the two
        // directions are symmetric and neither sees the other's just-pushed chunks
        // (the counts then reflect a true pre-exchange diff).
        let a_front = frontier_for_doc(a, doc_id)?;
        let b_front = frontier_for_doc(b, doc_id)?;

        // a -> b: chunks a holds that b lacks.
        for (id, chunk) in &a_front {
            if !b_front.contains_key(id) {
                b.put_chunk(doc_id, &chunk.id, &chunk.format, &chunk.payload)?;
                chunks_a_to_b += 1;
            }
        }
        // b -> a: chunks b holds that a lacks.
        for (id, chunk) in &b_front {
            if !a_front.contains_key(id) {
                a.put_chunk(doc_id, &chunk.id, &chunk.format, &chunk.payload)?;
                chunks_b_to_a += 1;
            }
        }
    }

    // Rebuild both projections from their (now-augmented) chunk histories (DL-6).
    a.rebuild_projection(indexes)?;
    b.rebuild_projection(indexes)?;

    Ok(SyncReport {
        chunks_a_to_b,
        chunks_b_to_a,
        docs_synced: doc_ids.len(),
    })
}

/// Rebuild a fresh [`RecordsDoc`] from a store's persisted chunks for one
/// `doc_id`, ordered as `get_chunks` returns them. Exposed for tests/diagnostics
/// that want the CRDT view of a synced doc without going through the projection;
/// the rebuild is order/duplication independent so the ordering is immaterial to
/// the result.
pub fn rebuild_doc(store: &Store, doc_id: &str) -> Result<RecordsDoc> {
    let rows = store.get_chunks(doc_id)?;
    let payloads: Vec<Vec<u8>> = rows.into_iter().map(|r| r.payload).collect();
    let refs: Vec<&[u8]> = payloads.iter().map(|p| p.as_slice()).collect();
    RecordsDoc::from_updates(forge_storage::LOCAL_PEER_ID, &refs)
}

/// The chunk format tag the sync seam preserves (re-exported for callers that
/// build chunks to feed a store before syncing). Currently `loro`.
pub const SYNC_CHUNK_FORMAT: &str = CHUNK_FORMAT;

#[cfg(test)]
mod tests;
