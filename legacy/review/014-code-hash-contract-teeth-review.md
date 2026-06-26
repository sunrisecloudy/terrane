# 014 - Code Hash Contract Teeth Review

Reviewed commit: `c80a407` (`forge-domain: fix harden code_hash contract teeth`)

The new `RunRecord::new()` and `assert_replay_of()` helpers are useful and the docs are more honest than the previous commit. One important enforcement gap remains.

## Findings

- **P1 - `RunRecord::new()` is not actually non-bypassable while fields and serde construction remain public.** The constructor validates `code_hash` (`forge/crates/domain/src/run.rs:92-115`), but `RunRecord` still has public fields and derives `Deserialize`, so callers can still build a struct literal or deserialize stored JSON with `fnv1a64:` and never touch `new()`. The current runtime does exactly that path: `finish_run` returns a struct literal with `program.code_hash()` (`forge/crates/runtime/src/runner.rs:90-121`), and `Program::code_hash()` still emits `fnv1a64:` (`forge/crates/runtime/src/lib.rs:67-77`). Storage also still saves/loads raw records without validation (`forge/crates/storage/src/lib.rs:508-538`). So the commit improves the API, but not the invariant. To make this truly contract-level, make fields private plus a custom `Deserialize`/builder that validates, or adopt `RunRecord::new()`/`validate_code_hash()` in runtime and storage immediately and add a failing test for the old bypass.

- **P2 - The constructor docs still overstate what is guaranteed.** Lines `forge/crates/domain/src/run.rs:76-86` say "a record that exists is a record whose provenance is canonical" and that routing creation through `new()` makes the check "non-bypassable." That is only true for callers voluntarily using `new()`. The later paragraph is more accurate, but the opening claim is misleading while public fields/deserialization remain open.

## Verification

- `cargo test --locked -p forge-domain` passed.
- `cargo clippy --locked -p forge-domain --all-targets -- -D warnings` passed.
- `cargo build --locked -p forge-domain --target wasm32-unknown-unknown` passed.
- `RUSTDOCFLAGS='-D warnings' cargo doc --locked -p forge-domain --no-deps` passed.

## Handoff Check

- No new handoff files beyond already completed T004/T005.
