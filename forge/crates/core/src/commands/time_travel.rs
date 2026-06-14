//! `db.history` + `db.restore` — the DL-20 file-level time-travel commands
//! (prd-merged/02 DL-20, `forge/spec/time-travel.md`).
//!
//! These are the COMMAND boundary over the `forge-storage` time-travel substrate
//! (`Store::record_history` / `Store::restore_record`): the storage methods are the
//! pure read/append (they do not gate); this layer adds the RBAC the spec defers to
//! the caller (§5) and a deterministic, replay-safe write clock for the restore.
//!
//!   - [`cmd_db_history`](super::super::WorkspaceCore::cmd_db_history) returns the
//!     per-record change feed (WHO/WHEN/WHAT + state, oldest-to-newest). RBAC: the
//!     collection-scoped `db.read` capability, scoped from the TRUSTED grant table
//!     (keyed by the actor), never the request payload (review 048/050) — a read of a
//!     record's history is a data read, exactly like `query.execute`.
//!   - [`cmd_db_restore`](super::super::WorkspaceCore::cmd_db_restore) performs the
//!     NON-DESTRUCTIVE restore: it appends a NEW version equal to a prior state and
//!     returns that version. RBAC: the collection-scoped `db.write` capability
//!     (a restore is a record WRITE), scoped from the TRUSTED grant table. Because a
//!     restore is a committed mutation transaction (a new insert/delete on the target
//!     id), it drives the SAME DL-16 live-query notification turn as an already-
//!     committed `ctx.db` write, so an active `db.watch` over the collection observes
//!     the restore (review 167 P1).
//!
//! Determinism (the SC-12/audit-log lesson): the restore's WHEN — the
//! `restored_logical_at` stamped on the new version — is a LOGICAL clock, never a
//! wall clock, so the restore replays deterministically. The caller may pin it
//! explicitly; absent, it is derived as a MONOTONE default strictly greater than the
//! record's current logical clock / data frontier (the max `logical_at` across its
//! history), so the restored version is `> every prior change AND > the change it
//! undid` (review 167 P2). It is NOT the EventSink event counter, which starts
//! independently at 0 and could collide with / precede the record's seeded timestamps.

use forge_domain::{CoreError, Result};
use forge_storage::Mutation;

use super::super::auth::{require_db_read, require_db_write};
use super::super::WorkspaceCore;
use super::take_field;

impl WorkspaceCore {
    /// `db.history` — the DL-20 per-record change feed for `{collection, id}`
    /// (`forge/spec/time-travel.md` §2). Returns the ordered list of every version
    /// that touched the record, each carrying WHO (`actor`/`source`), WHEN
    /// (`logical_at`), WHAT (`kind`), and the record's full `state` at that version,
    /// oldest-to-newest by `version` (the chunk frontier == oplog lamport, a logical
    /// clock). The read is a pure replay of the append-only substrate, so it is
    /// deterministic and byte-stable across calls.
    ///
    /// RBAC (§5): the collection-scoped `db.read` capability ([`require_db_read`]),
    /// resolved from the workspace's TRUSTED grant table (keyed by the actor), never
    /// the request payload (review 048/050) — so a caller cannot widen its own read
    /// scope by editing the command body. A role/scope denial is returned BEFORE any
    /// history is read.
    pub(in crate::workspace) fn cmd_db_history(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let collection: String = take_field(cmd, "collection")?;
        let id: String = take_field(cmd, "id")?;

        // Capability gate (DL-20 §5 / CR-A3): role + the collection-scoped db.read
        // grant, read from the TRUSTED table — denied before any history is read.
        let trusted_scope = self.db_read_grants.get(cmd.actor.actor.as_str()).cloned();
        require_db_read(cmd, &collection, trusted_scope.as_deref())?;

        let feed = self.store.record_history(&collection, &id)?;
        let entries: Vec<serde_json::Value> = feed.iter().map(history_entry_to_json).collect();
        Ok(serde_json::json!({
            "collection": collection,
            "id": id,
            "entries": entries,
        }))
    }

    /// `db.restore` — the DL-20 NON-DESTRUCTIVE restore of `{collection, id}` to its
    /// state as of `to_version` (`forge/spec/time-travel.md` §3). Appends a NEW
    /// version equal to the prior state through the same DL-4 mutation path the live
    /// spine uses — it NEVER rolls back or rewrites prior history; every prior
    /// chunk/oplog row stays intact and reconstructable, and the change feed only
    /// grows. Returns the new version (the chunk frontier the restore appended) as the
    /// audit/undo handle, plus the record's resulting state.
    ///
    /// Payload `{ collection, id, to_version, restored_logical_at? }`:
    ///
    ///   - `to_version` is the target version (a past chunk frontier from
    ///     `db.history`); restoring a tombstoned/pre-history version makes the record
    ///     ABSENT as a new version (a delete), and restoring a live version onto a
    ///     deleted record reinserts it (DL-21 reinsert).
    ///   - `restored_logical_at` is the LOGICAL timestamp (a logical clock, NOT a wall
    ///     clock) stamped on the new version's `updated_at`. When omitted it is derived
    ///     as a MONOTONE default — strictly greater than the record's current logical
    ///     clock / data frontier (the max `logical_at` across its history) — so the new
    ///     version's WHEN is `> every prior change AND > the change it undid` (the
    ///     monotone restore-timestamp contract, `forge/spec/time-travel.md` §3). It is
    ///     NOT drawn from the EventSink event counter, which starts independently at 0
    ///     and could collide with / precede the record's own seeded timestamps (review
    ///     167 P2). When the caller PINS it, that value is honored verbatim.
    ///
    /// LIVE QUERIES (DL-16, review 167 P1): a restore is implemented as a new
    /// `record.insert` (restoring a live state) or `record.delete` (restoring a
    /// tombstone), so — like any committed mutation transaction — it dirties the target
    /// id and an active `db.watch` over the collection MUST be notified. The restore is
    /// therefore routed through the SAME notification path as an already-committed
    /// `ctx.db` write ([`notify_committed_mutations`](WorkspaceCore::notify_committed_mutations)):
    /// snapshot the watch registry BEFORE the restore, apply it through the storage
    /// time-travel path, then drive ONE committed-transaction notification turn AFTER
    /// the storage transaction commits (the write already landed; the turn computes its
    /// dirty set + notifications without re-applying it). A no-op restore (restoring a
    /// tombstone onto an already-absent record) writes nothing, so it produces no
    /// notification.
    ///
    /// RBAC (§5): the collection-scoped `db.write` capability ([`require_db_write`]) —
    /// a restore appends a new record version, i.e. it is a record WRITE — resolved
    /// from the workspace's TRUSTED grant table, never the request payload (review
    /// 048/050). A role/scope denial is returned BEFORE any version is appended.
    pub(in crate::workspace) fn cmd_db_restore(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let collection: String = take_field(cmd, "collection")?;
        let id: String = take_field(cmd, "id")?;
        let to_version: u64 = take_field(cmd, "to_version")?;
        let restored_logical_at = restored_logical_at(cmd)?;

        // Capability gate (DL-20 §5 / CR-A3): role + the collection-scoped db.write
        // grant, read from the TRUSTED table — denied before any version is appended.
        let trusted_scope = self.db_write_grants.get(cmd.actor.actor.as_str()).cloned();
        require_db_write(cmd, &collection, trusted_scope.as_deref())?;

        // Determinism + MONOTONICITY (review 167 P2): the restore stamps a LOGICAL
        // clock, never a wall clock. When the caller did not pin `restored_logical_at`,
        // derive a default that is strictly GREATER than the record's current clock /
        // data frontier — so the new version is `> every prior change AND > the change
        // it undid`. (The EventSink event counter starts independently at 0 and would
        // collide with / precede the record's own seeded timestamps.)
        let logical_at = match restored_logical_at {
            Some(at) => at,
            None => self.monotone_restore_clock(&collection, &id)?,
        };

        // DL-16 (review 167 P1): snapshot every registered watch's result ids BEFORE the
        // restore lands — the enter/leave/changed filter needs the pre-transaction
        // membership to tell a record that LEFT a watched result from one never in it.
        let before = self.watch_sessions.to_registry()?.snapshot(&self.store)?;
        // The ACTUAL mutation the restore appends (insert when restoring a live state,
        // delete when restoring a tombstone onto a live record, or none when restoring a
        // tombstone onto an already-absent record). Resolved BEFORE the write so we can
        // drive its notification turn afterward against the pre-write snapshot.
        let restore_write = self.resolve_restore_mutation(&collection, &id, to_version, logical_at)?;

        let new_version =
            self.store
                .restore_record(&collection, &id, to_version, Some(logical_at), &self.indexes)?;

        // DL-16 (review 167 P1): the restore is a committed mutation transaction, so an
        // active `db.watch` over the collection must be notified. Drive the SAME turn an
        // already-committed `ctx.db` write drives — the write already landed, so this
        // computes the dirty set + notifications (recorded, callback re-entered) WITHOUT
        // re-applying it. A no-op restore (`None`) wrote nothing and fires nothing.
        if let Some(mutation) = restore_write {
            self.notify_committed_mutations(cmd.actor.actor.as_str(), &[(mutation, before)])?;
        }

        let state = self.store.get_record(&collection, &id)?;
        Ok(serde_json::json!({
            "collection": collection,
            "id": id,
            "to_version": to_version,
            "restored_logical_at": logical_at,
            "new_version": new_version,
            "state": state.map(|env| serde_json::json!({
                "id": env.entity_id,
                "fields": env.fields,
            })),
        }))
    }

    /// The MONOTONE default `restored_logical_at` when the caller did not pin one
    /// (review 167 P2): strictly greater than the record's current logical clock / data
    /// frontier, so the restored version's WHEN is `> every prior change AND > the
    /// change it undid` (`forge/spec/time-travel.md` §3 monotone restore-timestamp
    /// contract). The frontier is the max `logical_at` across the record's whole history
    /// feed (every prior version's externally-supplied logical timestamp); `+1` puts the
    /// restore strictly after it. A record with no logical history yet starts the clock
    /// at `1`. This is a logical clock derived from the record's own history — NOT the
    /// EventSink event counter, which starts independently at 0 and could collide with /
    /// precede the record's seeded timestamps.
    fn monotone_restore_clock(&self, collection: &str, id: &str) -> Result<i64> {
        let frontier = self
            .store
            .record_history(collection, id)?
            .iter()
            .filter_map(|e| e.logical_at)
            .max()
            .unwrap_or(0);
        // The frontier is a u64 logical clock; the storage path takes the LOGICAL
        // timestamp as an i64 (mirroring the spine's `logical_at`). The clock never
        // approaches i64::MAX in practice, so the cast is total here.
        Ok(frontier as i64 + 1)
    }

    /// Resolve the ACTUAL mutation a restore to `to_version` will append, WITHOUT
    /// applying it (review 167 P1) — so the caller can drive its DL-16 notification turn
    /// against the pre-write watch snapshot. This mirrors the kind dispatch inside
    /// [`Store::restore_record`](forge_storage::Store::restore_record):
    ///
    ///   - the target version reconstructs to a LIVE state ⇒ a `record.insert` of its
    ///     display fields (a full (re)create, even over a tombstone — DL-21 reinsert);
    ///   - the target version is a TOMBSTONE/pre-history and the record is currently
    ///     LIVE ⇒ a `record.delete` (the record becomes absent as a new version);
    ///   - the target is a tombstone and the record is ALREADY absent ⇒ `None`: the
    ///     restore is a no-op that appends nothing and therefore fires no notification.
    fn resolve_restore_mutation(
        &self,
        collection: &str,
        id: &str,
        to_version: u64,
        logical_at: i64,
    ) -> Result<Option<Mutation>> {
        match self.store.record_state_at(collection, id, to_version)? {
            Some(env) => Ok(Some(Mutation::Insert {
                collection: collection.to_string(),
                id: Some(id.to_string()),
                fields: env.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                logical_at: Some(logical_at),
            })),
            None if self.store.get_record(collection, id)?.is_some() => Ok(Some(Mutation::Delete {
                collection: collection.to_string(),
                id: id.to_string(),
                logical_at: Some(logical_at),
            })),
            None => Ok(None),
        }
    }
}

/// Read the OPTIONAL `restored_logical_at` payload field as an `i64` LOGICAL
/// timestamp. Absent / null ⇒ `None` (the handler draws one from the EventSink
/// clock). A present-but-non-integer value is a `ValidationError` rather than a
/// silently-ignored field, so a malformed clock is surfaced.
fn restored_logical_at(cmd: &forge_domain::CoreCommand) -> Result<Option<i64>> {
    match cmd.payload.get("restored_logical_at") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => v.as_i64().map(Some).ok_or_else(|| {
            CoreError::ValidationError(format!(
                "db.restore `restored_logical_at` must be an integer logical timestamp, got {v}"
            ))
        }),
    }
}

/// Serialize one [`HistoryEntry`](forge_storage::HistoryEntry) to the wire change-feed
/// row shape (`forge/spec/time-travel.md` §2): `version`, `actor`, `source`,
/// `logical_at`, `kind`, and the record `state` (its display `id`/`fields`, or `null`
/// when the record was tombstoned at that version). The shape mirrors `query.execute`'s
/// row projection for `state`.
fn history_entry_to_json(entry: &forge_storage::HistoryEntry) -> serde_json::Value {
    serde_json::json!({
        "version": entry.version,
        "actor": entry.actor,
        "source": entry.source,
        "logical_at": entry.logical_at,
        "kind": entry.kind,
        "state": entry.state.as_ref().map(|env| serde_json::json!({
            "id": env.entity_id,
            "fields": env.fields,
        })),
    })
}
