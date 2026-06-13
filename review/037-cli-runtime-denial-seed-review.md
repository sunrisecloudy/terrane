# Commit Review: 8cfd94cd..64ca48a6

Reviewed commits:

- `8cfd94cd forge-cli: close review 032 (facade scenarios + exact denial code)`
- `64ca48a6 forge-runtime: close review 029/035 (exact denial-shape gate for legacy fallback) +1 test`

## Findings

1. **P2 - `runtime.run` seed override is now public command shape but the command spec still says the old contract.** `runtime.run` now accepts `random_seed?`/`time_start?` (`forge/crates/core/src/workspace.rs:263`) and returns `host_call_methods` (`forge/crates/core/src/workspace.rs:385`), while `forge/spec/commands.md:26` still documents only `applet_id, entrypoint, input, limits?` and `run_id, result, ui patch, logs`. Because the CLI scenario test is now deliberately proving the real command/event facade, this should be pinned in the command spec (or moved behind a test/conformance-only command) before shells and conformance runners depend on an undocumented API shape.

2. **P2 - `time_start` accepts `u64` values that the runtime clock cannot represent.** `seed_field` accepts any JSON `u64` for `time_start` (`forge/crates/core/src/workspace.rs:654`), but `LogicalClock::new` casts that `u64` to `i64` (`forge/crates/runtime/src/recorder.rs:74`). A payload with `time_start > i64::MAX` is accepted and recorded as the original `u64`, while `ctx.time.now()` starts from the wrapped negative `i64`; the new tests cover half-specified/non-integer overrides, but not this accepted out-of-range case. Either reject `time_start > i64::MAX` in `runtime.run` or change the clock/recorded response type so the seam can represent the full accepted range.

3. **P2 - The review 035 denial marker fix still collides with CoreError-shaped user JSON.** `is_recorded_denial` now treats any response exactly shaped as `{"denied": <CoreError>}` as a recorded denial (`forge/crates/runtime/src/runner.rs:273`, `forge/crates/runtime/src/runner.rs:287`). That avoids `{"denied": false}`, but `storage.get`/`db.get`/`db.list` can replay arbitrary app data, so a legitimate snapshotless legacy run that read `{"denied":{"kind":"RuntimeError","detail":"..."}}` and then failed for an app reason still takes the all-deny path and can replay as a spurious permission failure. Use an unambiguous denial marker outside the user-response domain, or add a regression proving CoreError-shaped user data is not misclassified.

## Verification

- `git show --check 8cfd94cd` passed.
- `git show --check 64ca48a6` passed.
- `cargo test --locked -p forge-cli --test scenarios` passed on the current workspace.
- `cargo test --locked -p forge-runtime --test determinism` passed on the current workspace.
- `cargo test --locked -p forge-core` passed on the current workspace.
- `cargo clippy --locked -p forge-runtime --all-targets -- -D warnings` passed on the current workspace.
- `cargo clippy --locked -p forge-cli --all-targets -- -D warnings` passed on the current workspace.
- `cargo clippy --locked -p forge-core --all-targets -- -D warnings` passed on the current workspace.

Note: the workspace has unrelated dirty edits (including apparent follow-ups to review 036), so verification is a smoke check of the current tree; the findings above are grounded in the committed diffs and current line references.
