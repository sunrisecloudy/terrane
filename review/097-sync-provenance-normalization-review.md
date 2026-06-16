# Review: sync provenance validation follow-up

Commit reviewed: `c5b58f36 forge-core/sync: authorize forwarded chunks against original author (review 092 #1)`.

## Finding

- [P2] Canonicalize or reject mixed blank `record_ids` before import. The new guard only checks that *some* supplied record id is non-blank (`forge/crates/storage/src/lib.rs:1252`), but then persists every raw caller-supplied string into the `RemoteChunk` (`forge/crates/storage/src/lib.rs:1268`, `forge/crates/storage/src/lib.rs:1269`). That means `put_chunk_from_remote(..., &["", "t1"])` passes validation even though the docs say blank entries are rejected, and the remote-import oplog row will contain both entries. On the next relay, `oplog_index` recovers those raw strings without trimming/filtering (`forge/crates/sync/src/lib.rs:235`, `forge/crates/sync/src/lib.rs:240`), then the core adapter treats any list that is not exactly one id as `record_id = None` (`forge/crates/core/src/workspace.rs:1610`, `forge/crates/core/src/workspace.rs:1612`), so a chunk with a valid touched id is denied as missing record metadata. Please either reject any blank entry (`all(|id| !trimmed.is_empty())`) or normalize by filtering/trimming before constructing `RemoteChunk`, and add a regression for mixed blank+valid ids so the public import API cannot persist an envelope that fails at the next hop.
