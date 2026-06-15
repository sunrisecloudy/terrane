# Review 002 — ffi abi conformance (working diff)

- **Slice goal:** Phase 1.1 gap fill: make `forge-ffi` more usable as the C ABI replacement before any host cutover or legacy deletion. This slice does not port hosts and does not delete legacy code.
- **Reviewed:** working diff touching `forge/crates/ffi`.
- **Files changed:** `forge/crates/ffi/Cargo.toml`, `forge/crates/ffi/include/forge_ffi.h`, `forge/crates/ffi/tests/ffi.rs`.
- **Commands run:** `cd forge && cargo fmt --package forge-ffi` -> passed; `cd forge && cargo test -p forge-ffi --locked` -> passed, 11 FFI tests; `cd forge && cargo clippy -p forge-ffi --all-targets --locked -- -D warnings` -> passed; `cd forge && cargo build -p forge-ffi --locked` -> passed; `find forge/target/debug -maxdepth 1 -name 'libforge_ffi.*' -print | sort` -> showed `libforge_ffi.a`, `libforge_ffi.dylib`, `libforge_ffi.rlib`; `cd forge && cargo run -p forge-cli -- demo` -> passed and printed `REPLAY IDENTICAL: true`.

## Findings

No blocking findings.

- [P3] Header-test coverage was initially too weak because it checked only symbol substrings. Resolution: `checked_in_c_header_declares_the_exported_abi` now checks full prototypes including pointer types, argument order, and `const` placement.
- [P3] FFI edge coverage missed several cheap host-boundary cases. Resolution: added tests for file-backed `forge_core_open`, null `command_json`, `forge_string_free(NULL)`, and `forge_core_last_error` clearing after a later successful open.

Independent read-only review (Poincare) confirmed:

- `staticlib` is declared in `forge/crates/ffi/Cargo.toml`.
- The install/run/drain test exercises the real `WorkspaceCore` path through exported FFI calls, `WorkspaceCore::handle`, `applet.install`, `runtime.run`, and event draining.
- No P1/P2 findings.

## Resolution status

- P3 header prototype check -> fixed in this slice.
- P3 FFI edge coverage -> fixed in this slice.

## Follow-ups

- Host cutover remains blocked on actual native bridge rewrites and packaging/CI changes.
- CRDT remains intentionally not exported as `forge_crdt_*` in this slice; the next migration decision is whether macOS direct CRDT bridge is retired into `forge_core_handle_command` or a dedicated CRDT ABI is added.
- `server/` remains blocked until a Forge server replacement covers active `/bridge`, `/control`, and sync consumers.
