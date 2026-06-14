//! Oplog rows: the append-only op substrate (DL-4) the `crdt` crate folds into
//! the projection on rebuild, in deterministic `(lamport, op_id)` order.

use forge_domain::Result;
use rusqlite::params;

use crate::errors::map_sql;
use crate::store::{now_ms, Store};

impl Store {
    // --- Oplog (append-only substrate, DL-4) -----------------------------

    /// Append one op to the oplog. `op_id` is the primary key; appending the
    /// same id twice is a `StorageError` (the substrate is append-only).
    #[allow(clippy::too_many_arguments)]
    pub fn append_op(
        &self,
        op_id: &str,
        actor_id: &str,
        workspace_id: &str,
        lamport: u64,
        kind: &str,
        payload: &[u8],
    ) -> Result<()> {
        self.conn
            .execute(
                "INSERT INTO oplog
                     (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![op_id, actor_id, workspace_id, lamport as i64, kind, payload, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Read every oplog entry, ordered by `(lamport, op_id)` — a deterministic
    /// total order for replay/rebuild.
    pub fn list_ops(&self) -> Result<Vec<OpRow>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT op_id, actor_id, workspace_id, lamport, kind, payload
                   FROM oplog ORDER BY lamport, op_id",
            )
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(OpRow {
                    op_id: row.get(0)?,
                    actor_id: row.get(1)?,
                    workspace_id: row.get(2)?,
                    lamport: row.get::<_, i64>(3)? as u64,
                    kind: row.get(4)?,
                    payload: row.get(5)?,
                })
            })
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(map_sql)?);
        }
        Ok(out)
    }
}

/// A row read back from the oplog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpRow {
    pub op_id: String,
    pub actor_id: String,
    pub workspace_id: String,
    pub lamport: u64,
    pub kind: String,
    pub payload: Vec<u8>,
}
