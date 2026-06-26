---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/spec/crdt-write-path.md, forge/fixtures/crdt-write/*.json, forge/fixtures/crdt-write/manifest.json
---

# T024 — CRDT write-path + projection-rebuild fixtures (DL-4 / DL-6)

The next foundational workflow routes record writes through the CRDT op/oplog
(prd-merged/02 DL-4): mutation → Loro op → append crdt_chunks + oplog → materialize
the records projection, all in one SQLite transaction; and DL-6 projection rebuild
reconstructs `records` purely from the CRDT docs with zero diff. I want a spec +
fixtures pinning the invariants before I build the Rust.

## Deliverables

1. `forge/spec/crdt-write-path.md` — derive from the committed crates (read
   `forge/crates/crdt/src/lib.rs` RecordsDoc API, `forge/crates/storage/src/lib.rs`
   crdt_chunks/oplog/records, `prd-merged/02` DL-1..6): the write sequence, what an
   oplog row + a crdt_chunk hold per write, how the projection is materialized from
   the CRDT state, and the rebuild contract (DL-6: rebuild from chunks == current
   projection, zero diff). Note the M0a scope vs deferred (multi-peer sync is M0b).
2. `forge/fixtures/crdt-write/<case>.json` + manifest — each: an ordered list of
   record mutations (insert/patch/delete) on a collection, and the EXPECTED final
   state: the materialized records (id → fields), AND a `rebuild_equals_projection: true`
   marker (the rebuilt-from-CRDT projection must equal the incrementally-maintained one).
   ```json
   { "case": "insert_patch_delete_rebuild",
     "ops": [ {"insert": {"id":"t1","fields":{"title":"a"}}},
              {"patch":  {"id":"t1","fields":{"done":true}}},
              {"insert": {"id":"t2","fields":{"title":"b"}}},
              {"delete": {"id":"t1"}} ],
     "expect_records": [ {"id":"t2","fields":{"title":"b"}} ],
     "rebuild_equals_projection": true }
   ```

## Coverage (~10)

insert→read; patch preserves omitted fields (DL-9); delete tombstones; insert/patch/
delete sequence then rebuild equals projection; two records independently; re-insert
after delete; an unknown/forward-compat field preserved through a patch; an empty
collection rebuild.

In `## Result`, flag anything the current RecordsDoc API can't express yet (e.g. a
real delete/tombstone op, incremental update export) so I add exactly what the
fixtures need to the crdt crate.
