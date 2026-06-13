# Commit Review: 859e3309

Reviewed commit: `859e3309 forge-core: wire the M0a spine facade (CR-A1..A5, P-04, CR-8/CR-9, UI-1)`

## Findings

1. **P1 - Add command-level RBAC before dispatch.** `WorkspaceCore::handle()` routes every command directly to its handler (`forge/crates/core/src/workspace.rs:127`) and the mutating/read handlers do not check `cmd.actor.role` (`applet.install` at `forge/crates/core/src/workspace.rs:173`, `query.execute` at `forge/crates/core/src/workspace.rs:392`, `runtime.replay` at `forge/crates/core/src/workspace.rs:346`). That means a Viewer/Auditor can install applets, a caller without `db.read` can list records, and a Runner can replay runs, even though `forge/spec/commands.md:7`, `:11`, `:21`, `:26`, and `:28` define stricter roles/caps and PRD CR-A3 says every command carries actor context and passes policy before touching state. Please add a per-command authorization gate before handler dispatch, then add negative tests for Viewer install/query, Runner replay, and non-run roles as appropriate.

2. **P1 - Repeated `runtime.run` calls overwrite the previous run record.** The core always calls `record_run()` with `DEFAULT_RANDOM_SEED` and `DEFAULT_TIME_START` (`forge/crates/core/src/workspace.rs:289`), while runtime derives `run_id` only from program hash plus those two seeds (`forge/crates/runtime/src/runner.rs:216`, `:240`). Running the same applet twice therefore produces the same `run_id`, and `Store::save_run()` overwrites on `ON CONFLICT(run_id)` (`forge/crates/storage/src/lib.rs:515`). This violates CR-9's "every execution persists" audit/replay requirement because the second execution replaces the first, including different inputs, outputs, logs, and writes. Please mint a unique per-execution run identity in the facade or include an invocation nonce while keeping replay seeds deterministic, and add a test that two runs of the same applet with different inputs both remain loadable.

3. **P2 - Replay is tied to the currently installed applet version.** `applet.install` bumps `version` but persists only the latest applet under `applet/<id>` (`forge/crates/core/src/workspace.rs:214`, `:417`), and `runtime.replay` reconstructs the program from the current installed `js_code`/manifest (`forge/crates/core/src/workspace.rs:359`). After reinstalling/upgrading an applet, old runs can no longer replay against the code hash they recorded; they will either diverge or fail before the saved host responses can be used. Please retain replay artifacts by `code_hash` or applet version, or persist enough source/compiled program in/next to `RunRecord`, then add a regression test: install v1, run, reinstall v2, replay the v1 run.

## Verification

- `git show --check 859e3309`
- `cargo test --locked -p forge-core`
- `cargo clippy --locked -p forge-core --all-targets -- -D warnings`
