//! Index-synced record writes (DL-5 FTS maintenance): the `*_indexed` surface
//! that keeps active FTS5 shadow tables in sync with the projection in the SAME
//! transaction as the canonical `records` write.

use forge_domain::{CoreError, RecordEnvelope, Result};

use crate::errors::map_json;
use crate::index::IndexManager;
use crate::records::{bump_updated_at, get_record_tx, put_record_tx};
use crate::store::Store;

impl Store {
    // --- Index-synced record writes (DL-5 FTS maintenance) ----------------

    /// Put a record (as [`put_record`](Self::put_record)) **and** refresh any
    /// active FTS5 shadow rows for it in the **same** SQLite transaction (DL-5:
    /// FTS must be kept in sync on insert/update). Expression indexes are
    /// maintained by SQLite automatically, so only FTS needs the hand-sync; the
    /// canonical `records` write and the FTS refresh commit or roll back together.
    pub fn put_record_indexed(
        &mut self,
        env: &RecordEnvelope,
        indexes: &IndexManager,
    ) -> Result<()> {
        let data = serde_json::to_string(env).map_err(|e| map_json("put_record_indexed", e))?;
        let collection = env.collection.as_str().to_string();
        let id = env.entity_id.as_str().to_string();
        self.transact(|tx| {
            put_record_tx(tx, env)?;
            indexes.sync_fts_for_record(tx, &collection, &id, &data)
        })
    }

    /// Patch a record (as [`patch_record`](Self::patch_record)) and refresh active
    /// FTS rows for it in the same transaction (DL-5). Returns the merged
    /// envelope.
    pub fn patch_record_indexed(
        &mut self,
        collection: &str,
        id: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        logical_at: Option<i64>,
        indexes: &IndexManager,
    ) -> Result<RecordEnvelope> {
        self.transact(|tx| {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("patch: record {collection}/{id} does not exist"))
            })?;
            for (k, v) in fields {
                env.fields.insert(k.clone(), v.clone());
            }
            bump_updated_at(&mut env, logical_at);
            put_record_tx(tx, &env)?;
            let data =
                serde_json::to_string(&env).map_err(|e| map_json("patch_record_indexed", e))?;
            indexes.sync_fts_for_record(tx, collection, id, &data)?;
            Ok(env)
        })
    }

    /// Delete (tombstone) a record (as [`delete_record`](Self::delete_record)) and
    /// drop it from any active FTS shadow rows in the same transaction (DL-5): a
    /// deleted record stops matching text search. Returns the tombstoned envelope.
    pub fn delete_record_indexed(
        &mut self,
        collection: &str,
        id: &str,
        logical_at: Option<i64>,
        indexes: &IndexManager,
    ) -> Result<RecordEnvelope> {
        self.transact(|tx| {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("delete: record {collection}/{id} does not exist"))
            })?;
            env.deleted = true;
            bump_updated_at(&mut env, logical_at);
            put_record_tx(tx, &env)?;
            let data =
                serde_json::to_string(&env).map_err(|e| map_json("delete_record_indexed", e))?;
            indexes.sync_fts_for_record(tx, collection, id, &data)?;
            Ok(env)
        })
    }
}
