# Review 005 — forge-server spine

- **Slice goal:** Phase 1.3 first server replacement slice: create a Rust `forge-server` crate/binary with a real Forge-owned HTTP spine before any legacy `server/` deletion.
- **Reviewed:** working diff adding `forge/crates/server` and registering it in the workspace.
- **Files changed:** `forge/Cargo.toml`, `forge/Cargo.lock`, `forge/crates/server/Cargo.toml`, `forge/crates/server/src/lib.rs`, `forge/crates/server/src/main.rs`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `cd forge && cargo fmt --package forge-server` -> passed; `cd forge && cargo test -p forge-server --locked` -> passed; `cd forge && cargo clippy -p forge-server --all-targets --locked -- -D warnings` -> initially failed on `Arc<ForgeServer>` not being Send/Sync, then passed after changing `serve_blocking` to borrow `&ForgeServer`; `cd forge && cargo run -p forge-cli -- demo` -> passed with `REPLAY IDENTICAL: true`.

## Findings

No blocking findings after the fix.

- [P2] Clippy correctly rejected `Arc<ForgeServer>` because `WorkspaceCore` is not Send/Sync. Resolution: the first spine is single-threaded; `serve_blocking` now borrows `&ForgeServer` directly.
- [P3] This does not yet replace the legacy Zig `/control` tool surface or v0.4 `/bridge` request shape. Resolution: recorded as a follow-up; this slice creates the Forge-owned binary/build target and CoreCommand bridge only.

## Resolution status

- Added workspace crate `forge-server` with `GET /health`, `POST /bridge` accepting a serialized `CoreCommand`, and `POST /events/drain`.
- Added `forge-server` binary with `--bind`, `--workspace`, and `--workspace-id` arguments.
- Tests pin health, valid CoreCommand bridge response, and malformed bridge JSON behavior.

## Follow-ups

- Add legacy-compatible `/control/command` adapters for the active tools that still block `server/` deletion.
- Add v0.4 `/bridge` compatibility or repoint `runtime-web`/reference-host tests to the CoreCommand bridge.
- Extend server tests to real socket smoke once route compatibility starts replacing `tools/reference-host/test/server-*.test.js`.
