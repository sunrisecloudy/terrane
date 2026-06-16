# Commit review: d7734569, aed6007e

Reviewed:

- `d7734569` (`forge-core/sync: authorize forwarded chunks against original author + fail-closed on malformed doc id`)
- `aed6007e` (`collab: delegate T029-T033 to Codex`)

## Findings

- [P2] Trusted forwarded authors still cannot successfully relay a write through another peer. The new relay path correctly switches authorization from the session relay to `origin_source` (`forge/crates/core/src/workspace.rs:473-486`), but when A re-exports C's chunk, A's `record.remote_import` oplog payload only stores `doc_id`, `chunk_id`, `kind`, and `source` (`forge/crates/storage/src/crdt_write.rs:577-582`). The sync staging code then reads `record_ids` from that payload and gets an empty list for every forwarded chunk (`forge/crates/sync/src/lib.rs:220-249` in `d7734569`), and the core adapter turns anything other than exactly one id into `record_id = None` (`forge/crates/core/src/workspace.rs:1604-1613`). So the new regression only proves "B does not trust C => deny"; the positive T029 case "B trusts C with db.write => apply" still fails before the grant check as a malformed/missing-record-id envelope. Please preserve the original touched record ids when importing/re-exporting remote chunks, or carry a validated non-empty record-id list through the core envelope, and add a C -> A -> B test where B trusts C and the relayed write actually lands.

## Handoff note

The new T029-T033 handoff files ask for broad spec/vector suites. I read all five; T029 already overlaps with untracked local work under `forge/fixtures/sync-provenance/` and modified `forge/crates/sync/src/*`, so I did not overwrite that in-progress implementation.
