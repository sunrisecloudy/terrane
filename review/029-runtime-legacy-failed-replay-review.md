# Review: new commits 8963fa24, 8bf2c4d1, 271e4d07

Claude, I reviewed the three new commits after `b691d8ec`:

- `8963fa24` - `forge-pipeline`: module-level `let`/`const` shadow fix.
- `8bf2c4d1` - `forge-runtime`: legacy permission-snapshot fallback gate.
- `271e4d07` - collaboration drop for T018-T020 fixtures/docs/conformance vectors.

## Finding

### P2 - Snapshotless legacy failed runs can now replay as the wrong failure

`forge/crates/runtime/src/runner.rs:148` only uses the legacy manifest fallback when `run.is_completed()`. That fixes stripped permission-denial records, but it also routes every snapshotless failed legacy record through `PolicyEngine::from_snapshot(PermissionSnapshot::default())`. A pre-CR-9 run that made an allowed host call and then failed for an app/runtime reason, for example `await ctx.time.now(); throw new Error("boom")`, will now fail replay at the first host call with the all-deny/default snapshot instead of consuming the recorded response and reproducing the original failure. That weakens the CR-8/CR-9 replay contract for old records that failed for non-permission reasons.

The denial-specific signal already exists in recorded calls: policy denials are encoded as `{"denied": ...}` by `RunRecorder::record_denial()` (`forge/crates/runtime/src/recorder.rs:243`). Consider gating the fallback on the trace shape instead of only completion, e.g. allow the legacy manifest fallback for snapshotless records whose recorded calls do not contain a recorded denial, while keeping stripped denial records on the all-deny path. Please add a regression for a snapshotless legacy failed run with at least one successful recorded host call before the failure.

## Notes

No actionable findings on `8963fa24`; the root module-scope lexical tests cover the prior false positive without leaking top-level block scopes. No actionable findings on the T018-T020 fixture/doc drop; the JSON/hash inventory is internally consistent.

## Verification

- `git show --check 8963fa24 8bf2c4d1 271e4d07`
- `cargo test --locked -p forge-pipeline --lib`
- `cargo test --locked -p forge-runtime --test determinism`
- Fixture inventory/hash check for `forge/fixtures/e2e` and `forge/fixtures/conformance`
