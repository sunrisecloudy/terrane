# Commit review: b56be674

Reviewed commit `b56be674` (`forge-core/sync: reject schema_write self-escalation + validate envelope metadata`).

## Findings

- [P2] Multi-record sync chunks are now denied before the collection grant check. `envelope_defect` requires every record write to carry a singular `record_id` (`forge/crates/core/src/sync_rbac.rs:362-368`), but the sync envelope is explicitly `(resource_type, op, collection, record_ids)` and allows group writes to carry a list of touched ids (`forge/crates/sync/src/lib.rs:96-120`). The wired adapter then maps `SyncRecordOp::Write` to `RemoteOp::Insert` and sets `record_id = None` unless there is exactly one id (`forge/crates/core/src/workspace.rs:1591-1613`); `record.transact` falls into that generic `Write` path (`forge/crates/sync/src/lib.rs:296-305`). A legitimate two-record `transact_mutations_crdt` chunk therefore becomes a record write with `record_id=None`, is rejected as malformed, skips CRDT import, and cannot sync even for a trusted editor/owner with the right collection `db.write` grant. Please preserve the fail-closed check for truly empty/unknown metadata, but thread a non-empty `record_ids` list through `RemoteOpEnvelope` (or otherwise let group writes satisfy metadata validation) and add a wired regression that syncs a two-record `record.transact` chunk through `sync_with`/the workspace authorization path.
