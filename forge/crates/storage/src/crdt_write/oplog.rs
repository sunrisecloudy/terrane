//! Oplog metadata for the CRDT write path (DL-4): the logical-mutation `kind`
//! string, the tx-scoped oplog-row append, and the chunk→lamport derivation that
//! keeps the oplog's `(lamport, op_id)` total order matching write order.

use crate::{map_sql, now_ms, Mutation};
use forge_domain::Result;
use rusqlite::params;

/// The stable oplog `kind` string for a logical mutation, e.g. `record.insert`.
/// Matches the fixtures' `expect_oplog_kinds`.
pub(super) fn oplog_kind(m: &Mutation) -> &'static str {
    match m {
        Mutation::Insert { .. } => "record.insert",
        Mutation::Update { .. } => "record.update",
        Mutation::Patch { .. } => "record.patch",
        Mutation::Delete { .. } => "record.delete",
        Mutation::Transact { .. } => "record.transact",
    }
}

/// Append one oplog row inside an open transaction (the DL-4 write metadata that
/// identifies the logical mutation, its doc id, and the chunk it produced). The
/// `op_id` is `(doc_id)#(chunk_id)`, unique because chunk ids are unique per doc.
#[allow(clippy::too_many_arguments)]
pub(super) fn append_op_tx(
    tx: &rusqlite::Transaction<'_>,
    op_id: &str,
    actor_id: &str,
    workspace_id: &str,
    lamport: u64,
    kind: &str,
    payload: &[u8],
) -> Result<()> {
    tx.execute(
        "INSERT INTO oplog
             (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![op_id, actor_id, workspace_id, lamport as i64, kind, payload, now_ms()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Derive a monotone-ish lamport for the oplog from the chunk sequence number, so
/// the oplog's `(lamport, op_id)` total order matches write order without a
/// separate clock. `chunk-0007` → lamport 7. Malformed ids fall back to 0.
pub(super) fn chunk_id_lamport(chunk_id: &str) -> u64 {
    chunk_id
        .strip_prefix("chunk-")
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
}
