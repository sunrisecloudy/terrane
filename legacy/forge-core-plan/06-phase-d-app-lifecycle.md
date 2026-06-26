# Phase D — App-lifecycle authority (step D12)

**Theme:** close the single biggest *design* gap. Today the shells own app version state and mutate
`app_versions.status` with **raw SQL** — rollback, quarantine, activation — bypassing the core's
audit, atomicity, and replay. **Decision locked in:** the **Forge core should own** this. After
Phase D, shell raw SQL on `app_versions.status` is illegal; all transitions go through core commands
that emit SC-12 audit rows and are covered by conformance vectors.

**Prerequisites:** A3 (status/trust enums), A5 (one schema), C11 (recording unified). This is the
most behaviorally significant step — do it after the core already owns recording and the schema is
consolidated.

---

## D12 — New `applet.*` / `quota.*` commands; migrate shells off raw SQL

**New core commands** (behind the JSON seam, persisted in `forge-storage` via the trusted-seam
pattern used by `grant_db_read`):

| Command | Replaces (shell logic) | Notes |
|---|---|---|
| `applet.list_versions` | `PlatformAppRegistry.activeVersion` + version queries | returns history + active pointer |
| `applet.activate_version` | raw `UPDATE apps SET active_install_id=…` | atomic; emits audit |
| `applet.rollback_version` | `PlatformAppRegistry.rollback:129-173` (dataVersion check, find prior non-quarantined, swap, events) | one transaction; emits audit |
| `applet.set_status` | raw `UPDATE app_versions.status` in rollback + quarantine | the only legal status path |
| `quota.auto_quarantine` (+ `quota.status` fields `budget_error_count_60s`, `quarantine_eligible`) | `BridgeBudgetQuarantine.maybeQuarantineAfterBudgetError:9-32` + `quarantineWebapp:125-207` | budget-driven, auditable |

**Persistence:** move app version history + the active-version pointer into the core's `Store`
(unify with the `apps/app_versions/app_installations` tables now owned by the migrations from A5).
The core becomes the authority; the shell DB is no longer independently mutated.

**Moves (decision):**
- App version install / rollback / activation — `PlatformAppRegistry.swift:35-208` → core commands.
- Auto-quarantine (3+ `resource_budget_exceeded` in 60s → quarantine + restore prior version) —
  `BridgeBudgetQuarantine.swift` → `quota.auto_quarantine`. (Currently macOS-only; becomes shared.)

**Stays per-platform:** OS-level install/uninstall **filesystem** operations (copying package files
into place, removing them). The *decision* to activate/rollback/quarantine is the core's; the
*file movement* is the shell's.

**Migration order:** macOS `PlatformAppRegistry` + `BridgeBudgetQuarantine` first (call the new
commands instead of raw SQL; assert outcomes match prior behavior), then fan out one shell per
commit.

**Validation:** `cargo test -p forge-core` (lifecycle + auto-quarantine conformance vectors);
`cargo run -p forge-cli -- demo`; export/verify public contract; audit-log assertion that **every**
status transition emits an SC-12 row; per-shell test that rollback/quarantine outcomes match prior
behavior; replay-identical gate.

**Risk:** high. **App-visible:** yes. **Effort:** XL.

---

## Open decisions that gate D12

These are in [09-decisions-and-open-questions.md](09-decisions-and-open-questions.md); confirm before
implementing:

- **Auto-quarantine policy knobs:** is the `3 errors / 60s` threshold core-owned policy, and is it
  configurable data (`quota` config) vs a fixed constant in core?
- **Public-contract surface:** which of `applet.list_versions/activate/rollback/set_status` and
  `quota.auto_quarantine` must appear in `artifacts/public-contract.json` for Premium vs stay
  internal? This decides whether the Premium pin must be refreshed.

---

## Phase D exit criteria

- The core owns app version history, the active-version pointer, and all status transitions.
- No shell mutates `app_versions.status` with raw SQL — every transition is a core command emitting
  an SC-12 audit row.
- Auto-quarantine is a shared, auditable, core-owned policy (no longer macOS-only).
- Conformance vectors + replay-identical gate green; contract exported; Premium pin handled per the
  surface decision.
