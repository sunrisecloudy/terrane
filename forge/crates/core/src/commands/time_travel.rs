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
//!     (a restore is a record WRITE), scoped from the TRUSTED grant table.
//!
//! Determinism (the SC-12/audit-log lesson): the restore's WHEN — the
//! `restored_logical_at` stamped on the new version — is a LOGICAL clock, never a
//! wall clock, so the restore replays deterministically. The caller may pin it
//! explicitly; absent, it is drawn from the workspace's monotone EventSink logical
//! clock (the same deterministic source the audit rows use), advanced once per
//! restore so two restores never collide.

use forge_domain::{CoreError, Result};

use super::super::auth::{require_db_read, require_db_write};
// The facade's deterministic event-clock helper: the restore stamps its LOGICAL
// `restored_logical_at` from the same monotone source the audit rows use (no wall
// clock on the replayable path). A private item of the parent `workspace` module is
// visible to this descendant command module.
use super::super::emit_event_logical_time;
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
    ///     clock) stamped on the new version's `updated_at`. When omitted it is drawn
    ///     from the workspace's monotone EventSink logical clock, so the restore is
    ///     replay-safe and two restores never collide. Recording the host call against
    ///     this logical clock keeps the WHEN deterministic.
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

        // Determinism: record the restore against the LOGICAL clock. When the caller
        // did not pin `restored_logical_at`, advance the workspace's monotone EventSink
        // clock once (a replay-safe source — no wall clock) so the new version's WHEN
        // is deterministic and two restores never share a timestamp.
        let logical_at = match restored_logical_at {
            Some(at) => at,
            None => {
                let at = emit_event_logical_time(
                    &mut self.events,
                    "db.restore",
                    serde_json::json!({
                        "collection": collection,
                        "id": id,
                        "to_version": to_version,
                    }),
                );
                // The EventSink clock is a u64; the storage path takes the LOGICAL
                // timestamp as an i64 (mirroring the spine's `logical_at`). The clock
                // never approaches i64::MAX in practice, so the cast is total here.
                at as i64
            }
        };

        let new_version = self.store.restore_record(
            &collection,
            &id,
            to_version,
            Some(logical_at),
            &self.indexes,
        )?;

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
