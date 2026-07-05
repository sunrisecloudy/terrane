# Capability: `applescript` — macOS automation as a recorded effect

Crate `rust/crates/terrane-cap-applescript/`, namespace `applescript`. **This
capability is already planned in depth — and largely implemented** — on branch
`feat/mac-control-applescript-dual-mlx`: see `plan-mac-control-app.md` (on
that branch; authored in worktree competent-bardeen-2a9d81) for the full
program, which pairs the cap with the `apps/mac-control` app (NL →
Osaurus AppleScript model via `local-model` → check → confirm → run). The
program is currently blocked on **model inference only** (JANG 6-bit ZAYA1
MoE unsupported by the pinned Python mlx-lm fork — `blocker.md` on the
branch); the capability itself is model-agnostic and complete. This doc is
the capability-shaped distillation; where they differ, the branch plan wins.

## Locked decisions (inherited from `plan-mac-control-app.md`)

- **Execution is an edge effect; replay never re-executes.**
  `applescript.run`/`applescript.check` return
  `Decision::Effect(Effect::AppleScriptRun/Check)`; the edge pipes the script
  to `/usr/bin/osascript` (run) or `osacompile` (check) **via stdin**, and
  records the outcome. Replay folds recorded facts — no osascript ever spawns
  during replay.
- **The full script is recorded**, not a hash. Auditability is the point: an
  event that says "some script ran, hash abc123" is useless in a permission
  post-mortem. Bounded by the 64 KiB script cap; `describe()` truncates to
  ~50 chars, so log listings stay readable while the log stays complete.
- **macOS only, typed.** On non-mac hosts (or missing binary) the edge returns
  `Error::Runtime("applescript requires macOS (osascript not found)")` — no
  panic, tests green everywhere. A `native.supports`-style probe for
  discoverability is a Decision below.

## Surface (as built on the branch)

| Surface | Name | Args / semantics |
| --- | --- | --- |
| Command | `applescript.run` | `app, script` → `Effect::AppleScriptRun`; validation pure: app exists, script non-empty, ≤ 64 KiB |
| Command | `applescript.check` | `app, script` → `Effect::AppleScriptCheck` (osacompile syntax check, runs nothing) |
| Resource (call) | `run(script)` / `check(script)` | same, for app JS; `resource_call_output` returns `{ok, output, error, exitCode, durationMs}` / `{ok, error}` |
| Resource (read) | `runs()` | this app's folded run history (read-only) |

### Events

| Kind | Payload (borsh) | Fold |
| --- | --- | --- |
| `applescript.ran` | `{ app, script, ok, output, error, exit_code, duration_ms }` — `exit_code = -1` on timeout kill | push to `app → Vec<RunRecord>`, deterministic truncation at `MAX_RUNS_PER_APP = 100` |
| `applescript.checked` | `{ app, script, ok, error }` | audit-only, no state |
| (reacts) `app.removed` | — | drop the app's history |

Edge behavior: 30 s wall-clock timeout (env `TERRANE_APPLESCRIPT_TIMEOUT_MS`)
that kills the child and still records a failed event — a timeout is a fact,
never a lost run.

## Security & permissions — this is the dangerous one

AppleScript is arbitrary machine control (files, mail, keystrokes via System
Events). The layering:

1. **Terrane grant, per-app, default-deny.** Grant resource `applescript`
   (verbs `call` + `read`) whose description says plainly: *full automation of
   this Mac*. No blanket/user-wide grant shorthand.
2. **Approval shows the ask.** The in-session elicitation flow (auth-plan/15)
   carries the pending command's args — the shell prompt for an ungranted
   `applescript.run` must render a **script preview** (first N lines +
   size), so the user approves a *capability having seen a concrete script*,
   not an abstraction. Post-grant runs don't re-prompt per script — that's
   the app-level confirm-before-run UX (`mac-control`'s plan→Run flow), which
   stays the recommended app pattern, not a core gate.
3. **macOS TCC stacks on top.** First automation of each target app fires the
   OS consent prompt at the process hosting terrane; System Events may need
   Accessibility. Documented in `doc.rs`/`APP_API.md`; a TCC denial surfaces
   as a failed run event with the osascript error — recorded, auditable.
4. **The log is the audit trail.** Full script + output + exit in every
   `applescript.ran`; `terrane log` after an incident shows exactly what ran.

## Replay story

Fold `applescript.ran` into bounded history; `applescript.checked` is
audit-only. Replaying a log on a Linux CI box is fine — no osascript, no TCC,
identical state. The paired mac-control app records only ordinary `kv.*` and
`local-model.responded` events around it (Option A), so the full NL→run loop
replays with zero re-inference and zero re-execution.

## Implementation plan

Most of this exists on `feat/mac-control-applescript-dual-mlx`; the plan is
convergence, not construction:

1. **Rebase/extract:** bring `terrane-cap-applescript` + interface `Effect`
   variants + `terrane-host/src/applescript.rs` + registry wiring from the
   branch onto current `main` (the cap does not depend on the blocked model
   work — split it from the dual-MLX commits).
2. **Contract check:** confirm crate matches this doc (events, caps, timeout,
   typed non-mac error, `MAX_RUNS_PER_APP` truncation determinism).
3. **Script preview in elicitation:** extend the shell permission prompt to
   render pending `applescript.*` args as a code preview (web + mac shells).
4. **Docs:** `APP_API.md` resource table with the explicit danger note + grant
   command; CLI help lines.
5. **Tests:** engine `terrane-core/tests/cap/applescript.rs` (decide→Effect,
   pure-core refusal, stub-runner dispatch + replay identity, truncation,
   `app.removed`); e2e `terrane-host/tests/cap/applescript.rs` — validation
   default-run everywhere; `#[ignore = "runs real osascript"]` for
   `run calc "return 2 + 2"` → `4`, failing-script records `ok=false`, then
   `terrane replay` identical (pure-compute scripts need no TCC).

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`

## Non-goals

**Sandboxed script vetting** — statically judging what an AppleScript will do
is impossible; the security model is grant + preview + TCC + audit log, stated
honestly, never a filter that pretends otherwise. Windows/Linux automation
(different capability if ever, not this namespace stretched). Script
scheduling (a future scheduler cap composes with this one). JXA: osascript
takes `-l JavaScript`, but v1 records and runs AppleScript source only — a
`lang` field is an additive event change if wanted (Decision below).

## Decisions to confirm

- **Support probe** — *recommendation:* add an `applescript.supports` query
  answering from the host platform (mirrors `native.supports`) so apps can
  hide the surface off-mac; *alternative:* let callers hit the typed runtime
  error (works today, worse UX).
- **JXA in v1** — *recommendation:* no — AppleScript only, matching the
  Osaurus model output and the branch implementation; *alternative:* a
  `lang: "applescript" | "jxa"` arg + event field now to avoid a later
  format bump.
- **Per-script re-approval mode** — *recommendation:* keep approval at grant
  time + app-level confirm UX (branch plan's stance); *alternative:* an
  auth-level "prompt every run" grant flag for the extra-cautious — heavier
  elicitation traffic, real safety win; fits auth-plan/15 if wanted.
