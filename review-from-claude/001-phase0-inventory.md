# Review 001 — phase0 inventory (working diff)

- **Slice goal:** Phase 0.1-0.4 of `task-jun-15/03-LEGACY-REMOVAL-MIGRATION.md`: create the recovery ref, capture tracked legacy inventory/LOC, map legacy references, and verify the Forge baseline. No deletion and no host/tooling cutover.
- **Reviewed:** working diff adding `task-jun-15/05-PHASE0-INVENTORY.md`.
- **Files changed:** `task-jun-15/05-PHASE0-INVENTORY.md`.
- **Commands run:** `git rev-parse legacy-archive/pre-removal` -> `498213987611bea97e76518e2a507347a8e0835a`; `cd forge && cargo test --workspace --locked` -> passed; `cd forge && cargo clippy --workspace --all-targets --locked -- -D warnings` -> passed; `cd forge && cargo run -p forge-cli -- demo` -> passed and printed `REPLAY IDENTICAL: true`.

## Claude Review Attempt and User Override

Claude Code CLI is installed at `/Users/vehasuwat/.local/bin/claude`, but the required Opus review could not be completed because the CLI is not authenticated.

Attempted command:

```text
claude -p --model opus "<phase 0 review prompt and artifact content>"
```

Result:

```text
Not logged in · Please run /login
```

Because the prompt requires Claude Code Opus 4.8 review before committing each slice, this file records the blocker honestly instead of fabricating a Claude review.

Second attempt during the next goal continuation:

```text
claude -p --model opus "Authentication smoke check: reply with exactly CLAUDE_AUTH_OK and do not use tools."
```

Result:

```text
Not logged in · Please run /login
```

User override:

```text
2026-06-15: "you have to work indepently from  claude  code update goal to do it  this way"
```

Resolution: the migration continues without Claude Code as an external reviewer. Slice reviews remain written under `review-from-claude/` for continuity with the implementation prompt, but they are now independent Codex review/verification artifacts unless a later user direction reinstates Claude Code review.

Third attempt during the next goal continuation:

```text
claude -p --model opus "Authentication smoke check: reply with exactly CLAUDE_AUTH_OK and do not use tools."
```

Result:

```text
Not logged in · Please run /login
```

## Independent Read-Only Verification

Three read-only explorers completed parallel checks:

- Phase 0 inventory/reference scan: confirmed 13 tracked legacy files; `src/`-only LOC 16,578; source plus public C headers 16,660; exact-term hits in 78 files total, split into live code/tooling/CI and docs/task/history.
- Forge FFI/consumer scan: confirmed `forge-ffi` exposes only `forge_core_open`, `forge_core_open_in_memory`, `forge_core_handle_command`, `forge_core_drain_events`, `forge_core_last_error`, `forge_core_close`, and `forge_string_free`; no `forge_crdt_*` ABI; all five legacy native host paths still consume Zig, while top-level `windows/` already imports `forge_ffi`.
- Tooling/CI/server scan: confirmed `package-release`, `check-repo`, CI/release workflows, reference-host tests, `runtime-web`, and native control tests still depend on Zig server/core assumptions; `server/` must not be deleted before a Forge server replacement exists for active `/bridge`, `/control`, and notebook sync consumers.

## Findings

- [P3] Claude review loop was unavailable locally. The original implementation prompt required Claude Code Opus 4.8 review before committing each slice, but `claude -p --model opus` failed with `Not logged in · Please run /login` on three attempts. The user explicitly changed the working rule on 2026-06-15 to proceed independently from Claude Code. Process gate: `task-jun-15/04-IMPLEMENTATION-PROMPT.md` Claude Review Loop, superseded for this run by user direction.

No content blockers were found by local verification or the independent read-only explorers. The Phase 0 artifact does not delete legacy code, does not edit remote-team-owned paths, records the recovery branch, records the live reference gates, and records green Forge baseline commands.

## Resolution status

- Claude review loop authentication blocker -> resolved by explicit user direction to proceed independently from Claude Code.

## Follow-ups

- Continue writing per-slice review/verification artifacts, but do not block on Claude Code unless the user reinstates that requirement.
- Keep `zig-core/`, `zig-crdt/`, and `server/` undeleted until the Phase 1 and Phase 2 gates in `task-jun-15/03-LEGACY-REMOVAL-MIGRATION.md` are satisfied and zero-reference grep proves no live consumers remain.
- Prepared the next likely Phase 1 slice by inspection only: `forge-ffi` should first add a checked-in C header, build `staticlib` alongside `cdylib`/`rlib`, and extend `forge/crates/ffi/tests/ffi.rs` to drive a real `applet.install` + `runtime.run` + `forge_core_drain_events` path, mirroring the existing top-level Windows Forge consumer. Current checks: `cargo test -p forge-ffi --locked` passed and `cargo build -p forge-ffi --locked` passed.
