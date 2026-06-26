# Review 124: core command handler extraction

Reviewed commit `1cc57726` (`forge-core: extract command handlers into commands/ modules`).

## Findings

- No blocking findings. `WorkspaceCore::handle` still runs the command-level `authorize` gate before matching the command name, and still dispatches the same command names to `self.cmd_*` handlers with the same unknown-command rejection path.
- The moved handlers are now grouped under `forge/crates/core/src/commands/` (`applet`, `runtime_run`, `replay`, `ui`, `schema`, `query`, `workspace_export`). Shared applet install/replay types remain reachable to sibling modules through the crate-private re-export in `workspace.rs`, and UI/session replay tests moved with the UI module.

## Verification

- `cargo test -p forge-core`
- `cargo clippy -p forge-core -- -D warnings`
- `cargo run -p forge-cli -- demo` (`REPLAY IDENTICAL: true`)
