# Review: sync provenance remote API

Commit reviewed: `2a6db80f forge-core/sync: authorize forwarded chunks against original author (review 092 #1)`.

## Finding

- [P2] `Store::put_chunk_from_remote` still creates provenance-poor remote imports. The new `RemoteChunk` path preserves forwarded provenance by carrying `author_actor_id` and `record_ids`, then writing `"source"` as the original author plus `"record_ids"` into the `record.remote_import` payload (`forge/crates/storage/src/crdt_write.rs:542`, `forge/crates/storage/src/crdt_write.rs:548`, `forge/crates/storage/src/crdt_write.rs:602`, `forge/crates/storage/src/crdt_write.rs:608`). But the still-public legacy API writes a remote import with only the immediate `source` and no `record_ids` (`forge/crates/storage/src/lib.rs:1193`, `forge/crates/storage/src/lib.rs:1247`, `forge/crates/storage/src/lib.rs:1248`, `forge/crates/storage/src/lib.rs:1257`). Any tool or caller that imports through this method can still produce chunks that lose the original author/touched-record envelope, reintroducing the relay authorization failure this commit is trying to close. Please remove/deprecate this API or make it delegate to the `RemoteChunk` import path with explicit original-author and record-id inputs, then add a regression proving it cannot emit a provenance-poor `record.remote_import`.

Validation while reviewing: `jq empty forge/fixtures/sync-provenance/*.json` passed.
