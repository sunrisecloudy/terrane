# Final Legacy Removal Forge Cutover - Independent Closure

## Scope

Final independent closure note for the Forge v1 cutover and legacy-removal run.
The user asked this session to work quickly, use subagents heavily, and commit
small reviewable slices so Claude can review commit-by-commit.

## Commit boundary

This closure covers the committed migration chain from `900ede56` through the
current head, including the follow-up persistence and static-check commits:

- `87503131 docs: record cutover closure blockers`
- `268bf82b native-apple: persist forge core workspaces`
- `5a41b8a2 native-linux: persist forge core workspace`
- `2409f2a3 native-android: persist forge core workspace`
- `3670e3ea native-windows: persist forge core workspace`
- `21fe8da6 tools: enforce durable native forge opens`

## Closed blockers

- Native hosts no longer open Forge through `forge_core_open_in_memory`.
  macOS, iOS, Linux, Android, and native Windows now use file-backed
  `forge_core_open(path, workspace_id)` with a host-private
  `forge-workspace.sqlite`.
- `tools/check-repo.mjs` now requires durable native `forge_core_open` and fails
  if those host bridges reintroduce `forge_core_open_in_memory`.
- Live legacy Zig reference grep found no live hits outside archived docs,
  review, task, and generated-output exclusions.

## Verification

- `cargo test --workspace --locked`: passed.
- `cargo clippy --workspace --all-targets --locked -- -D warnings`: passed.
- `cargo run -p forge-cli -- demo`: passed and printed `REPLAY IDENTICAL: true`.
- `node --no-warnings tools/check-repo.mjs`: passed.
- `node --test --no-warnings tools/reference-host/test/android-native-build.test.js`: 12 passed, 0 failed.
- `node --test --no-warnings tools/reference-host/test/windows-native-build.test.js`: 3 passed, 0 failed, 4 skipped on macOS.
- `node --test --no-warnings tools/reference-host/test/*.test.js`: 219 passed, 0 failed, 11 skipped.
- `rg` for native host `forge_core_open_in_memory` / `open_in_memory`: no native host hits.
- `git diff --check`: passed.

## Remaining external gates

- Remote CI/release evidence is not proven in this local session. Local tooling
  and packaging checks pass, but a real remote green `forge-ci.yml`, `ci.yml`,
  and `release.yml` run should be recorded separately.
- Linux and Windows native executable smoke tests remain platform-gated on this
  macOS machine. The full reference-host suite skipped those as expected.
- Forge server `/bridge` still accepts a caller-supplied `CoreCommand`; this is
  a latent P2 while the server is localhost/unwired, and a blocker before any
  network exposure. A future server-auth slice should introduce connection auth
  and derive trusted actor identity server-side.
- Existing unrelated dirty/untracked work remains intentionally untouched.
