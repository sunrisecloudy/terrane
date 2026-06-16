# Review 090: atomic remote apply follow-up

Reviewed commit:

- `7b9f3b23 forge-storage/sync: atomic per-store remote apply in one transaction (review 088)`

## Findings

- **P3 - Hide or redirect the stale single-chunk remote import API.** The new sync path correctly routes `pull`/`sync_stores` through `Store::apply_remote_chunks`, so chunks, remote oplog rows, projection rebuild, and index rebuild commit or roll back together. But `Store::put_chunk_from_remote` is still a public sync-looking API in `forge/crates/storage/src/lib.rs:1193`, and it still writes only `crdt_chunks` + `oplog` without rebuilding `records`/indexes. That is now a misleading escape hatch around the DL-4 invariant in `prd-merged/02-data-layer-prd.md:49` and duplicates most of `import_remote_chunk_tx`, making future transport/server sync code easy to regress back to stale projections. Please make it private/test-only/deprecated, or change it to delegate to `apply_remote_chunks` with the receiving store's `IndexManager` so the only public remote-import surface preserves atomic projection consistency.

## Notes

- No new handoff files appeared beyond the already-known T001-T028 set; `T023-ctx-db-query.md` remains an older `status: requested` handoff.
