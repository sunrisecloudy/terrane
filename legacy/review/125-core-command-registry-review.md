# Review 125: core command registry

Reviewed commit `9d1915ec` (`forge-core: command registry + facade handle()`).

## Findings

- No blocking findings. `WorkspaceCore::handle` still runs `authorize(&cmd)` before dispatch, and the new `commands::Registry` table preserves the same command names/handlers plus the same CR-A5 unknown-command error.
- The registry is static routing data only; command lifecycle and capability gates remain in `WorkspaceCore` handlers and host/runtime paths.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
