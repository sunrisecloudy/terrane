//! Append-only `crdt_chunks` writes inside an open transaction: minting the next
//! immutable chunk id and the tx-scoped append (idempotent re-write, history-
//! rewrite guard), plus the `.optional_row()` read helper.

use crate::{map_sql, now_ms};
use forge_domain::{CoreError, Result};
use rusqlite::params;

/// Mint the next immutable chunk id for `doc_id`. Ids are `chunk-NNNN`, zero-padded
/// so lexical order matches insertion order (the `get_chunks` ordering tiebreak).
/// The sequence advances past both ordinary chunks and compact snapshots so new
/// writes after compaction never reuse an older frontier id. Computed inside the
/// open transaction so it sees only committed chunks.
pub(super) fn next_chunk_id(tx: &rusqlite::Transaction<'_>, doc_id: &str) -> Result<String> {
    let mut stmt = tx
        .prepare("SELECT chunk_id FROM crdt_chunks WHERE doc_id = ?1")
        .map_err(map_sql)?;
    let rows = stmt
        .query_map(params![doc_id], |row| row.get::<_, String>(0))
        .map_err(map_sql)?;
    let mut max_seq = 0u64;
    for row in rows {
        let chunk_id = row.map_err(map_sql)?;
        if let Some(seq) = chunk_id
            .strip_prefix("chunk-")
            .or_else(|| chunk_id.strip_prefix("compact-"))
            .and_then(|n| n.parse::<u64>().ok())
        {
            max_seq = max_seq.max(seq);
        }
    }
    Ok(format!("chunk-{:04}", max_seq + 1))
}

/// Append one immutable CRDT chunk inside an open transaction. Mirrors
/// [`Store::put_chunk`](crate::Store::put_chunk)'s append-only contract (review
/// 003) but tx-scoped: an identical re-write is an idempotent no-op, a conflicting
/// payload under an existing `(doc_id, chunk_id)` is a `StorageError`.
pub(super) fn put_chunk_tx(
    tx: &rusqlite::Transaction<'_>,
    doc_id: &str,
    chunk_id: &str,
    format: &str,
    payload: &[u8],
) -> Result<()> {
    let existing: Option<(String, Vec<u8>)> = tx
        .query_row(
            "SELECT format, payload FROM crdt_chunks WHERE doc_id = ?1 AND chunk_id = ?2",
            params![doc_id, chunk_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional_row()?;
    if let Some((ef, ep)) = existing {
        if ef == format && ep == payload {
            return Ok(());
        }
        return Err(CoreError::StorageError(format!(
            "crdt chunk ({doc_id}, {chunk_id}) is append-only and already exists \
             with different content; refusing to rewrite history"
        )));
    }
    tx.execute(
        "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![doc_id, chunk_id, format, payload, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// A tiny extension so the tx-scoped chunk read can use `.optional()` semantics
/// without pulling the whole `OptionalExtension` import into scope confusingly.
pub(super) trait OptionalRow<T> {
    fn optional_row(self) -> Result<Option<T>>;
}
impl<T> OptionalRow<T> for std::result::Result<T, rusqlite::Error> {
    fn optional_row(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(map_sql(e)),
        }
    }
}
