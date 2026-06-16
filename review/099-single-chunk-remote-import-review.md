# Review: single-chunk remote import

Commit reviewed: `63c1cf1d forge-storage: retire non-atomic put_chunk_from_remote escape hatch (review 090 #3)`.

## Findings

- No actionable findings. The public single-chunk import wrapper now delegates to `apply_remote_chunks`, so it shares the atomic chunk/oplog/projection/index rebuild path instead of committing only chunk + oplog rows. The in-crate call sites were updated to the new `IndexManager`-aware signature.

Validation while reviewing:

- `rg -n 'put_chunk_from_remote\(' forge/crates forge -g '*.rs'`
