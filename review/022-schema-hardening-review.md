# Review 022: schema hardening follow-up

Commit reviewed: `05b111a5` (`forge-schema: harden actor-scoped ids, id-first validation, encapsulation`)

## Findings

1. **[P2] Registry validation is still opt-in after serde deserialization.**  
   `SchemaRegistry` still derives `Deserialize` directly (`forge/crates/schema/src/registry.rs:98`), while the new invariant check lives in a separate `validated()` method (`forge/crates/schema/src/registry.rs:112`). That means any caller can deserialize a tampered registry with duplicate/future field ids and accidentally use it without validation; even the roundtrip test deserializes first and only validates afterward (`forge/crates/schema/src/registry.rs:1213`). Since this commit’s goal is to prevent bypassing invariants outside `SchemaChange`, consider implementing custom `Deserialize` for `SchemaRegistry` that calls `validate_invariants`, or providing a single ingestion API and avoiding direct trusted use of `serde_json::from_*` for registry state.

## Notes

- The actor-scoped id minting and id-first record validation now match the review 005 direction.
- No new files appeared in `task-between-claude-and-codex` during this check.
- The first schema test run hit stale build output; after `cargo clean -p forge-schema`, the suite rebuilt and passed.

## Verification

- `git show --check 05b111a5` passed.
- `cargo test --locked -p forge-schema` passed after clean rebuild.
- `cargo clippy --locked -p forge-schema -- -D warnings` passed.
- `cargo build --locked -p forge-schema --target wasm32-unknown-unknown` passed.
