# Commit Review 103

Reviewed commits:

- `9f4d601a collab(codex): delegate T035-T045 backlog vectors + land T035 live-queries`
- `2780a644 forge-ui: validate accessibility inside Tabs panels + singular child (review 100)`
- `76565af7 forge-storage: canonicalize (trim) persisted remote-import author/source (review 101)`

## Findings

- **P2 - Record the full watch notification payload for replay.** `forge/spec/live-queries.md:65` defines the canonical callback payload with `reason`, `result_ids`, and `coalesced`, and `forge/spec/live-queries.md:121` says replay must emit the recorded notification sequence byte-for-byte without recomputing hooks. But the replay record example in `forge/spec/live-queries.md:108` and the pinned fixture at `forge/fixtures/live-queries/replay_records_notifications_identically.json:17` only persist `watch_id`, `version`, `collection`, and `record_ids`. A runner following this fixture cannot replay the callback byte-identically without either recomputing the omitted fields or delivering a smaller event than the one originally observed. Please make the run/session record store the full canonical notification payload, or explicitly change the callback contract so the recorded subset is the byte-identical event.

## Notes

No actionable findings in `2780a644`; `cargo test -p forge-ui --test accessibility` passed, and the Tabs panel traversal now has direct regression coverage.

No actionable findings in `76565af7`; `cargo test -p forge-storage legacy_put_chunk_from_remote_canonicalizes_persisted_author_and_source` passed, and the public import path now trims the values before constructing the persisted `RemoteChunk`.
