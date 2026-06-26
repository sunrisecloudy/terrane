# 11 — Schemas & Public Contract

The catalog is only trustworthy if it cannot silently drift from the code. This
file defines the **source-of-truth rules** and how the catalog plugs into the
existing public-contract export/verify gate.

## Source-of-truth rules

| Fact | Single home | Consumers read from |
| --- | --- | --- |
| Command exists + handler | `COMMANDS` table (`commands/mod.rs:68`) | dispatch, catalog |
| Required roles | the unified role table (P1.4) | `authorize()` **and** catalog |
| Payload/response shape | `schemas/commands/<name>.*.schema.json` (reusing `schemas/*.json` via `$ref`) | CLI `--dry-run`, console forms, agent input schema |
| Summary/tier/flags | the `CommandDescriptor` | every front-end |
| Effectful classification | `forge-policy` | catalog `effectful` field |

**Rule:** no consumer hard-codes any of these. The CLI, console, and agent read
the catalog; the catalog references the schemas and the role table. One fact, one
home, many readers.

## Schema strategy

- New per-command files live under `schemas/commands/`. They **reuse existing
  object schemas** (the ~23 `schemas/*.schema.json` for manifests, records, bridge
  contracts) via `$ref` rather than re-declaring shapes — so a change to the
  record schema flows into every command that embeds it.
- `forge-schema` (the Rust schema registry: collections, fields, migrations)
  remains the authority for *data* shapes; command schemas reference it where the
  payload is a data object.
- Until a command has a schema, it is `stability: preview` and the console falls
  back to a raw-JSON textarea. Authoring a schema is the gate to `stable`.

## Wiring into the public contract

Today `tools/export-public-contract.mjs` emits **command names only**, from
hardcoded arrays (`:86`–`121`), and `verify` checks hashes/presence, not schema
correctness. The plan upgrades this:

### E1 — Export the catalog, not a hand-list

Replace the hardcoded `bridge.methods` array with the **emitted catalog** (names
+ tiers + role sets + schema refs + `catalogVersion`). Source the export from
`system.describe` output (or the same static table), so the contract and the
runtime cannot disagree.

### E2 — Add a catalog drift gate

`verify-public-contract.mjs` gains a check: the `catalogVersion` /
per-command hash in `artifacts/public-contract.json` must equal what the current
build's catalog produces. A command added/changed without refreshing the contract
**fails CI** — the same posture as the existing hash checks, extended to command
metadata.

### E3 — Respect the generated-app boundary

The contract already carries `generatedAppBoundary.api` (the `ctx.*` inner
surface, `:213`). Keep inner vs outer separate in the export: outer commands under
the command catalog; `ctx.*` under the generated-app boundary. Visibility `debug`
commands are **excluded** from the public contract entirely.

## Determinism of the contract

- The catalog serializes in a **stable order** (sorted by name) with stable JSON,
  so `catalogVersion` is reproducible across builds (required by both the replay
  posture and the hash gate — see [06](06-PHASE-2-INTROSPECTION-COMMAND.md) P2.3).
- No clock/random enters the export (consistent with the repo's determinism
  rules and the `Date.now`/`Math.random` constraints in tooling).

## Downstream (Terrane Premium)

Per `CLAUDE.md`, after a contract change is accepted here:

```sh
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
```

Then refresh the Premium pin in `../terrane-premium` intentionally and run its
contract verification. The unified CLI is **public surface**, so Premium consumes
the catalog through the contract — never a private fork.

## Deliverables

- `schemas/commands/*.schema.json` (reusing existing object schemas).
- Export upgraded to emit the catalog with `catalogVersion`.
- Verify upgraded with a catalog-drift gate.
- Docs note in `docs/35_PUBLIC_CONTRACT_EXPORT.md` describing the new catalog
  section.

## Exit criteria

- Adding/altering a command without refreshing the contract fails `verify`.
- The exported catalog matches `system.describe` for the same build.
- `debug` commands never appear in the public contract; `ctx.*` stays under the
  generated-app boundary.
