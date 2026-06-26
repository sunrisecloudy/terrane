//! `query.execute` — read the records projection for a collection, gated by the
//! collection-scoped `db.read` capability (CR-A2, DL-15). Moved verbatim from
//! `workspace.rs` (/simplify #11a).

use forge_domain::{CoreError, Result};

use super::super::auth::require_db_read;
use super::super::WorkspaceCore;

impl WorkspaceCore {
    /// `query.execute` — list every record in `collection` from the projection
    /// (CR-A2, DL-15 subset). Payload: `{ collection, grants? }`.
    ///
    /// `forge/spec/commands.md:21` requires **"Role plus db.read capability"**,
    /// and `forge/spec/capabilities.md:23` models `db.read` as a *collection-scoped*
    /// grant (`resource: collection:<name>`). Two independent layers gate the read
    /// (review 036/038 finding 1):
    ///
    ///   1. the command-level [`authorize`](super::super::auth::authorize) role gate
    ///      (a `Runner` is execution-only and cannot read data) — `PermissionDenied`;
    ///      then
    ///   2. the **collection-scoped `db.read` capability** ([`require_db_read`]):
    ///      the target `collection` must fall within the caller's granted
    ///      `db.read` scope (`payload.grants.db.read`, the same grant shape the
    ///      `forge/fixtures/query/reject_ungranted_collection.json` vector pins).
    ///      A collection outside the granted scope is `CapabilityRequired` —
    ///      enforced **before** `list_records` touches state — even for a role that
    ///      cleared layer 1. This is the caller boundary `forge-storage` defers to
    ///      (the projection scans any collection unguarded; the grant lives here).
    pub(in crate::workspace) fn cmd_query_execute(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let collection = cmd
            .payload
            .get("collection")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError("query.execute requires `collection`".into())
            })?;
        // Capability gate (CR-A3 / DL-15): role first, then the collection-scoped
        // db.read grant. The grant scope is read from the workspace's TRUSTED grant
        // table (keyed by the actor), never from the request payload (review 048
        // finding 1), so a caller cannot widen its own read scope. Denied before
        // any projection is read.
        let trusted_scope = self.db_read_grants.get(cmd.actor.actor.as_str()).cloned();
        require_db_read(cmd, collection, trusted_scope.as_deref())?;
        let records = self.store.list_records(collection)?;
        let rows: Vec<serde_json::Value> = records
            .into_iter()
            .map(|env| {
                serde_json::json!({
                    "id": env.entity_id,
                    "fields": env.fields,
                })
            })
            .collect();
        Ok(serde_json::json!({ "collection": collection, "rows": rows }))
    }
}
