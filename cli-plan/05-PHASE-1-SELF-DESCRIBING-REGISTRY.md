# Phase 1 — Self-describing registry (the keystone)

**Theme:** attach machine-readable metadata to every command and make it the
single source of truth. This is ~80% of the value; the CLI, console, and agent
adapter are projections of what this phase produces.

**Risk:** low. Handler bodies do not change; we add data + read-only checks.
**Replay impact:** none (pure static data).

## Steps

### P1.1 — Choose the descriptor home

Two viable options; pick one in [13-OPEN-QUESTIONS.md](13-OPEN-QUESTIONS.md) Q1.

- **(A) In-Rust table** beside `COMMANDS` in `commands/mod.rs`: a parallel
  `const CATALOG: &[CommandDescriptor]` (or fold descriptor into the existing
  tuple so a command can't be registered without one). Compile-time guaranteed
  complete; no I/O.
- **(B) Checked-in `forge/data/commands.json`** loaded at startup, mirroring the
  `forge/data/*.json` extraction pattern from `forge-core-plan`. Easier for
  non-Rust tools to read directly; needs a load + validate step.

**Recommendation:** **(A) for the source of truth**, with the JSON emitted *from*
it (so Rust stays authoritative and tools still get JSON). This avoids a second
hand-maintained file drifting from the table.

### P1.2 — Define `CommandDescriptor`

Add a `forge-domain` (or `forge-core`) type matching
[04-COMMAND-CATALOG.md](04-COMMAND-CATALOG.md): `name`, `namespace` (derived),
`summary`, `surface`, `mutates`, `effectful`, `visibility`, `required_roles`,
`capabilities`, `payload_schema`, `response_schema`, `events`, `stability`,
`since`, `examples`. Serialize with serde. Keep it `wasm`-clean (no I/O in the
type).

### P1.3 — Fold the descriptor into the registry

Change `COMMANDS` from `(&str, Handler)` to `(CommandDescriptor, Handler)` (or a
parallel table keyed by name with a test that the key sets match). The
compiler/tests then guarantee **every command is described**.

- Dispatch still routes by `descriptor.name` — same semantics, same CR-A5
  reject for unknown names (`commands/mod.rs:224`).
- Keep ordering for readability; dispatch remains exact-match.

### P1.4 — Unify the role table with `authorize()`

Today `authorize()` (`auth.rs:28`) is a static match name → roles. Refactor so
the **same** mapping feeds both `authorize()` and `descriptor.required_roles` —
either:

- move the role list into the descriptor and have `authorize()` read it, or
- keep `authorize()` authoritative and *derive* the descriptor field from it,
  plus a test asserting they agree for every command.

Either way, **one source**, cross-checked. (Recommendation: descriptor holds the
role set; `authorize` consults the catalog — fewer places, one truth.)

### P1.5 — Author per-command schemas

For each of the **42** outer commands in `COMMANDS`, add
`schemas/commands/<name>.request.schema.json` and `.response.schema.json`. Reuse
existing object schemas under `schemas/` (23 files today) via `$ref` where shapes
already exist. **MVP `stable` backfill** (ship before console/agent): `runtime.run`,
`query.execute`, `applet.install`, `ui.dispatch_event`, `workspace.open`. The
remaining commands stay `preview` until schemas land; a missing schema blocks
`stable` but not registry completeness.

### P1.6 — Tests (the Phase-1 exit gate)

Add a `forge-core` (or `forge-testkit`) test module asserting the invariants from
[04](04-COMMAND-CATALOG.md):

1. `COMMANDS` keys == catalog names (no undescribed command, no orphan
   descriptor).
2. Each `name` resolves to a handler.
3. `required_roles` == `authorize()` for every command.
4. Each referenced schema file exists and parses.
5. No `public` command requires a privileged-only role.
6. Catalog serialization is deterministic (sort by name; stable JSON) — needed
   for `system.describe` replay-safety and for the contract hash.

## Deliverables

- `CommandDescriptor` type + the `CATALOG` table (or folded tuple).
- Unified role source shared with `authorize()`.
- `schemas/commands/*.schema.json` for at least the `stable` outer commands.
- A test module enforcing the six invariants.
- (Optional) a `forge/data/commands.json` emitted from the table for non-Rust
  consumers.

## Validation

```sh
cd forge
cargo test -p forge-core
cargo clippy -p forge-core -- -D warnings
# full gate once the role refactor touches the facade:
cargo test --workspace --locked
cargo run -p forge-cli -- demo            # spine still green
```

## Exit criteria

- Building `forge-core` fails if any command lacks a descriptor.
- A test fails if descriptor roles and `authorize()` disagree.
- The catalog serializes deterministically and every schema path resolves.
