//! The public DL-17 mutation surface (`update`/`patch`/`delete`/`apply`/
//! `transact`) plus the transaction-scoped applier that keeps active FTS in sync.

use forge_domain::{CoreError, RecordEnvelope, Result};

use crate::index::IndexManager;
use crate::query::Mutation;
use crate::records::{bump_updated_at, get_record_tx, materialize_field_ids, put_record_synced_tx};
use crate::store::Store;

impl Store {
    // --- Mutations (DL-17) -----------------------------------------------

    /// Replace a record's known display fields (DL-17 `update`). Fields the
    /// caller does not mention are dropped from `fields`, but `field_ids`,
    /// `unknown_fields`, and `extensions` are preserved (DL-9). A missing record
    /// is a `QueryError`. `logical_at`, when given, advances `updated_at`.
    pub fn update_record(
        &self,
        collection: &str,
        id: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        logical_at: Option<i64>,
    ) -> Result<RecordEnvelope> {
        let mut env = self.get_record(collection, id)?.ok_or_else(|| {
            CoreError::QueryError(format!("update: record {collection}/{id} does not exist"))
        })?;
        env.fields = fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        bump_updated_at(&mut env, logical_at);
        self.put_record(&env)?;
        Ok(env)
    }

    /// Merge the supplied fields into a record (DL-17 `patch`), preserving fields
    /// the caller omits. A missing record is a `QueryError`. `logical_at`, when
    /// given, advances `updated_at`.
    pub fn patch_record(
        &self,
        collection: &str,
        id: &str,
        fields: &serde_json::Map<String, serde_json::Value>,
        logical_at: Option<i64>,
    ) -> Result<RecordEnvelope> {
        let mut env = self.get_record(collection, id)?.ok_or_else(|| {
            CoreError::QueryError(format!("patch: record {collection}/{id} does not exist"))
        })?;
        for (k, v) in fields {
            env.fields.insert(k.clone(), v.clone());
        }
        bump_updated_at(&mut env, logical_at);
        self.put_record(&env)?;
        Ok(env)
    }

    /// Tombstone a record (DL-17 `delete`, DL-21 sync-correct soft delete). The
    /// row is retained with `deleted = true` so the delete syncs; query hides it
    /// unless `includeDeleted`. A missing record is a `QueryError`.
    pub fn delete_record(
        &self,
        collection: &str,
        id: &str,
        logical_at: Option<i64>,
    ) -> Result<RecordEnvelope> {
        let mut env = self.get_record(collection, id)?.ok_or_else(|| {
            CoreError::QueryError(format!("delete: record {collection}/{id} does not exist"))
        })?;
        env.deleted = true;
        bump_updated_at(&mut env, logical_at);
        self.put_record(&env)?;
        Ok(env)
    }

    /// Apply a single [`Mutation`] outside a group (its own statement), keeping
    /// active FTS5 shadow tables in sync in the **same** transaction (DL-5/DL-17).
    ///
    /// This is the applet-facing DL-17 mutation surface, so it must not bypass
    /// dynamic-index maintenance: insert/update/patch/delete each refresh any
    /// active FTS rows for the touched record atomically with the projection
    /// write (review 041/042 finding 3). A nested `transact` here is rejected —
    /// use [`transact_mutations`](Self::transact_mutations) for groups.
    ///
    /// Pass the workspace's [`IndexManager`](index::IndexManager); when no FTS
    /// index is active the sync is a cheap no-op, but it can never be skipped by
    /// going through this surface (the unsynced [`put_record`](Self::put_record)
    /// family is reserved for projection rebuild, not applet writes).
    pub fn apply_mutation(
        &mut self,
        m: &Mutation,
        indexes: &IndexManager,
    ) -> Result<()> {
        if matches!(m, Mutation::Transact { .. }) {
            return Err(CoreError::QueryError(
                "nested transact is not allowed; pass items to transact_mutations".into(),
            ));
        }
        self.transact(|tx| {
            apply_mutation_tx(tx, m, indexes)?;
            Ok(())
        })
    }

    /// Apply a group of mutations as one local SQLite transaction (DL-17
    /// `transact`): all-or-nothing. A failure rolls back the whole group, so the
    /// projection is left byte-for-byte unchanged (reuses [`transact`](Self::transact)).
    ///
    /// Active FTS5 shadow tables are refreshed for every touched record inside
    /// the same transaction (DL-5), so a record inserted/patched here is
    /// immediately searchable without a manual rebuild (review 041/042 finding 3).
    ///
    /// Returns the number of leaf mutations applied. Items may themselves be a
    /// `transact` group; nested items are flattened into the same transaction.
    pub fn transact_mutations(
        &mut self,
        items: &[Mutation],
        indexes: &IndexManager,
    ) -> Result<usize> {
        // Borrow-checker: run inside one transaction by routing each leaf through
        // a tx-scoped applier that also keeps FTS in sync.
        self.transact(|tx| {
            let mut count = 0usize;
            for m in items {
                count += apply_mutation_tx(tx, m, indexes)?;
            }
            Ok(count)
        })
    }
}

/// Apply one mutation inside an open transaction, returning the number of leaf
/// mutations applied (so a nested `transact` counts each contained leaf). Every
/// projection write goes through the transaction and refreshes any active FTS
/// shadow rows in the same transaction, so a later failure rolls the whole group
/// back (DL-17 atomic-local) and an active FTS index never goes stale (DL-5).
fn apply_mutation_tx(
    tx: &rusqlite::Transaction<'_>,
    m: &Mutation,
    indexes: &IndexManager,
) -> Result<usize> {
    match m {
        Mutation::Insert {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let id = id.as_ref().ok_or_else(|| {
                CoreError::QueryError("insert requires a collection-scoped id".into())
            })?;
            let at = forge_domain::LogicalTimestamp(logical_at.unwrap_or(0).max(0) as u64);
            let mut env = RecordEnvelope::new(
                forge_domain::CollectionId::new(collection.clone()),
                forge_domain::RecordId::new(id.clone()),
                fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                at,
            );
            // Materialize the stable field ids the projection indexes read, so an
            // inserted record is visible to active expression/FTS indexes (review
            // 045/046 finding 1).
            materialize_field_ids(&mut env);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Update {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("update: record {collection}/{id} does not exist"))
            })?;
            env.fields = fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            materialize_field_ids(&mut env);
            bump_updated_at(&mut env, *logical_at);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Patch {
            collection,
            id,
            fields,
            logical_at,
        } => {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("patch: record {collection}/{id} does not exist"))
            })?;
            for (k, v) in fields {
                env.fields.insert(k.clone(), v.clone());
            }
            materialize_field_ids(&mut env);
            bump_updated_at(&mut env, *logical_at);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Delete {
            collection,
            id,
            logical_at,
        } => {
            let mut env = get_record_tx(tx, collection, id)?.ok_or_else(|| {
                CoreError::QueryError(format!("delete: record {collection}/{id} does not exist"))
            })?;
            env.deleted = true;
            bump_updated_at(&mut env, *logical_at);
            put_record_synced_tx(tx, &env, indexes)?;
            Ok(1)
        }
        Mutation::Transact { items } => {
            let mut count = 0usize;
            for item in items {
                count += apply_mutation_tx(tx, item, indexes)?;
            }
            Ok(count)
        }
    }
}
