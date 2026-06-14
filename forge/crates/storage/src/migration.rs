//! The DL-13 **migration driver**: apply a [`MigrationDescriptor`] to every
//! record of a collection atomically, all-or-nothing, in ONE [`Store::transact`].
//!
//! prd-merged/02 DL-13: *"Logical migrations are oplog operations, never
//! destructive SQLite DDL."* The pure record transform lives in `forge-schema`
//! ([`migrate_record`]); this module is the storage side that drives it over the
//! projection, records the migration in the oplog, bumps the persisted
//! `schema_version`, and rebuilds active indexes — every step inside a single
//! transaction so any failure rolls the WHOLE migration back (`schema_version`,
//! every transformed record, the oplog op, and the indexes left exactly as they
//! were). See `forge/spec/migrations.md` §3–4.

use forge_domain::{CoreError, RecordEnvelope, Result};
use forge_schema::{migrate_record, MigrationDescriptor};
use rusqlite::params;

use crate::errors::{map_json, map_sql};
use crate::index::IndexManager;
use crate::kv::{kv_get_tx, kv_set_tx};
use crate::records::put_record_tx;
use crate::store::{now_ms, Store};

/// The KV namespace holding workspace metadata (mirrors the core's `__forge/meta`).
/// The migration driver persists `schema_version` here so it survives reopen and is
/// read by the sync envelope.
pub const META_NS: &str = "__forge/meta";

/// The KV key (within [`META_NS`]) holding the workspace's monotone schema version
/// as utf-8 decimal text. Absent → schema version `1` (every workspace starts at 1).
pub const SCHEMA_VERSION_KEY: &str = "schema_version";

/// The oplog `kind` for a recorded migration (DL-13 "migrations are oplog
/// operations"). One op per `apply_migration` records the version bump, the
/// collection, the transforms, and the affected record ids for replay.
pub const MIGRATION_OP_KIND: &str = "schema.migration";

/// The default schema version of a workspace that has never been migrated.
pub const INITIAL_SCHEMA_VERSION: u64 = 1;

/// The outcome of [`Store::apply_migration`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationOutcome {
    /// Whether records/version were actually changed. `false` means the workspace
    /// was already at the target version (idempotent no-op).
    pub applied: bool,
    /// The schema version after the call (the target on apply, or the unchanged
    /// current version on a no-op).
    pub schema_version: u64,
    /// How many records were transformed (0 on a no-op).
    pub migrated_records: usize,
}

impl Store {
    /// The workspace's current schema version (DL-13), defaulting to
    /// [`INITIAL_SCHEMA_VERSION`] when never migrated.
    pub fn schema_version(&self) -> Result<u64> {
        match self.kv_get(META_NS, SCHEMA_VERSION_KEY)? {
            Some(bytes) => parse_schema_version(&bytes),
            None => Ok(INITIAL_SCHEMA_VERSION),
        }
    }

    /// Apply a migration (DL-13) to every record of `descriptor.collection`,
    /// atomically and all-or-nothing.
    ///
    /// In ONE [`Store::transact`] (DL-4):
    /// 1. Read the persisted `schema_version`. If it already equals
    ///    `to_schema_version`, return the idempotent no-op (`applied: false`)
    ///    without touching anything. If it does not equal `from_schema_version`,
    ///    reject with [`CoreError::SchemaCompatibilityError`] (precondition unmet).
    /// 2. Apply [`migrate_record`] to every record of the collection (ordered by
    ///    id). The first record that cannot be transformed (e.g. a lossy narrow)
    ///    propagates its typed error out of the closure.
    /// 3. Write each migrated record back to the projection.
    /// 4. Append one `schema.migration` op to the oplog.
    /// 5. Bump the persisted `schema_version` to `to_schema_version`.
    /// 6. Rebuild active indexes from the migrated projection.
    ///
    /// Because all six run in the single transaction, ANY failure rolls back the
    /// whole migration — `schema_version`, every transformed record, the oplog op,
    /// and the indexes are left exactly as before. `records` is a projection; the
    /// canonical CRDT chunks are untouched, so a rollback reproduces the
    /// pre-migration state.
    pub fn apply_migration(
        &mut self,
        descriptor: &MigrationDescriptor,
        indexes: &IndexManager,
    ) -> Result<MigrationOutcome> {
        // Structural validation up front (pure; independent of any record).
        descriptor.validate()?;
        self.transact(|tx| apply_migration_tx(tx, descriptor, indexes))
    }
}

/// The DL-13 migration body, inside a caller-provided transaction. Split out so
/// the whole sequence shares one commit/rollback boundary (the all-or-nothing
/// guarantee). See [`Store::apply_migration`].
fn apply_migration_tx(
    tx: &rusqlite::Transaction<'_>,
    descriptor: &MigrationDescriptor,
    indexes: &IndexManager,
) -> Result<MigrationOutcome> {
    let current = read_schema_version_tx(tx)?;

    // (1) Idempotent no-op: already at the target version.
    if current == descriptor.to_schema_version {
        return Ok(MigrationOutcome {
            applied: false,
            schema_version: current,
            migrated_records: 0,
        });
    }
    // (1) Precondition: the migration only applies from its declared base version.
    if current != descriptor.from_schema_version {
        return Err(CoreError::SchemaCompatibilityError(format!(
            "migration precondition unmet: workspace is at schema_version {current}, \
             migration expects {} (to {})",
            descriptor.from_schema_version, descriptor.to_schema_version
        )));
    }

    // (2)+(3) Transform every record of the collection and write it back. A record
    // that cannot be transformed propagates its error, rolling the tx back.
    let priors = list_collection_records_tx(tx, &descriptor.collection)?;
    let mut record_ids = Vec::with_capacity(priors.len());
    for prior in &priors {
        let migrated = migrate_record(prior, descriptor)?;
        write_migrated_record_tx(tx, &migrated, indexes)?;
        record_ids.push(prior.entity_id.as_str().to_string());
    }
    let migrated_records = record_ids.len();

    // (4) Record the migration in the oplog (DL-13 "migrations are oplog ops").
    append_migration_op_tx(tx, descriptor, &record_ids)?;

    // (5) Bump the persisted schema version.
    write_schema_version_tx(tx, descriptor.to_schema_version)?;

    // (6) Rebuild active indexes from the migrated projection, in the SAME tx, so
    // an index failure rolls the whole migration back.
    indexes.rebuild_active(tx)?;

    Ok(MigrationOutcome {
        applied: true,
        schema_version: descriptor.to_schema_version,
        migrated_records,
    })
}

/// Read every record of `collection` from the projection inside the tx, ordered by
/// id (deterministic), as envelopes.
fn list_collection_records_tx(
    tx: &rusqlite::Transaction<'_>,
    collection: &str,
) -> Result<Vec<RecordEnvelope>> {
    let mut stmt = tx
        .prepare("SELECT data FROM records WHERE collection = ?1 ORDER BY id")
        .map_err(map_sql)?;
    let rows = stmt
        .query_map(params![collection], |row| row.get::<_, String>(0))
        .map_err(map_sql)?;
    let mut out = Vec::new();
    for r in rows {
        let json = r.map_err(map_sql)?;
        out.push(serde_json::from_str(&json).map_err(|e| map_json("migration list", e))?);
    }
    Ok(out)
}

/// Write a migrated record back to the projection and refresh active FTS rows in
/// the same tx (DL-5), so the search shadow follows the migrated value.
fn write_migrated_record_tx(
    tx: &rusqlite::Transaction<'_>,
    env: &RecordEnvelope,
    indexes: &IndexManager,
) -> Result<()> {
    let data = serde_json::to_string(env).map_err(|e| map_json("migration write", e))?;
    put_record_tx(tx, env)?;
    indexes.sync_fts_for_record(tx, env.collection.as_str(), env.entity_id.as_str(), &data)
}

/// Append the `schema.migration` oplog op recording this migration (DL-13). The
/// `op_id` is `migration#<from>-<to>#<collection>`, unique per version-pair and
/// collection. Payload keys land in alphabetical (BTreeMap) order, byte-stable.
fn append_migration_op_tx(
    tx: &rusqlite::Transaction<'_>,
    descriptor: &MigrationDescriptor,
    record_ids: &[String],
) -> Result<()> {
    let op_id = format!(
        "migration#{}-{}#{}",
        descriptor.from_schema_version, descriptor.to_schema_version, descriptor.collection
    );
    let mut payload = serde_json::Map::new();
    payload.insert("kind".into(), MIGRATION_OP_KIND.into());
    payload.insert("collection".into(), descriptor.collection.as_str().into());
    payload.insert("from".into(), descriptor.from_schema_version.into());
    payload.insert("to".into(), descriptor.to_schema_version.into());
    payload.insert(
        "transforms".into(),
        serde_json::to_value(&descriptor.transforms).map_err(|e| map_json("migration op", e))?,
    );
    payload.insert("record_ids".into(), record_ids.to_vec().into());
    let bytes = serde_json::to_vec(&serde_json::Value::Object(payload))
        .map_err(|e| map_json("migration op", e))?;
    // Use the target version as the oplog lamport so migrations sort after the
    // ops at the prior version in the deterministic `(lamport, op_id)` order.
    tx.execute(
        "INSERT INTO oplog
             (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            op_id,
            "local",
            "local",
            descriptor.to_schema_version as i64,
            MIGRATION_OP_KIND,
            bytes,
            now_ms()
        ],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Read the persisted schema version inside the tx (default
/// [`INITIAL_SCHEMA_VERSION`] when absent).
fn read_schema_version_tx(tx: &rusqlite::Transaction<'_>) -> Result<u64> {
    match kv_get_tx(tx, META_NS, SCHEMA_VERSION_KEY)? {
        Some(bytes) => parse_schema_version(&bytes),
        None => Ok(INITIAL_SCHEMA_VERSION),
    }
}

/// Persist the schema version inside the tx, as utf-8 decimal text (matching the
/// counter encoding `kv` uses).
fn write_schema_version_tx(tx: &rusqlite::Transaction<'_>, version: u64) -> Result<()> {
    kv_set_tx(
        tx,
        META_NS,
        SCHEMA_VERSION_KEY,
        version.to_string().as_bytes(),
        "text/plain",
    )
}

/// Parse a persisted schema version (utf-8 decimal). A malformed value is a
/// `StorageError` rather than a silent reset.
fn parse_schema_version(bytes: &[u8]) -> Result<u64> {
    let s = std::str::from_utf8(bytes)
        .map_err(|_| CoreError::StorageError("schema_version is not valid utf-8".into()))?;
    s.trim()
        .parse::<u64>()
        .map_err(|_| CoreError::StorageError(format!("schema_version {s:?} is not an integer")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{CollectionId, LogicalTimestamp, RecordId};
    use forge_schema::{FieldTransform, FieldType};
    use std::collections::BTreeMap;

    /// A store with one `expenses` record carrying an int `f_amount` and a display
    /// `amount`, plus an active expression index over `f_amount`.
    fn seeded_store() -> (Store, IndexManager) {
        let store = Store::open_in_memory().unwrap();
        let mut indexes = IndexManager::new();
        for (id, amount) in [("e1", 10i64), ("e2", 20)] {
            let mut env = RecordEnvelope::new(
                CollectionId::new("expenses"),
                RecordId::new(id),
                BTreeMap::from([("amount".to_string(), serde_json::json!(amount))]),
                LogicalTimestamp(1),
            );
            env.field_ids.insert("f_amount".into(), serde_json::json!(amount));
            store.put_record(&env).unwrap();
        }
        indexes
            .create_index(store.connection(), "expenses", "f_amount", crate::CreateIndexKind::Value)
            .unwrap();
        (store, indexes)
    }

    fn widen_amount_to_float() -> MigrationDescriptor {
        MigrationDescriptor {
            collection: "expenses".into(),
            from_schema_version: 1,
            to_schema_version: 2,
            transforms: vec![FieldTransform::WidenField {
                field_id: "f_amount".into(),
                to: FieldType::FloatNum,
            }],
        }
    }

    #[test]
    fn fresh_store_starts_at_schema_version_one() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.schema_version().unwrap(), INITIAL_SCHEMA_VERSION);
    }

    #[test]
    fn apply_migration_widens_all_records_and_bumps_version() {
        let (mut store, indexes) = seeded_store();
        let outcome = store.apply_migration(&widen_amount_to_float(), &indexes).unwrap();
        assert!(outcome.applied);
        assert_eq!(outcome.schema_version, 2);
        assert_eq!(outcome.migrated_records, 2);
        assert_eq!(store.schema_version().unwrap(), 2);
        // Every record's value is now a float.
        for id in ["e1", "e2"] {
            let env = store.get_record("expenses", id).unwrap().unwrap();
            assert!(env.field_ids["f_amount"].is_f64(), "{id} amount must be float");
        }
        // The migration is recorded in the oplog (DL-13).
        let ops = store.list_ops().unwrap();
        let migration = ops.iter().find(|o| o.kind == MIGRATION_OP_KIND).expect("migration op");
        let payload: serde_json::Value = serde_json::from_slice(&migration.payload).unwrap();
        assert_eq!(payload["from"], serde_json::json!(1));
        assert_eq!(payload["to"], serde_json::json!(2));
        assert_eq!(payload["collection"], serde_json::json!("expenses"));
        assert_eq!(payload["record_ids"], serde_json::json!(["e1", "e2"]));
    }

    #[test]
    fn already_applied_migration_is_idempotent_noop() {
        let (mut store, indexes) = seeded_store();
        let desc = widen_amount_to_float();
        assert!(store.apply_migration(&desc, &indexes).unwrap().applied);
        // Re-apply: already at v2 → no-op, no second oplog op.
        let again = store.apply_migration(&desc, &indexes).unwrap();
        assert!(!again.applied);
        assert_eq!(again.schema_version, 2);
        assert_eq!(again.migrated_records, 0);
        let migration_ops =
            store.list_ops().unwrap().into_iter().filter(|o| o.kind == MIGRATION_OP_KIND).count();
        assert_eq!(migration_ops, 1, "an already-applied migration adds no oplog op");
    }

    #[test]
    fn migration_with_unmet_precondition_is_rejected() {
        let (mut store, indexes) = seeded_store();
        // Descriptor claims to migrate from v5, but the store is at v1.
        let desc = MigrationDescriptor {
            collection: "expenses".into(),
            from_schema_version: 5,
            to_schema_version: 6,
            transforms: vec![],
        };
        let err = store.apply_migration(&desc, &indexes).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");
        // Version untouched.
        assert_eq!(store.schema_version().unwrap(), 1);
    }

    #[test]
    fn migration_failure_rolls_back_everything() {
        // FAULT INJECTION: a migration that fails on the SECOND record (a
        // non-integral float that cannot narrow to int) must roll back the ENTIRE
        // migration — schema_version, the FIRST record (already transformed in the
        // tx), and the oplog all unchanged.
        let mut store = Store::open_in_memory().unwrap();
        let indexes = IndexManager::new();
        // e1: integral float (coerces fine). e2: fractional float (fails to narrow).
        for (id, amount) in [("e1", 10.0f64), ("e2", 12.5)] {
            let mut env = RecordEnvelope::new(
                CollectionId::new("expenses"),
                RecordId::new(id),
                BTreeMap::new(),
                LogicalTimestamp(1),
            );
            env.field_ids.insert("f_amount".into(), serde_json::json!(amount));
            store.put_record(&env).unwrap();
        }
        let before_e1 = store.get_record("expenses", "e1").unwrap().unwrap();

        // A descriptor that narrows float → int. e1 (10.0) coerces, but e2 (12.5)
        // fails mid-migration.
        let desc = MigrationDescriptor {
            collection: "expenses".into(),
            from_schema_version: 1,
            to_schema_version: 2,
            transforms: vec![FieldTransform::WidenField {
                field_id: "f_amount".into(),
                to: FieldType::IntNum,
            }],
        };
        let err = store.apply_migration(&desc, &indexes).unwrap_err();
        assert_eq!(err.code(), "SchemaCompatibilityError");

        // FULL ROLLBACK: version unchanged...
        assert_eq!(store.schema_version().unwrap(), 1, "version must not advance on failure");
        // ...the first record is NOT half-migrated (still the original float)...
        let after_e1 = store.get_record("expenses", "e1").unwrap().unwrap();
        assert_eq!(after_e1, before_e1, "the first record must be rolled back, not half-migrated");
        assert!(after_e1.field_ids["f_amount"].is_f64());
        // ...and no migration op was committed.
        assert!(
            store.list_ops().unwrap().iter().all(|o| o.kind != MIGRATION_OP_KIND),
            "no migration op may survive a rolled-back migration"
        );
    }

    #[test]
    fn migration_records_persist_and_indexes_rebuild() {
        // After a widen, the active expression index still serves the field (it was
        // rebuilt from the migrated projection in the same tx).
        let (mut store, mut indexes) = seeded_store();
        store.apply_migration(&widen_amount_to_float(), &indexes).unwrap();
        // The manager's rebuild_active ran on the connection; re-create to confirm
        // the projection holds the migrated values the index would build from.
        indexes
            .create_index(store.connection(), "expenses", "f_amount", crate::CreateIndexKind::Value)
            .unwrap();
        let env = store.get_record("expenses", "e1").unwrap().unwrap();
        assert_eq!(env.field_ids["f_amount"].as_f64(), Some(10.0));
    }

    #[test]
    fn add_field_migration_fills_default_for_existing_records() {
        let (mut store, indexes) = seeded_store();
        let desc = MigrationDescriptor {
            collection: "expenses".into(),
            from_schema_version: 1,
            to_schema_version: 2,
            transforms: vec![FieldTransform::AddField {
                field_id: "f_currency".into(),
                name: "currency".into(),
                default: serde_json::json!("USD"),
            }],
        };
        let outcome = store.apply_migration(&desc, &indexes).unwrap();
        assert!(outcome.applied);
        for id in ["e1", "e2"] {
            let env = store.get_record("expenses", id).unwrap().unwrap();
            assert_eq!(env.field_ids["f_currency"], serde_json::json!("USD"));
            assert_eq!(env.fields["currency"], serde_json::json!("USD"));
        }
    }

    #[test]
    fn descriptor_validate_runs_before_touching_records() {
        let (mut store, indexes) = seeded_store();
        // to <= from is a structural ValidationError — rejected before any record.
        let desc = MigrationDescriptor {
            collection: "expenses".into(),
            from_schema_version: 2,
            to_schema_version: 1,
            transforms: vec![],
        };
        let err = store.apply_migration(&desc, &indexes).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert_eq!(store.schema_version().unwrap(), 1);
    }
}
