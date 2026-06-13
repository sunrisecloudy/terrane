# Review 008 - review-fixes commit e2a5d69

Buddy review for Claude on `e2a5d69 fix(review): storage append-only chunks, crdt patch/replace split, AGENTS.md v1`.

## Findings

- **No blocking findings in the committed diff.** The storage change removes the history-rewriting `ON CONFLICT DO UPDATE` path and rejects conflicting duplicate chunks while keeping identical replays idempotent (`forge/crates/storage/src/lib.rs:395`, `forge/crates/storage/src/lib.rs:872`). The CRDT change gives normal read-modify-write a DL-9-safe `patch_record_fields` path and keeps destructive replacement explicit (`forge/crates/crdt/src/lib.rs:92`, `forge/crates/crdt/src/lib.rs:112`). The new tests cover omitted/unknown field preservation plus concurrent patches to different fields of the same record (`forge/crates/crdt/src/lib.rs:317`, `forge/crates/crdt/src/lib.rs:457`). AGENTS now clearly scopes v0.4 rules away from `forge/` (`AGENTS.md:3`, `AGENTS.md:12`).

- **Residual note - full-workspace WASM is still not green.** `forge-crdt` is wasm-clean, but the full workspace check still fails on the existing `rquickjs-sys` and `sqlite-wasm-rs` build-script blockers. Not introduced here, but still relevant because the active M0a lane includes QuickJS-WASM.

## Verification

- `cargo test --locked` from `forge/`: passed.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` from `forge/`: passed.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-crdt` from `forge/`: passed.
- `cargo check --locked --target wasm32-unknown-unknown` from `forge/`: fails on `rquickjs-sys` and `sqlite-wasm-rs`; pre-existing full-WASM lane blocker.
