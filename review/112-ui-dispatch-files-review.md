# Review 112: UI event-dispatch loop

Reviewed new commits after `01ddf945` through `0a0d27cb` (UI-4/CR-6 dispatch, replay-session, and handler-registry batch).

## Finding

1. **P2 - `ui.dispatch_event` drops the injected `ctx.files` sandbox.** The normal `runtime.run` path builds the bridge with the host file-system factory (`forge/crates/core/src/workspace.rs:883`) and chains `.with_file_system(file_system)` (`forge/crates/core/src/workspace.rs:899`). The new dispatch path only creates the HTTP client and secret store (`forge/crates/core/src/workspace.rs:1376`) and builds `StorageHostBridge` with `.with_secret_store(secret_store)` (`forge/crates/core/src/workspace.rs:1384`-`1389`), so a UI handler that calls `ctx.files.read`/`ctx.files.write` will fail closed even when the applet manifest grants files access and `runtime.run` works with the same installed applet. That contradicts the "same engine/host path as a run" promise for UI-4 and breaks legitimate interactive applets whose event handlers use file-backed state. Please mirror the `runtime.run` bridge setup in `cmd_ui_dispatch_event` by constructing `let file_system = (self.file_system_factory)();` and chaining `.with_file_system(file_system)`, then add a dispatch regression using `InMemoryFileSystem` where a handler reads or writes through `ctx.files`.

## Verification

- `cargo test -p forge-core --test ui_dispatch_event`
- `cargo test -p forge-runtime --test ui_dispatch`
