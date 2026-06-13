# CRDT Write Path and Projection Rebuild Fixtures

Source of record: `prd-merged/02-data-layer-prd.md` DL-4, DL-6, DL-17, DL-21, and the current `forge-crdt` / `forge-storage` APIs.

This document pins the workflow that `forge/fixtures/crdt-write/*.json` expects Rust to implement next.

## Scope

M0a scope:

- One local workspace file, one SQLite transaction per logical write or transaction group.
- `RecordsDoc` is the CRDT document for one collection. It stores a top-level Loro map named `records`, keyed by record id, with each value a nested map of fields.
- Storage persists append-only CRDT updates in `crdt_chunks`, append-only write metadata in `oplog`, and the derived query surface in `records`.
- Projection rebuild reconstructs the visible `records` projection from persisted CRDT chunks with zero diff from the incrementally maintained projection.

Deferred to M0b / later:

- Multi-peer transport, server fanout, and peer frontier negotiation.
- Compaction into snapshots and long-retention peer reset policy.
- Full public workspace schema columns beyond the current M0a subset, including `oplog.hlc`, `oplog.schema_ref`, `oplog.redaction_class`, and `crdt_chunks.start_frontier` / `end_frontier`.
- Hard purge and policy-proof deletion flows.

## Current APIs

`forge-crdt::RecordsDoc` currently provides the CRDT primitives the write path needs:

- `replace_record_fields(record_id, fields)` for insert/update-style exact field replacement.
- `patch_record_fields(record_id, fields)` for DL-9 partial updates that preserve omitted fields.
- `delete_record(record_id)` for whole-record CRDT delete. The record disappears from `get_record`, `list_record_ids`, and `materialized`; Loro retains the delete in history so it can propagate and rebuild.
- `version()`, `export_updates_since(version)`, `export_all_updates()`, and `import_updates(bytes)` for update chunks.
- `from_updates(peer_id, chunks)` / `from_updates_with_map(...)` for DL-6 rebuild from persisted chunks.

`forge-storage::Store` currently provides the substrate primitives:

- `transact(...)` for one SQLite transaction.
- `put_record`, `get_record`, `list_records`, `query` for the derived projection.
- `append_op` / `list_ops` for append-only write metadata ordered by `(lamport, op_id)`.
- `put_chunk`, `get_chunk`, `get_chunks` for append-only CRDT update chunks. Identical re-put is idempotent; changed payload under the same `(doc_id, chunk_id)` is rejected.

There is not yet a single typed storage API that performs the full DL-4 chain. The fixtures intentionally describe the desired orchestration.

## DL-4 Write Sequence

For a single record mutation, the implementation should:

1. Resolve `collection`, `doc_id`, `actor_id`, and the next logical `lamport`.
2. Enter one SQLite transaction before mutating CRDT or projection state.
3. Load or reconstruct the collection `RecordsDoc` from existing chunks for `doc_id`.
4. Capture `before_version = doc.version()`.
5. Apply the mutation:
   - `insert`: write the supplied fields for a new or previously deleted id.
   - `patch`: merge supplied fields into the existing record, preserving omitted fields.
   - `delete`: call `delete_record`, producing a CRDT delete/tombstone operation.
   - `transact`: apply all child mutations to the same document before a single commit/export when they share a document.
6. Commit the `RecordsDoc`.
7. Export `chunk_payload = doc.export_updates_since(before_version)`.
8. Append one immutable `crdt_chunks` row for that exported update.
9. Append one `oplog` row whose payload identifies the logical mutation, record id, doc id, chunk id, and projection effect.
10. Materialize the `records` projection from the post-mutation CRDT state inside the same transaction.
11. Commit SQLite only after the chunk, oplog row, and projection are all written.

If any step fails, the SQLite transaction rolls back and leaves no partial chunk, no partial op, and no partial projection row.

## Projection Rules

Fixture `expect_records` is the live, materialized read surface: an ordered list of `{id, fields}` for records visible after normal deletion filtering.

`expect_deleted_ids` pins delete/tombstone intent. Implementations may retain tombstone rows in the `records` projection as long as normal reads hide them and rebuild reproduces the same retained tombstone state. If the implementation stores only visible rows in `records`, the ids in `expect_deleted_ids` must still be derivable from CRDT/op history.

For fields:

- `patch` must preserve omitted fields, including forward-compatible / unknown values.
- `insert` after a delete for the same id creates the visible record again.
- Rebuild comparison uses canonical JSON ordering and compares semantic values, not wall-clock timestamps.

## DL-6 Rebuild Contract

For each fixture:

1. Apply the ordered fixture operations through the DL-4 write sequence.
2. Capture the incrementally maintained live projection as `expect_records`.
3. Drop or ignore the projection.
4. Rebuild a fresh `RecordsDoc` from the persisted chunks for the fixture's `doc_id`.
5. Materialize `records` again from that rebuilt document.
6. Assert `rebuild_equals_projection == true`: rebuilt projection equals the incremental projection exactly.

When the fixture includes `rebuild_chunk_order`, rebuild must use that chunk order instead of write order. This pins the Loro update property that duplicate and reordered chunks converge to the same materialized state.

## Fixture Shape

Each case has this shape:

```json
{
  "version": 1,
  "case": "insert_patch_delete_rebuild",
  "collection": "tasks",
  "doc_id": "collection/tasks",
  "chunk_format": "loro",
  "ops": [
    {"op": "insert", "id": "t1", "fields": {"title": "a"}},
    {"op": "patch", "id": "t1", "fields": {"done": true}},
    {"op": "delete", "id": "t1"}
  ],
  "expect_records": [],
  "expect_deleted_ids": ["t1"],
  "expect_chunk_count": 3,
  "rebuild_equals_projection": true
}
```

The JSON fixtures deliberately do not contain real Loro bytes. They are semantic vectors for the Rust fixture runner: the runner should generate chunks by applying the operations to `RecordsDoc`, then verify storage rows and rebuild output.

## Result

The current `RecordsDoc` API can express the fixture operations: insert/update-style replacement, patch-with-preserve, whole-record delete, incremental update export, and rebuild from update chunks.

The remaining implementation gap is in `forge-storage` / core orchestration: today `apply_mutation` updates projection and FTS, while `append_op` and `put_chunk` are separate primitives. The next Rust step should add a typed DL-4 write path that owns all three writes in one transaction and exposes the DL-6 rebuild path against stored chunks.
