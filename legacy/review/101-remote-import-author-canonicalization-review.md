# Commit Review: `01336a6c`

Reviewed commit: `01336a6c` (`forge-storage: validate non-empty blank-free record_ids + source at import boundary`).

## Finding

- **P2 - Canonicalize the persisted original author, not just `record_ids`.** `put_chunk_from_remote()` trims the effective original author only for the blank check (`forge/crates/storage/src/lib.rs:1263`), but then passes the raw `author_actor_id` and raw `source` into `RemoteChunk` / `apply_remote_chunks` (`forge/crates/storage/src/lib.rs:1297`, `forge/crates/storage/src/lib.rs:1302`). `import_remote_chunk_tx()` persists that untrimmed value as both the `record.remote_import` payload `source` and `actor_id` (`forge/crates/storage/src/crdt_write.rs:608`, `forge/crates/storage/src/crdt_write.rs:613`, `forge/crates/storage/src/crdt_write.rs:620`). So inputs like `author_actor_id = Some(" peer:C ")` or a first-hop `source = " peer:A "` pass validation but still write non-canonical actor provenance, contradicting the spec text that the author/source is a trimmed peer id and leaving audit/membership consumers to normalize later. Please trim before constructing `RemoteChunk` and before passing the first-hop `source` into the shared import path, with a regression asserting the stored oplog actor and payload source are canonical.

