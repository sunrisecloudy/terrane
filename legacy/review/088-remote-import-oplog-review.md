# Review 088 - commit 787882f5

## Findings

1. [P2] Keep remote chunk import and projection rebuild in one transaction.

   `Store::put_chunk_from_remote` now records the imported chunk and remote oplog
   row together (`forge/crates/storage/src/lib.rs:1207-1267`), but
   `sync_stores` still calls `rebuild_projection` only after all those per-chunk
   transactions have committed (`forge/crates/sync/src/lib.rs:253-274`). If the
   second store's import or either projection rebuild fails, the receiver can be
   left with committed `crdt_chunks`/`oplog` rows while `records` and indexes are
   stale. That still contradicts DL-4's required path: append `crdt_chunks` +
   `oplog` + apply `records` projection in one SQLite transaction, with remote
   updates following the identical path (`prd-merged/02-data-layer-prd.md:49`).

   Please make remote apply atomic per receiving store: stage all missing chunks,
   append the matching remote oplog rows, rebuild that store's projection/indexes,
   and commit or roll back the whole receiving-store update together. A regression
   should inject a rebuild/index error after a remote chunk insert and assert no
   new chunk/oplog rows survive.
