//! Records projection reads/writes (DL-4) and the transaction-scoped record
//! helpers + stable `field_id` materialization the mutation paths share.

use forge_domain::{RecordEnvelope, Result};
use rusqlite::{params, OptionalExtension};

use crate::errors::{map_json, map_sql};
use crate::index::IndexManager;
use crate::store::{now_ms, Store};

impl Store {
    // --- Records projection (DL-4) ---------------------------------------

    /// Materialize/overwrite a record in the projection. The full envelope is
    /// stored as canonical JSON `TEXT` in `records.data` (queryable via JSON1),
    /// and `collection`/`id` are kept as columns for the PK and lookups.
    pub fn put_record(&self, env: &RecordEnvelope) -> Result<()> {
        let data = serde_json::to_string(env).map_err(|e| map_json("put_record", e))?;
        self.conn
            .execute(
                "INSERT INTO records (collection, id, data, updated_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(collection, id) DO UPDATE SET
                     data = excluded.data,
                     updated_at = excluded.updated_at",
                params![
                    env.collection.as_str(),
                    env.entity_id.as_str(),
                    data,
                    now_ms()
                ],
            )
            .map_err(map_sql)?;
        Ok(())
    }

    /// Read a single record back as a reconstructed envelope.
    pub fn get_record(&self, collection: &str, id: &str) -> Result<Option<RecordEnvelope>> {
        let data: Option<String> = self
            .conn
            .query_row(
                "SELECT data FROM records WHERE collection = ?1 AND id = ?2",
                params![collection, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(map_sql)?;
        match data {
            Some(json) => {
                let env = serde_json::from_str(&json).map_err(|e| map_json("get_record", e))?;
                Ok(Some(env))
            }
            None => Ok(None),
        }
    }

    /// List every record in `collection`, ordered by id, as envelopes.
    pub fn list_records(&self, collection: &str) -> Result<Vec<RecordEnvelope>> {
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM records WHERE collection = ?1 ORDER BY id")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map(params![collection], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let json = r.map_err(map_sql)?;
            out.push(serde_json::from_str(&json).map_err(|e| map_json("list_records", e))?);
        }
        Ok(out)
    }
}

/// Advance a record's `updated_at` to `logical_at` when supplied (never
/// backwards), leaving it untouched otherwise.
pub(crate) fn bump_updated_at(env: &mut RecordEnvelope, logical_at: Option<i64>) {
    if let Some(at) = logical_at {
        let ts = forge_domain::LogicalTimestamp(at.max(0) as u64);
        if ts > env.updated_at {
            env.updated_at = ts;
        }
    }
}

/// The stable `field_id` (DL-7) a display field name maps to in the M0a mutation
/// surface: `f_<name>`. This is the projection-side stand-in for the schema
/// registry's name→stable-id mapping (the registry mints ids from `ActorId` for
/// schema-defined fields; the DL-17 mutation surface writes display fields and
/// must materialize the matching stable id so expression/FTS indexes keyed by
/// `$.field_ids.<id>` actually see the record — review 045/046 finding 1).
fn field_id_for_name(name: &str) -> String {
    format!("f_{name}")
}

/// Re-derive `env.field_ids` from the record's display `fields` so a record
/// written through the DL-17 mutation surface is visible to indexes keyed by the
/// stable field id (review 045/046 finding 1). Without this, `RecordEnvelope::new`
/// leaves `field_ids` empty and an inserted/patched record is invisible to active
/// expression/FTS indexes until a manual rebuild.
///
/// Each display field `<name>` materializes/refreshes `field_ids["f_<name>"] =
/// value`, **layered on top of** any existing stable ids rather than rebuilding
/// the map. Existing schema-minted ids (e.g. `f_alice_0`, `f_dev.01_0`) are
/// PRESERVED — a display-name `update`/`patch` must never drop them, which would
/// strand the record from an expression/FTS index keyed to the real schema id and
/// (because active FTS sync deletes-then-reinserts) make it disappear from search
/// after an unrelated mutation (review 049). A brand-new insert starts with empty
/// `field_ids`, so this yields exactly the `f_<name>` stand-ins.
///
/// Trade-off (M0a): storage has no schema name→id map, so a field patched away
/// leaves its stand-in behind rather than risk dropping a real schema id; a stale
/// stand-in is a far lesser fault than corrupting a record's stable identity.
pub(crate) fn materialize_field_ids(env: &mut RecordEnvelope) {
    for (name, value) in &env.fields {
        env.field_ids.insert(field_id_for_name(name), value.clone());
    }
}

// --- Transaction-scoped record helpers (for grouped mutations) -------------

/// Read a record inside an open transaction (the tx-scoped form of
/// [`Store::get_record`]). Public so a caller composing a multi-write atomic
/// commit (e.g. the core's CR-7 `purge_data` uninstall) can read-modify-write
/// records inside one [`Store::transact`] closure.
pub fn get_record_tx(
    tx: &rusqlite::Transaction<'_>,
    collection: &str,
    id: &str,
) -> Result<Option<RecordEnvelope>> {
    let data: Option<String> = tx
        .query_row(
            "SELECT data FROM records WHERE collection = ?1 AND id = ?2",
            params![collection, id],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql)?;
    match data {
        Some(json) => Ok(Some(
            serde_json::from_str(&json).map_err(|e| map_json("get_record_tx", e))?,
        )),
        None => Ok(None),
    }
}

/// Upsert a record inside an open transaction (the tx-scoped form of
/// [`Store::put_record`], the projection-only write that does NOT refresh active
/// FTS rows — use [`put_record_synced_tx`] for applet writes). Public so a caller
/// composing a multi-write atomic commit (e.g. the core's CR-7 `purge_data`
/// uninstall tombstoning) can write records inside one [`Store::transact`].
pub fn put_record_tx(tx: &rusqlite::Transaction<'_>, env: &RecordEnvelope) -> Result<()> {
    let data = serde_json::to_string(env).map_err(|e| map_json("put_record_tx", e))?;
    tx.execute(
        "INSERT INTO records (collection, id, data, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(collection, id) DO UPDATE SET
             data = excluded.data,
             updated_at = excluded.updated_at",
        params![
            env.collection.as_str(),
            env.entity_id.as_str(),
            data,
            now_ms()
        ],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Upsert a record inside an open transaction AND refresh active FTS rows for it
/// in the same transaction (DL-5). The single seam every tx-scoped mutation uses,
/// so no applet write can leave an active FTS shadow table stale (review
/// 041/042 finding 3).
pub(crate) fn put_record_synced_tx(
    tx: &rusqlite::Transaction<'_>,
    env: &RecordEnvelope,
    indexes: &IndexManager,
) -> Result<()> {
    let data = serde_json::to_string(env).map_err(|e| map_json("put_record_synced_tx", e))?;
    put_record_tx(tx, env)?;
    indexes.sync_fts_for_record(
        tx,
        env.collection.as_str(),
        env.entity_id.as_str(),
        &data,
    )
}
