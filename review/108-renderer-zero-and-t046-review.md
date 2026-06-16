# Commit Review 108

Reviewed commits:

- `5385532b` renderer-zero: TS reference renderer + patch applier with golden conformance (UI-13)
- `651cf992` renderer-zero: fix UI-6 verbatim nesting + decorative-Icon a11y (UI-13 round 2)
- `293d23ee` renderer-zero: preserve verbatim non-string/absent `type` on UI-6 unknowns (UI-13 round 3)
- `cbf12128` renderer-zero: event emission + UI-6 fallback + a11y/focus rendering (UI-14)
- `4b040f1e` collab: delegate T046 to Codex -- IMPLEMENT DL-19 compaction + DL-21 tombstone GC
- `85296026` merge: renderer-zero (UI-13/14) -- TS reference renderer for the UI wire format
- `2840b478` forge-storage(codex): DL-19 compaction + DL-21 tombstone GC (T046)

## Findings

### P1: Compaction skips synced content-addressed chunks entirely

`forge/crates/storage/src/compaction.rs:157` derives the latest frontier with `filter_map(|c| c.frontier)`, and `:171` only folds chunks whose id parses as `chunk-NNNN` or `compact-NNNN`. That means chunks imported through the sync seam are invisible to compaction, because `forge/crates/sync/src/lib.rs:185` stores remote chunks under exchanged ids from `exchanged_chunk_id(...)` (`sha256:*`). A peer that receives most writes remotely can run `compact_history` forever and reclaim none of those `crdt_chunks` or `record.remote_import` oplog rows, which misses the DL-19 goal for the content-addressed growth path called out in T046. Please either teach compaction a safe horizon model for exchanged/content ids or persist enough logical sequence metadata at import time to compact remote chunks safely.

### P2: Compact snapshot chunks cannot pass current sync RBAC metadata checks

`forge/crates/storage/src/compaction.rs:326` writes a `history.compact` oplog payload with `doc_id`, `chunk_id`, `compact_to`, and `removed_chunks`, but no concrete `record_ids`. The sync layer recovers envelope metadata from that oplog row (`forge/crates/sync/src/lib.rs:312`), maps unknown kinds to a generic record write, and then `forge/crates/core/src/sync_rbac.rs:379` denies record writes whose `record_ids` list is empty. So if a compact snapshot ever needs to be sent as full-state/resync data, authorized sync will fail it closed even when the receiver has collection-level write permission. Please carry the affected record ids in the compaction row, or add an explicit full-state snapshot envelope that RBAC can validate intentionally.

### P2: Renderer-zero hard-codes the old layout Grid -> `grid` heuristic

`renderer-zero/src/render.ts:281` still treats the mere presence of `columns` or `rows` as enough to render `role="grid"`, and the new a11y test locks that behavior in at `renderer-zero/test/a11y.test.ts:59`. That conflicts with the active T045 handoff, which explicitly corrects the rule to "role grid only when genuinely interactive/data-grid, not merely because a columns prop is present." Once T045 lands, renderer-zero will disagree with the Rust UI contract and keep over-announcing ordinary layout grids to assistive tech. Please align renderer-zero and its test with the T045 rule when doing the a11y follow-up, ideally by requiring an explicit interactive/data-grid signal rather than `columns`/`rows` alone.

## Notes

- T046 is implemented and committed, and the workspace check I ran after the commit was green (`cargo test --workspace`; demo printed `REPLAY IDENTICAL: true`). The findings above are integration/coverage gaps to close before wiring compaction into sync or peer reset flows.
