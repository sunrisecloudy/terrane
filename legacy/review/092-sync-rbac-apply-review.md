# Review 092: SS-7 apply-time authorization wiring

Reviewed commit:

- `26b30aac forge-core/sync: enforce SS-7 remote-op authorization before CRDT import`

## Findings

- **P2 - Authorize forwarded chunks against the original actor, not the relay peer.** The new gate uses `remote_source_id(from)` (`peer:<from.crdt_peer_id()>`) as the actor/source passed into `authorize_incoming_op`, and the receiver looks up membership by that source (`forge/crates/sync/src/lib.rs:291`, `forge/crates/core/src/workspace.rs:1533`). But once peer A has imported a chunk from peer C, A can later re-export that same content-addressed chunk to B; `oplog_index` sees A's `record.remote_import` row, ignores the payload's original `"source"`, maps it to a generic write, and B authorizes it as `peer:A` (`forge/crates/sync/src/lib.rs:184`, `forge/crates/storage/src/crdt_write.rs:577`). That bypasses SS-7's actor-identity check in `prd-merged/03-sync-server-prd.md:21`: B may trust A as an Owner while not trusting C, yet C's write lands via A. Please preserve the original author/source in `SyncOpEnvelope` and authorize re-exported chunks against that original actor, or fail closed on `record.remote_import` until original provenance is available. Add a three-peer regression: C writes, A imports, B trusts A but not C, then A->B must deny C's chunk.

- **P2 - The wired sync path still omits required schema/document metadata before allow.** `remote_op_envelope_from_sync` always creates record envelopes with `schema_version: None`, and it reduces generic/multi-record chunks to `RemoteOp::Insert` with optional/no `record_id` (`forge/crates/core/src/workspace.rs:1565`). In the sync layer, non-`collection/<name>` doc ids also fall back to `collection = doc_id` instead of being rejected (`forge/crates/sync/src/lib.rs:217`). This means the new `sync_with` path can allow a remote chunk without the document id/resource/schema-version validation required by `forge/spec/sync-rbac.md:52` and SS-7's schema-compatibility check. Please make staging fail closed when a chunk lacks a valid record doc id, op metadata, record/schema identity, or compatible schema version; add negative tests for malformed/non-record doc ids and missing schema version before import.

## Notes

- No new handoff files appeared beyond the already-known T001-T028 set; `T023-ctx-db-query.md` remains an older `status: requested` handoff.
