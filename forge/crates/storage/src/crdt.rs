//! CRDT blob bridges that live alongside the [`Store`] handle: the append-only
//! `crdt_chunks` rows (DL-4) and `crdt_snapshots` (DL-19), plus the public
//! single-chunk remote-import surface that funnels through the atomic apply
//! engine in [`crate::crdt_write`].

use forge_domain::{CoreError, Result};
use rusqlite::{params, OptionalExtension};

use crate::crdt_write::RemoteChunk;
use crate::errors::map_sql;
use crate::index::IndexManager;
use crate::store::{now_ms, Store};

impl Store {
    // --- CRDT blobs (DL-4 chunks, DL-19 snapshots) -----------------------

    /// Append a CRDT op chunk for `doc_id`.
    ///
    /// `crdt_chunks` is the append-only rebuild/sync source of truth (DL-6), so
    /// a `(doc_id, chunk_id)` is **immutable** once written: re-writing the same
    /// chunk with identical `(format, payload)` is an idempotent no-op
    /// (sync-safe — the same op chunk may arrive twice), but re-writing an
    /// existing chunk id with *different* content is rejected with
    /// `StorageError` rather than silently rewriting history (review 003).
    pub fn put_chunk(
        &self,
        doc_id: &str,
        chunk_id: &str,
        format: &str,
        payload: &[u8],
    ) -> Result<()> {
        if let Some(existing) = self.get_chunk(doc_id, chunk_id)? {
            if existing.format == format && existing.payload == payload {
                return Ok(()); // idempotent: identical chunk re-write
            }
            return Err(CoreError::StorageError(format!(
                "crdt chunk ({doc_id}, {chunk_id}) is append-only and already exists \
                 with different content; refusing to rewrite history"
            )));
        }
        self.conn
            .execute(
                "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![doc_id, chunk_id, format, payload, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Import a SINGLE CRDT chunk that arrived from a **remote peer** during sync,
    /// in the SAME `crdt_chunks` + `oplog` + `records`/index shape a local write
    /// uses (DL-4: "Remote updates follow the identical path"). The single-chunk
    /// convenience wrapper over [`apply_remote_chunks`](Self::apply_remote_chunks):
    /// it stages the one chunk and **delegates to the same atomic apply engine**, so
    /// it can NEVER leave a stale projection behind.
    ///
    /// ATOMIC PROJECTION CONSISTENCY (`review 090 #3`). An earlier version of this
    /// method wrote only `crdt_chunks` + `oplog` and skipped the projection/index
    /// rebuild — a public sync-looking escape hatch around the DL-4 atomic invariant
    /// (`prd-merged/02-data-layer-prd.md`) that could strand committed chunk/oplog
    /// rows under a `records` table that never saw the imported record. That footgun
    /// is retired: this now funnels through [`apply_remote_chunks`](Self::apply_remote_chunks),
    /// which — in ONE SQLite transaction — appends the chunk (append-only,
    /// idempotent), appends the matching `oplog` row, and rebuilds the `records`
    /// projection AND active physical indexes from the augmented chunk history. A
    /// failure anywhere rolls all of it back together, so the ONLY public
    /// remote-import surface keeps projection + indexes consistent.
    ///
    /// `indexes` must be the RECEIVING store's OWN [`IndexManager`](index::IndexManager)
    /// (review 084 #1): index metadata is per-store and not part of the synced chunk
    /// payload, so rebuilding against a foreign manager would issue index DML for
    /// tables this store lacks (or skip the ones it has). Pass
    /// [`IndexManager::new`](index::IndexManager::new) when none are active.
    ///
    /// Idempotence is load-bearing for sync: re-importing an already-present chunk
    /// (identical `(format, payload)` under the same `(doc_id, chunk_id)`) appends
    /// NO new chunk and NO new oplog row — a second sync of an already-converged pair
    /// adds nothing (it still rebuilds the projection to the identical state). A
    /// conflicting payload under an existing chunk id is rejected exactly as the
    /// append-only [`put_chunk`] guard does (history is never rewritten).
    ///
    /// PROVENANCE IS REQUIRED (`review 095`). Both this method and
    /// [`apply_remote_chunks`](Self::apply_remote_chunks) import through the SAME
    /// `import_remote_chunk_tx` engine, so neither can emit a provenance-poor
    /// `record.remote_import`:
    ///
    /// - `source` is the importing peer (the relay/session peer that handed us the
    ///   chunk), recorded as the oplog `actor_id`/`source` only when the chunk was
    ///   authored by that same peer (a first hop);
    /// - `author_actor_id` is the chunk's ORIGINAL author when the importing `source`
    ///   merely FORWARDED it (it imported the chunk from another peer and re-exports
    ///   it). When `Some`, the remote-import oplog row is stamped with the original
    ///   author, not the relay, so a later hop still gates the original actor
    ///   (`review 092 #1`);
    /// - `record_ids` are the records the chunk touched, persisted into the oplog row
    ///   so a later hop's authorization envelope still names a concrete record
    ///   (`review 092 #2`) instead of failing closed on dropped record identity.
    ///
    /// PROVENANCE IS VALIDATED AT THIS BOUNDARY (`review 096`). The spec forbids any
    /// import path from writing a provenance-poor `record.remote_import` row
    /// (`forge/spec/sync-rbac.md`), so this public API REJECTS provenance-poor input
    /// BEFORE delegating — the store is left completely unchanged on failure (no
    /// chunk, no oplog row, no projection change):
    ///
    /// - the effective original author (`author_actor_id` when forwarded, else
    ///   `source`) must be a non-blank, trimmed peer id — a blank actor would yield a
    ///   `record.remote_import` row attributable to no one;
    /// - the touched `record_ids` list must be NON-EMPTY and BLANK-FREE — every entry
    ///   must be non-blank after trim (`review 097`, strict reject-on-blank). A record
    ///   import that names no record would make the next relay hop recover an envelope
    ///   core policy must deny as missing a record id
    ///   (`forge/crates/core/src/sync_rbac.rs` envelope-metadata gate), and a list that
    ///   mixes a blank entry with a valid one (`&["", "t1"]`) would persist a
    ///   `record.remote_import` row whose recovered ids include one naming nothing. So
    ///   `&[]`, `&[""]`, AND `&["", "t1"]` are all rejected — there is no blank-id
    ///   loophole, and the persisted row's ids are trimmed to their canonical form.
    ///
    /// The batch path [`apply_remote_chunks`](Self::apply_remote_chunks) is fed only by
    /// the trusted internal sync seam (`forge_sync`), whose generic transact-group /
    /// unknown-op chunks legitimately carry an empty `record_ids` and are gated by the
    /// core authorization envelope instead; this single-chunk API is the public,
    /// caller-supplied surface, so the provenance floor is enforced here.
    ///
    /// Returns `true` if the chunk (and its oplog row) was newly imported, `false`
    /// if it was already present (idempotent no-op).
    #[allow(clippy::too_many_arguments)]
    pub fn put_chunk_from_remote(
        &mut self,
        doc_id: &str,
        chunk_id: &str,
        format: &str,
        payload: &[u8],
        source: &str,
        author_actor_id: Option<&str>,
        record_ids: &[&str],
        indexes: &IndexManager,
    ) -> Result<bool> {
        // Reject provenance-poor input BEFORE touching the store (review 096): the
        // effective original author must be a non-blank peer id, and a record import
        // must name a non-empty, blank-free list of touched record ids. Validating
        // here — ahead of the apply — means a rejected call leaves NO chunk and NO
        // oplog row.
        //
        // Trim the first-hop `source` and the optional original `author` up front so
        // the values that flow into the apply path are already canonical (review
        // 101): `import_remote_chunk_tx` persists `author_actor_id.unwrap_or(source)`
        // as BOTH the oplog `actor_id` and the payload `source`, so passing a padded
        // ` peer:A ` / ` peer:C ` would otherwise write non-canonical provenance even
        // though it passes the non-blank check below.
        let source = source.trim();
        let author_actor_id = author_actor_id.map(str::trim);
        let original_author = author_actor_id.unwrap_or(source);
        if original_author.is_empty() {
            return Err(CoreError::ValidationError(
                "put_chunk_from_remote: remote import has no original author/source \
                 (would write a provenance-poor record.remote_import)"
                    .into(),
            ));
        }
        // STRICT reject-on-blank contract (review 097): the list must be non-empty AND
        // EVERY entry must be non-blank after trim. We reject any blank entry outright
        // rather than silently filter it, so a caller cannot smuggle a blank id past
        // the floor (`&["", "t1"]`) and persist a `record.remote_import` row that a
        // later relay hop recovers as a record id naming nothing. `&[]` and `&[""]`
        // both fail this same check, so there is no provenance-poor loophole.
        if record_ids.is_empty() || record_ids.iter().any(|id| id.trim().is_empty()) {
            return Err(CoreError::ValidationError(
                "put_chunk_from_remote: remote import names no touched record id (the \
                 record_ids list must be non-empty and contain no blank entries — \
                 would write a provenance-poor record.remote_import)"
                    .into(),
            ));
        }

        // Build the same content + provenance unit the batch path imports, then
        // delegate to the ONE atomic apply path so the chunk, its oplog row, AND the
        // projection/index rebuild commit or roll back together (review 090 #3 — no
        // stale-projection escape hatch). The author, source, and ids are all trimmed
        // on the way in so the persisted RemoteChunk (and the oplog `actor_id` /
        // payload `source` derived from it) carries the exact canonical provenance a
        // downstream hop will recover (no surrounding whitespace — reviews 097, 101).
        let chunk = RemoteChunk {
            doc_id: doc_id.to_string(),
            chunk_id: chunk_id.to_string(),
            format: format.to_string(),
            payload: payload.to_vec(),
            author_actor_id: author_actor_id.map(str::to_string),
            record_ids: record_ids.iter().map(|s| s.trim().to_string()).collect(),
            // The public single-chunk import surface imports record-write chunks only;
            // a DL-13 migration chunk's version advance rides the batch sync seam
            // (review 139), so this path never carries one.
            schema_version: None,
        };
        // `apply_remote_chunks` reports the number of chunks newly imported (0 or 1
        // for a single chunk); map it back to this API's was-newly-written boolean.
        let imported = self.apply_remote_chunks(std::slice::from_ref(&chunk), source, indexes)?;
        Ok(imported == 1)
    }

    /// Read a single chunk by `(doc_id, chunk_id)`, if present.
    pub fn get_chunk(&self, doc_id: &str, chunk_id: &str) -> Result<Option<ChunkRow>> {
        self.conn
            .query_row(
                "SELECT chunk_id, format, payload FROM crdt_chunks
                  WHERE doc_id = ?1 AND chunk_id = ?2",
                params![doc_id, chunk_id],
                |row| {
                    Ok(ChunkRow {
                        chunk_id: row.get(0)?,
                        format: row.get(1)?,
                        payload: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(map_sql)
    }

    /// Read all chunks for `doc_id`, ordered by `(created_at, chunk_id)`.
    pub fn get_chunks(&self, doc_id: &str) -> Result<Vec<ChunkRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT chunk_id, format, payload FROM crdt_chunks
                  WHERE doc_id = ?1 ORDER BY created_at, chunk_id",
            )
            .map_err(map_sql)?;
        let rows = stmt
            .query_map(params![doc_id], |row| {
                Ok(ChunkRow {
                    chunk_id: row.get(0)?,
                    format: row.get(1)?,
                    payload: row.get(2)?,
                })
            })
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }

    /// List every distinct `doc_id` that has at least one persisted chunk, sorted.
    ///
    /// This is the union/iteration primitive the in-process sync seam needs
    /// (prd-merged/03 SS-1/SS-2): a peer advertises a frontier *per `doc_id`*, so
    /// the sync runner walks the union of doc ids across two stores. It is the
    /// public form of the `SELECT DISTINCT doc_id` the DL-6
    /// [`rebuild_projection`](Self::rebuild_projection) already does internally —
    /// exposed so the sync crate (and any future transport) can enumerate docs
    /// without reaching the raw connection. Includes non-collection docs (e.g. a
    /// future `src/<file>` text doc); callers filter by prefix as needed.
    pub fn list_doc_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT doc_id FROM crdt_chunks ORDER BY doc_id")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }

    /// Store a CRDT snapshot for `doc_id`.
    pub fn put_snapshot(
        &self,
        doc_id: &str,
        snapshot_id: &str,
        format: &str,
        payload: &[u8],
        frontier: &[u8],
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO crdt_snapshots
                     (doc_id, snapshot_id, format, payload, frontier, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(doc_id, snapshot_id) DO UPDATE SET
                     format = excluded.format,
                     payload = excluded.payload,
                     frontier = excluded.frontier,
                     created_at = excluded.created_at",
                params![doc_id, snapshot_id, format, payload, frontier, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// The most-recent snapshot for `doc_id` (by `created_at`, then
    /// `snapshot_id`), if any.
    pub fn latest_snapshot(&self, doc_id: &str) -> Result<Option<SnapshotRow>> {
        self.conn
            .query_row(
                "SELECT snapshot_id, format, payload, frontier FROM crdt_snapshots
                  WHERE doc_id = ?1 ORDER BY created_at DESC, snapshot_id DESC LIMIT 1",
                params![doc_id],
                |row| {
                    Ok(SnapshotRow {
                        snapshot_id: row.get(0)?,
                        format: row.get(1)?,
                        payload: row.get(2)?,
                        frontier: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(map_sql)
    }
}

/// A CRDT op chunk read back for a doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkRow {
    pub chunk_id: String,
    pub format: String,
    pub payload: Vec<u8>,
}

/// A CRDT snapshot read back for a doc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRow {
    pub snapshot_id: String,
    pub format: String,
    pub payload: Vec<u8>,
    pub frontier: Vec<u8>,
}
