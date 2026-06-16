# Review 119: UI dispatch files follow-up

Reviewed commit `c0f82019` (`forge-core: wire ctx.files into ui.dispatch_event bridge like runtime.run`).

## Findings

- No blocking findings. This appears to close review 112: `cmd_ui_dispatch_event` now builds the injected filesystem sandbox and passes it into `StorageHostBridge::with_file_system`, matching the `runtime.run` bridge path for handlers that call `ctx.files`.
- The new regression in `forge/crates/core/tests/ui_dispatch_event.rs` covers the important end-to-end behavior: a dispatched handler writes and reads through `ctx.files`, records both host calls, and replays identically.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
