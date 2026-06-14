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
use forge_storage::{collection_of_doc, IndexManager, RemoteChunk, Store, CHUNK_FORMAT};
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
    /// Chunks REJECTED by the SS-7 apply-time authorization gate before import:
    /// `b`'s rejections of `a`'s chunks plus `a`'s rejections of `b`'s chunks.
    /// Zero for the unauthorized in-process [`sync_stores`] seam; non-zero only
    /// when a receiver's trusted membership denied an incoming op (the chunk was
    /// SKIPPED, the projection left unchanged, a `permission_denied` surfaced).
    pub chunks_denied: usize,
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
    /// The **origin store's** local `chunk-NNNN` id for this chunk. Discarded for
    /// the network-safe content id, but kept here so the staging step can join the
    /// chunk back to the origin's oplog row (`doc_id#local_id`) to recover the op
    /// kind + touched record ids — the SS-7 authorization envelope metadata.
    local_id: String,
}

/// The semantic envelope describing the logical op a chunk carries, derived at the
/// SS-7 apply boundary (`forge/spec/sync-rbac.md`). It carries NO opaque CRDT
/// bytes — only `(resource_type, op, collection, record_ids)`, the metadata the
/// receiver must inspect *before* importing the chunk to decide authorization.
///
/// `collection` is always recoverable from the chunk's `doc_id` (`collection/<n>`).
/// `op` and `record_ids` are recovered from the **origin** store's oplog row for
/// the chunk (the origin authored the write locally and recorded its `kind` +
/// touched record ids). When a chunk is a foreign re-import on the origin (its
/// oplog row is `record.remote_import`, carrying no record kind), the envelope
/// falls back to a generic record write so the receiver still gates it as a write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOpEnvelope {
    /// Always `record` in M0b (schema docs are not chunk-synced yet); kept explicit
    /// so the authorizer's resource dispatch matches `forge/spec/sync-rbac.md`.
    pub resource_type: SyncResource,
    /// The logical op the chunk authored: insert / patch / delete (or a generic
    /// write when the origin oplog row does not name a record op).
    pub op: SyncRecordOp,
    /// The target collection, from the chunk's `doc_id` (`collection/<name>`). Empty
    /// when `malformed` is set (the doc id was not a `collection/<name>` records doc).
    pub collection: String,
    /// The record ids the chunk touched, from the origin oplog payload (may be
    /// empty when unknown — the collection-level grant check still applies).
    pub record_ids: Vec<String>,
    /// The ORIGINAL author of the chunk, when the staging store is only a RELAY for
    /// it. A chunk authored locally on the staging store has `None` (the relay is
    /// the author). A chunk the staging store imported from another peer carries a
    /// `record.remote_import` oplog row whose payload names the original `source`;
    /// that original `peer:<id>` is threaded here so a forwarded chunk is authorized
    /// against the ORIGINAL actor, not the relay (`review 092 #1` / SS-7 actor
    /// identity). The receiver resolves trusted membership for `origin_source` when
    /// set, else for the direct relay source.
    pub origin_source: Option<String>,
    /// `Some(reason)` when the chunk's `doc_id` was not a `collection/<name>` records
    /// doc (or the envelope is otherwise unfit to make a resource decision). A
    /// malformed envelope is denied fail-closed at the apply boundary BEFORE any
    /// grant check (`review 092 #2`: the apply path must reject a chunk lacking a
    /// valid record doc id / op metadata rather than guessing a collection).
    pub malformed: Option<String>,
}

/// The resource an incoming chunk targets. M0b chunk sync only carries records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncResource {
    Record,
}

/// The record op an incoming chunk authored, recovered from the origin oplog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRecordOp {
    Insert,
    Patch,
    Delete,
    /// The origin oplog did not name a specific record op (e.g. a transact group
    /// or a foreign re-import). Still a record WRITE for authorization purposes.
    Write,
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
            ExchangedChunk {
                id,
                format: row.format,
                payload: row.payload,
                local_id: row.chunk_id,
            },
        );
    }
    Ok(out)
}

/// What the origin store's oplog recorded for one chunk: the op `kind`, the touched
/// `record_ids`, whether the row is a FORWARDED foreign import, and — when it is —
/// the original author's `origin_source`. Used to recover the SS-7 authorization
/// envelope for a chunk staged FROM this store.
struct OplogEntry {
    kind: String,
    record_ids: Vec<String>,
    /// `true` when this store only RELAYED the chunk: its row is a
    /// `record.remote_import` (it imported the chunk from another peer rather than
    /// authoring it locally). A forwarded chunk MUST be gated against its original
    /// author, never the relay — so the staging step needs to tell a relayed chunk
    /// apart from a locally-authored one even when the original `source` is missing
    /// (`review 092 #1` / SS-7 actor identity).
    is_remote_import: bool,
    /// `Some(peer:<id>)` when the origin store relayed this chunk AND its
    /// `record.remote_import` payload named a recoverable original `source`; `None`
    /// for a chunk this store authored locally OR a relayed chunk whose original
    /// provenance is UNRECOVERABLE. The latter two are disambiguated by
    /// `is_remote_import`: a relayed chunk with `origin_source == None` must FAIL
    /// CLOSED (it cannot be attributed to the relay), never be treated as a local
    /// write.
    origin_source: Option<String>,
}

/// Index the origin store's oplog by its op id (`doc_id#local_chunk_id`) → the
/// [`OplogEntry`] it recorded for that chunk. Used to recover the SS-7
/// authorization envelope for each chunk staged FROM this store. A local write
/// records `record.insert|patch|delete|transact` plus the touched record ids; a
/// foreign re-import records `record.remote_import` whose payload names the
/// original `source` (the actual author, preserved so a forwarded chunk is gated
/// against the original actor — `review 092 #1`).
fn oplog_index(store: &Store) -> Result<BTreeMap<String, OplogEntry>> {
    let mut out = BTreeMap::new();
    for op in store.list_ops()? {
        let payload = serde_json::from_slice::<serde_json::Value>(&op.payload).ok();
        // Recover the touched record ids, trimming each and DROPPING blanks so the
        // re-exported `RemoteChunk` and the core authorization envelope always get a
        // clean, blank-free list (`review 097`). The public import boundary already
        // refuses to persist a blank entry, so this is a belt-and-suspenders against
        // any legacy / foreign oplog row that named one — a forwarded chunk's record
        // identity stays canonical across every hop.
        let record_ids = payload
            .as_ref()
            .and_then(|v| {
                v.get("record_ids").and_then(|r| r.as_array()).map(|arr| {
                    arr.iter()
                        .filter_map(|e| e.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect::<Vec<_>>()
                })
            })
            .unwrap_or_default();
        // A forwarded foreign chunk carries the original author in its remote-import
        // payload `source`; preserve it so the receiver gates the original actor. A
        // relayed chunk whose payload does NOT name a recoverable, non-empty `source`
        // leaves `origin_source` None but keeps `is_remote_import` set, so the staging
        // step can fail it closed instead of mistaking it for a local write.
        let is_remote_import = op.kind == "record.remote_import";
        let origin_source = if is_remote_import {
            payload
                .as_ref()
                .and_then(|v| v.get("source").and_then(|s| s.as_str()))
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        } else {
            None
        };
        out.insert(
            op.op_id,
            OplogEntry {
                kind: op.kind,
                record_ids,
                is_remote_import,
                origin_source,
            },
        );
    }
    Ok(out)
}

/// Build the [`SyncOpEnvelope`] for one chunk staged from `from`: the collection
/// from its `doc_id`, the op + touched record ids + original-author provenance from
/// `from`'s oplog row keyed `doc_id#local_chunk_id`. A chunk whose doc id is NOT a
/// `collection/<name>` records doc is marked `malformed` (and given an empty
/// collection) so the apply boundary denies it fail-closed instead of guessing a
/// collection from the raw doc id (`review 092 #2`). A chunk whose oplog row is
/// missing or a transact group yields a generic record [`Write`] envelope so the
/// receiver still gates it as a write (never silently allowed).
///
/// A FORWARDED foreign chunk (the origin row is a `record.remote_import`) carries its
/// original author in `origin_source` so the receiver gates the ORIGINAL actor, not
/// the relay (`review 092 #1`). If that row's original provenance is UNRECOVERABLE
/// (the `record.remote_import` payload named no usable `source`), the chunk is marked
/// `malformed` and denied fail-closed: a relay must NOT be able to launder a write
/// whose author it cannot prove by having the receiver fall back to attributing the
/// chunk to the relay (`forge/spec/sync-rbac.md` "Trust boundary").
fn envelope_for_chunk(
    doc_id: &str,
    local_id: &str,
    oplog: &BTreeMap<String, OplogEntry>,
) -> SyncOpEnvelope {
    let collection = collection_of_doc(doc_id);
    let mut malformed = match collection {
        Some(c) if !c.is_empty() => None,
        _ => Some(format!(
            "chunk doc id {doc_id:?} is not a collection/<name> records doc"
        )),
    };
    let op_id = format!("{doc_id}#{local_id}");
    let (op, record_ids, origin_source) = match oplog.get(&op_id) {
        Some(entry) => {
            // A relayed chunk whose original author is unrecoverable cannot be
            // attributed to the relay — fail it closed (`review 092 #1`). Only the
            // first defect is surfaced, so a malformed doc id still takes priority.
            if entry.is_remote_import && entry.origin_source.is_none() && malformed.is_none() {
                malformed = Some(format!(
                    "forwarded chunk {op_id:?} has no recoverable original author \
                     (record.remote_import without a usable source)"
                ));
            }
            (
                op_from_kind(&entry.kind),
                entry.record_ids.clone(),
                entry.origin_source.clone(),
            )
        }
        None => (SyncRecordOp::Write, Vec::new(), None),
    };
    SyncOpEnvelope {
        resource_type: SyncResource::Record,
        op,
        collection: collection.unwrap_or("").to_string(),
        record_ids,
        origin_source,
        malformed,
    }
}

/// Map an oplog `kind` string to the record op the authorizer gates. Anything that
/// is not a recognized single-record op (transact group, remote re-import) maps to
/// the generic [`Write`](SyncRecordOp::Write) so it is still authorized as a write.
fn op_from_kind(kind: &str) -> SyncRecordOp {
    match kind {
        "record.insert" => SyncRecordOp::Insert,
        "record.patch" => SyncRecordOp::Patch,
        "record.delete" => SyncRecordOp::Delete,
        _ => SyncRecordOp::Write,
    }
}

/// One missing chunk staged for a receiving store, paired with the SS-7
/// authorization [`SyncOpEnvelope`] describing the op it carries. The
/// content-addressed [`RemoteChunk`] is the apply unit; the envelope is the
/// metadata the receiver authorizes BEFORE importing it. The envelope travels
/// *alongside* the chunk and is NOT mixed into the content-addressed `chunk_id`,
/// so convergence and the network-safe chunk identity are untouched.
#[derive(Debug, Clone)]
pub struct StagedChunk {
    /// The content-addressed chunk ready for [`Store::apply_remote_chunks`].
    pub chunk: RemoteChunk,
    /// The op envelope the receiver must authorize before importing `chunk`.
    pub envelope: SyncOpEnvelope,
}

/// Collect the chunks `from` holds for `doc_id` that `into` lacks — one direction,
/// one doc — as [`StagedChunk`]s (content-addressed [`RemoteChunk`] + SS-7
/// [`SyncOpEnvelope`]) ready for an authorized atomic apply. Does NOT mutate
/// `into`; the caller stages every doc's missing chunks and applies the authorized
/// ones in ONE transaction per receiving store (review 088 #1).
///
/// Each chunk is keyed by its **content-addressed** exchanged id (not the origin's
/// local id), so the append-only apply guard is never tripped by two peers'
/// colliding `chunk-NNNN` sequences: a chunk `into` already has (same content id) is
/// an idempotent no-op, and a chunk with new content lands under a fresh id. The
/// envelope is recovered from `from`'s oplog by the chunk's *origin-local* id (kept
/// in [`ExchangedChunk`]), so the op kind + touched records reach the apply gate.
fn missing_chunks_for_doc(into: &Store, from: &Store, doc_id: &str) -> Result<Vec<StagedChunk>> {
    let have = frontier_for_doc(into, doc_id)?;
    let theirs = frontier_for_doc(from, doc_id)?;
    let oplog = oplog_index(from)?;
    let mut out = Vec::new();
    for (id, chunk) in &theirs {
        if have.contains_key(id) {
            continue; // frontier already covers this chunk — nothing to send
        }
        let envelope = envelope_for_chunk(doc_id, &chunk.local_id, &oplog);
        // Carry the chunk's ORIGINAL-author provenance (the original author recovered
        // when `from` only relayed this chunk, plus the touched record ids) INTO the
        // RemoteChunk, so `from`'s import oplog row — and therefore the next relay
        // hop — preserves the true author + record identity instead of attributing the
        // chunk to the importer (`review 092 #1/#2`).
        out.push(StagedChunk {
            chunk: RemoteChunk {
                doc_id: doc_id.to_string(),
                chunk_id: chunk.id.clone(),
                format: chunk.format.clone(),
                payload: chunk.payload.clone(),
                author_actor_id: envelope.origin_source.clone(),
                record_ids: envelope.record_ids.clone(),
            },
            envelope,
        });
    }
    Ok(out)
}

/// A coarse remote-source identifier for oplog tagging (M0b has no server
/// membership / source-token model): the source store's Loro peer id, which is
/// distinct per workspace, rendered as `peer:<id>`. Recorded as the imported op's
/// `actor_id` / `source` so audit can tell a remote import from a local write.
fn remote_source_id(from: &Store) -> String {
    format!("peer:{}", from.crdt_peer_id())
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
/// `into_indexes` is the active [`IndexManager`] of the **`into`** store whose
/// physical indexes are rebuilt with its projection (DL-6); pass
/// [`IndexManager::new`] when none are active. It must be `into`'s OWN manager —
/// index metadata is per-store and not part of the synced chunk payload, so
/// rebuilding `into` against a foreign manager would issue index DML for tables
/// `into` does not have (or skip the ones it does), leaving its indexes wrong.
pub fn pull(into: &mut Store, from: &Store, into_indexes: &IndexManager) -> Result<usize> {
    let doc_ids = union_doc_ids(into, from)?;
    let source = remote_source_id(from);
    // Stage every doc's missing chunks, then apply the whole batch + projection
    // rebuild in ONE transaction on `into` (review 088 #1): a failure rolls back
    // every imported chunk, its oplog row, and the rebuild together rather than
    // leaving committed chunks under a stale projection.
    let mut staged: Vec<RemoteChunk> = Vec::new();
    for doc_id in &doc_ids {
        staged.extend(
            missing_chunks_for_doc(into, from, doc_id)?
                .into_iter()
                .map(|s| s.chunk),
        );
    }
    into.apply_remote_chunks(&staged, &source, into_indexes)
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
/// chunks ([`SyncReport::total_chunks_moved`] is `0`).
///
/// Each store rebuilds against its OWN [`IndexManager`] — `a_indexes` for `a`,
/// `b_indexes` for `b` (DL-6). Index metadata is per-store and is NOT part of the
/// synced (chunk) payload, so the two peers may hold ASYMMETRIC active indexes
/// (e.g. one has an FTS index on a collection the other does not). Rebuilding both
/// against a single manager would be order-dependent and wrong: it would issue
/// index DML for tables the other store lacks, or skip the indexes that store
/// actually has and leave them stale. Pass [`IndexManager::new`] for a store with
/// no active indexes.
///
/// The exchange is staged so each direction diffs against the *pre-exchange*
/// frontier: `a`'s missing chunks are computed from `a`'s original frontier, not
/// one already mutated by `b`'s push. Each receiving store's whole update — every
/// imported chunk, its oplog row, AND the projection/index rebuild — commits or
/// rolls back together in ONE transaction ([`Store::apply_remote_chunks`], review
/// 088 #1): no longer per-chunk commits followed by a post-hoc rebuild that could
/// leave committed chunk/oplog rows under a stale projection.
pub fn sync_stores(
    a: &mut Store,
    a_indexes: &IndexManager,
    b: &mut Store,
    b_indexes: &IndexManager,
) -> Result<SyncReport> {
    // Both in-process peers are assumed already authorized (the M0b local CI seam):
    // an always-allow gate that imports every staged chunk. The SS-7 enforced path
    // is `sync_stores_authorized`, used by `WorkspaceCore::sync_with`.
    sync_stores_authorized(a, a_indexes, b, b_indexes, |_src, _env| true)
}

/// The SS-7 authorized bidirectional sync: like [`sync_stores`], but each staged
/// chunk is passed to `authorize` BEFORE it is imported into the receiving store.
/// `authorize(source, envelope)` returns `true` to import the chunk and `false`
/// to REJECT it (`forge/spec/sync-rbac.md` apply-time decision): a rejected chunk
/// is SKIPPED — never handed to [`Store::apply_remote_chunks`] — so the receiver's
/// CRDT history and projection are left unchanged for that op. `source` is the
/// `peer:<id>` id of the chunk's origin (the session actor the receiver resolves
/// trusted membership for); `envelope` is the op metadata the receiver inspects.
///
/// The authorizer is the caller's seam to resolve trusted membership, call
/// `authorize_remote_op`, write the audit record (allow AND deny), and surface a
/// `permission_denied`. This crate only enforces the *mechanism*: a denied chunk
/// is filtered out of the batch handed to the atomic per-store apply, so the
/// import never runs for it. The returned [`SyncReport::chunks_denied`] counts the
/// rejections across both directions.
///
/// Two staging fields the authorizer must honor for SS-7 correctness (review 092):
/// the envelope's [`origin_source`](SyncOpEnvelope::origin_source) names the chunk's
/// ORIGINAL author when `source` is only a relay (a forwarded foreign import), so
/// the receiver resolves trusted membership for the original actor, not the relay;
/// and [`malformed`](SyncOpEnvelope::malformed) flags a chunk whose doc id is not a
/// valid `collection/<name>` records doc, which the authorizer must deny fail-closed
/// before any grant check.
///
/// Staging, the pre-exchange-frontier symmetry, the per-store [`IndexManager`]
/// (review 084 #1), and the one-transaction atomic apply (review 088) are all
/// identical to [`sync_stores`] — authorization runs strictly between staging and
/// the import, so an allowed sync converges byte-identically to the unauthorized
/// path while a denied op is simply absent from the applied batch.
pub fn sync_stores_authorized(
    a: &mut Store,
    a_indexes: &IndexManager,
    b: &mut Store,
    b_indexes: &IndexManager,
    mut authorize: impl FnMut(&str, &SyncOpEnvelope) -> bool,
) -> Result<SyncReport> {
    let doc_ids = union_doc_ids(a, b)?;
    // Each peer tags the chunks it RECEIVES with the other peer's source id, so the
    // imported oplog rows are attributable (DL-4 remote parity).
    let a_source = remote_source_id(a);
    let b_source = remote_source_id(b);

    // Stage BOTH directions against the pre-exchange frontiers (reading only) before
    // mutating either store, so the two diffs are symmetric and neither sees the
    // other's just-applied chunks.
    let mut to_b: Vec<StagedChunk> = Vec::new(); // a's chunks b lacks
    let mut to_a: Vec<StagedChunk> = Vec::new(); // b's chunks a lacks
    for doc_id in &doc_ids {
        to_b.extend(missing_chunks_for_doc(b, a, doc_id)?);
        to_a.extend(missing_chunks_for_doc(a, b, doc_id)?);
    }

    // SS-7 gate: authorize each staged op BEFORE import. A denied chunk is dropped
    // from the batch, so `apply_remote_chunks` never imports it — the receiver's
    // history + projection stay unchanged for that op (`forge/spec/sync-rbac.md`).
    // `b` receives `a`'s chunks (source `a_source`); `a` receives `b`'s (source
    // `b_source`).
    let mut denied = 0usize;
    let allowed_to_b = filter_authorized(to_b, &a_source, &mut authorize, &mut denied);
    let allowed_to_a = filter_authorized(to_a, &b_source, &mut authorize, &mut denied);

    // Apply each direction atomically into the RECEIVING store, each against its OWN
    // index manager (review 084 #1) so asymmetric indexes stay correct and the
    // result is independent of which store is `a` vs `b`. The returned count is the
    // number of chunks NEWLY imported (idempotent re-imports add nothing).
    let chunks_a_to_b = b.apply_remote_chunks(&allowed_to_b, &a_source, b_indexes)?;
    let chunks_b_to_a = a.apply_remote_chunks(&allowed_to_a, &b_source, a_indexes)?;

    Ok(SyncReport {
        chunks_a_to_b,
        chunks_b_to_a,
        docs_synced: doc_ids.len(),
        chunks_denied: denied,
    })
}

/// Partition one direction's staged chunks by the authorization decision: return
/// the [`RemoteChunk`]s `authorize` allowed (ready for the atomic apply) and bump
/// `denied` for every rejection. A rejected chunk is simply excluded, so it never
/// reaches the receiving store's import.
fn filter_authorized(
    staged: Vec<StagedChunk>,
    source: &str,
    authorize: &mut impl FnMut(&str, &SyncOpEnvelope) -> bool,
    denied: &mut usize,
) -> Vec<RemoteChunk> {
    let mut allowed = Vec::with_capacity(staged.len());
    for StagedChunk { chunk, envelope } in staged {
        if authorize(source, &envelope) {
            allowed.push(chunk);
        } else {
            *denied += 1;
        }
    }
    allowed
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
