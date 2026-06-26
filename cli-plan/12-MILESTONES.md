# 12 — Milestones, Sequencing & Effort

## Dependency order

```
Phase 1  Self-describing registry  ──┬─▶ Phase 2  system.describe ──┬─▶ Phase 3  CLI
(keystone)                           │                              ├─▶ Phase 4  Web console
                                     │                              └─▶ Phase 5  Agent adapter
                                     └─▶ (Phase 11) contract wiring (can land alongside 2–3)
```

Phase 1 unblocks everything. Phases 3/4/5 are **independent of each other** once
Phase 2 lands — they can proceed in parallel or in priority order.

## Milestones

### M1 — Catalog exists (Phase 1)

- `CommandDescriptor` + folded registry; unified role table; schemas for `stable`
  commands; invariant tests.
- **Exit:** build fails on an undescribed command; role/`authorize` cross-check
  passes; catalog serializes deterministically.
- **Effort:** the bulk of the work — registry fold + role-table unification is
  ~1–2 days; **per-command schema authoring for 42 outer commands** is the long
  pole (backfill `stable` for MVP commands first; rest stay `preview`).

### M2 — Discoverable surface (Phase 2)

- `system.describe` with role/tier filtering + stable `catalogVersion`.
- `system.trace` — read-only `RunRecord` / `RecordedCall` query (see doc `14`).
- **Exit:** any front-end gets the role-scoped catalog via one command; hash
  stable; `system.trace` returns redacted run effects without mutation.
- **Effort:** ~1–2 days (`system.trace` adds a handler over existing journal data).

### M3 — Generic CLI (Phase 3)

- `forge commands | describe | run` (local + `--server`), `--dry-run`,
  catalog-generated help, e2e tests.
- **Exit:** every outer command runnable via `forge run`; `demo` still green.
- **Effort:** ~1–2 days.

### M4 — Contract gate (Phase 11 wiring)

- Export emits the catalog; verify gains a drift gate; Premium pin refreshed.
- **Exit:** changing a command without refreshing the contract fails CI.
- **Effort:** ~1 day. *Can land with M2/M3.*

### M5 — Web console (Phase 4)

- Static console over `/bridge`; schema-driven forms + raw-JSON fallback; tier
  filtering; smoke test.
- **Exit:** any visible command runs from a generated form; no hard-coded
  commands in the UI.
- **Effort:** ~1 week (it is a real frontend).

### M6 — Agent adapter (Phase 5)

- Catalog→tools projector + executor + tier scoping + reference agent.
- **Exit:** offered tools == catalog filtered by the agent's tier/role.
- **Effort:** ~2–3 days, much of it falling out of M1–M2.

## Suggested delivery order

1. **M1 → M2 → M3** — this alone satisfies the original ask: a self-describing CLI
   that runs every action and that an agent can learn. **Ship this as the MVP.**
2. **M4** alongside M2/M3 to lock drift.
3. **M5** for the web console.
4. **M6** for the agent surface (cheap after M1–M2; can precede M5 if agents are
   the priority).

## Validation gates per milestone

Focused, while iterating:

```sh
cd forge
cargo test -p forge-core      # M1, M2
cargo test -p forge-cli       # M3
cargo clippy -p forge-<crate> -- -D warnings
```

Broaden before merging shared/contract changes:

```sh
cd forge
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p forge-cli -- demo

# contract (M4):
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
```

## Risk register

| Risk | Likelihood | Mitigation |
| --- | --- | --- |
| Role refactor (P1.4) regresses RBAC | med | one shared table + cross-check test; full `--workspace` gate before merge |
| Schema authoring is large | med | `preview` stability + raw-JSON fallback; backfill highest-traffic first |
| Accidental admin exposure | low | `public`-vs-privileged build test; tier filtering server-side |
| Console scope creep | med | it is a *renderer*; no command logic allowed in the UI |
| Contract export already drifts (F11) | **confirmed** | M4 catalog-as-export closes it on day one |
| Contract churn breaks Premium pin | low | intentional pin refresh per `CLAUDE.md`; drift gate makes it explicit |
| `control.*`/legacy surface confusion | low | `debug` tier excluded from public surface by default |

## Definition of done (initiative)

- One catalog is the single source of truth for command metadata; build-enforced
  complete.
- `system.describe`, `forge run`, the console, and the agent adapter all read the
  catalog and hard-code nothing.
- RBAC, audit, determinism, and the public/Premium boundary are unchanged.
- The public contract enumerates the catalog with a drift gate.
- `forge demo` (the M0a spine) stays green throughout.

## Commit hygiene (per repo norms)

Work on `plan/unified-cli` (this worktree); branch feature work off `main`; stage
your own files explicitly (never `git add -A`); keep commits frequent, granular,
and green. The plan folder lands as docs first; implementation phases land as
separate, individually-green commits.
