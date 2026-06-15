# Review 004 — sync core-command CRDT transport

- **Slice goal:** Phase 1.2 gap fill: fold CRDT exchange into the existing `forge_core_handle_command` ABI so hosts do not need a parallel `forge_crdt_*` C surface before native cutover.
- **Reviewed:** working diff touching `forge-sync`, `forge-core`, `forge-ffi` tests, and `forge/spec/commands.md`.
- **Files changed:** `forge/Cargo.lock`, `forge/crates/sync/Cargo.toml`, `forge/crates/sync/src/lib.rs`, `forge/crates/core/src/auth.rs`, `forge/crates/core/src/commands/mod.rs`, `forge/crates/core/src/commands/sync.rs`, `forge/crates/core/src/workspace.rs`, `forge/crates/core/tests/sync.rs`, `forge/crates/ffi/tests/ffi.rs`, `forge/spec/commands.md`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review was waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `cd forge && cargo test -p forge-sync --locked` -> passed; `cd forge && cargo test -p forge-core --test sync --locked` -> passed; `cd forge && cargo test -p forge-ffi --locked` -> passed, 12 FFI tests; `cd forge && cargo test -p forge-core --locked` -> passed; `cd forge && cargo clippy -p forge-sync -p forge-core -p forge-ffi --all-targets --locked -- -D warnings` -> passed; `cd forge && cargo run -p forge-cli -- demo` -> passed and printed `REPLAY IDENTICAL: true`.

## Findings

No blocking findings.

- [P3] `cargo fmt --package forge-core` initially touched broad pre-existing formatting outside the slice. Resolution: restored the formatting-only churn outside the intended paths before review/staging.
- [P3] `sync.import` must remain fail-closed for unknown peers. Resolution: added explicit `sync.trust_peer` command; the core test imports before trust and proves the chunk is denied without materializing records, then trusts the peer and imports successfully.
- [P3] The host ABI proof must not depend on Rust-only `WorkspaceCore::sync_with`. Resolution: added FFI test with two C handles moving applet-created CRDT state through `sync.export` and `sync.import` only.

## Resolution status

- CRDT replacement decision: use `forge_core_handle_command` with `sync.trust_peer`, `sync.export`, and `sync.import`; no `forge_crdt_*` symbols in this slice.
- Import safety: packet chunks carry the same SS-7 authorization envelope used by in-process sync; receiver-side import authorizes before atomic storage apply and persists allow/deny audit rows through the existing path.
- Host-cutover proof: `forge-ffi` test confirms a target handle can query the imported record after sync without any CRDT-specific C entrypoint.

## Follow-ups

- Native host cutovers still need to retire existing `CZigCrdtBridge` / `ZigCrdtBridge` consumers and call these core commands through their host bridge.
- The Forge server replacement should build on the same packet contract for network `/bridge`/sync transport rather than reintroducing raw CRDT ABI calls.
