# Review 004: forge-crdt Loro docs

Date: 2026-06-12

Reviewed commit:

- `eb964d9` forge-crdt: CrdtDoc trait + Loro records/text docs with 2-peer convergence

## Summary

Good direction: the CRDT crate introduces a clear boundary, native tests pass, and the record/text convergence tests are a useful first scaffold. The main issue is that `set_record` currently behaves like full replacement and can delete omitted fields, which conflicts with the forward-compat/read-modify-write rule in the PRD.

## Findings

### [P1] `set_record` can strip fields omitted by an older/stale client

`prd-merged/02-data-layer-prd.md:59` says unknown fields and schema features must survive read-modify-write. The domain envelope also encodes this explicitly: `forge/crates/domain/src/record.rs:33-36` stores `unknown_fields`, and `merge_known` preserves them instead of removing missing fields.

In the CRDT layer, `set_record` collects existing keys and deletes every key not present in the incoming object at `forge/crates/crdt/src/lib.rs:108-116`. That makes `set_record("rec_1", {"title": "new"})` erase `tag`, `f_future`, or any field the caller omitted. In collaborative/local-first terms, a stale client doing a partial update can strip data it did not understand.

Please split the API into two semantics:

- `patch_record_fields(record_id, fields)`: insert/update only, never delete omitted fields. This should be the default path for `record.patch` and older-client read-modify-write.
- `replace_record_fields(record_id, fields)` or explicit field-deprecation/delete ops: allowed only when the caller intentionally supplies deletion semantics.

Add a convergence test where both peers start from `{title, tag}`, peer A changes `title`, peer B changes `tag`, and the merge keeps both fields. Add another future-field case mirroring `domain::record::unknown_fields_survive_read_modify_write`.

### [P2] The tests do not cover concurrent edits to different fields on the same record

The current suite covers different records and the same scalar field, but not different fields in the same record. That is exactly the case where the delete-missing-field behavior can hide. Add this before higher layers start depending on `RecordsDoc::set_record`.

### [P2] M0a WASM lane remains blocked

No regression here, but the full workspace still cannot pass:

```text
cargo check --locked --target wasm32-unknown-unknown
```

It still fails on `rquickjs-sys` and `sqlite-wasm-rs`, as noted in reviews 002 and 003. `forge-domain` alone remains wasm-clean.

## Verification

- `cargo test --locked`: passed.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-domain`: passed.
- `cargo check --locked --target wasm32-unknown-unknown`: failed on native runtime/storage backend crates as noted above.
