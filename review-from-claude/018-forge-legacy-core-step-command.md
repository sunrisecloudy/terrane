# Independent Codex review: Forge legacy core.step compatibility command

- **Slice goal:** Add a temporary Forge CoreCommand replacement for the v0.4 generated-app `core.step` bridge so native hosts can migrate from `libzig_core` to `forge_core_handle_command` without also rewriting every legacy generated app in the same slice.
- **Review mode:** Independent Codex/self-review. Claude Code review is intentionally not requested because the user instructed Codex to work independently from Claude Code for this goal.
- **Files changed:** `forge/crates/core/src/commands/mod.rs`, `forge/crates/core/src/commands/legacy_core_step.rs`, `forge/crates/core/src/auth.rs`, `forge/spec/commands.md`.
- **Reason:** Host cutover cannot honestly relink `core.step` to Forge unless Forge exposes a real command that preserves the old generated-app-visible `{ ok, stateVersion, actions }` payload shape during migration.
- **Commands run:**
  - `cargo test -p forge-core legacy_core_step --locked` -> passed.
  - `cargo clippy -p forge-core --all-targets --locked -- -D warnings` -> passed.
  - Note: `cargo fmt --package forge-core` was run while developing, but it touched broad pre-existing formatting outside this slice; those unrelated format-only edits were restored before the final test and clippy run.

## Findings

- No blocker found. The command runs through the normal Forge `CoreCommand` registry and RBAC gate instead of bypassing the facade.
- `stateVersion` uses `Store::next_counter` under `__forge/meta`, so repeated calls are monotone and durable for file-backed workspaces.
- Invalid event payloads return the legacy inner payload error shape rather than a command-level `CoreError`, matching generated-app expectations.

## Resolution

- Registered `legacy.core_step` as a cutover-only command.
- Gated it to the same run-capable roles as `runtime.run`.
- Added tests for CreateTask parity, TransformText parity, durable counter progression, unknown-event fallback, and invalid-event payloads.

## Follow-ups

- Wire macOS and later each native host from `core.step` to this command through `forge-ffi`.
- Remove this command after legacy generated-app packages/runtime-web no longer expose the v0.4 `core.step` API.
