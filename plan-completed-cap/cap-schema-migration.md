# Capability: `migration` — versioned app data with deterministic migrations

New crate `rust/crates/terrane-cap-migration/`, namespace `migration`,
registered in `default_registry`. Today a shipped app's data cannot evolve: a
new `main.js` that renames a kv key or reshapes a row simply meets old data
and breaks. `relational_db` already carries the vocabulary at table level
(`TableSpec.spec_version`/`schema_version` in
`terrane-cap-relational-db/src/spec.rs`) but nothing versions *an app's data
as a whole*, and nothing runs the rewrite. This capability records a per-app
**data version** as an event fact and runs **migration scripts** as ordinary
JS through the existing runtime path, so replay never re-runs them.

## Recommended decision

**Option A migrations, forward-only, one commit batch per step.** A migration
is a JS script executed exactly like an app backend (`Decision::Runtime` →
QuickJS over `RuntimeHostHandle`): its writes go through
`ctx.resource.kv`/`ctx.resource.relational_db` and are recorded as the
ordinary `kv.*`/`relational_db.*` events those capabilities already own.
Replay folds those events and **never re-runs the script** — identical to how
app backends replay today. As its final act the script's runner records the
version bump *in the same batch*, so a step is atomic: `Core::run_runtime`
commits `host.take_records()` only after the runtime returns `Ok`, meaning a
throwing script commits **nothing** (data writes and version bump land
together or not at all). Rollback is not a mechanism: downgrading is refused,
and the recovery story is a home backup taken before migrating
([cap-backup-export.md](cap-backup-export.md)).

## Manifest contract

`manifest.json` (parsed by the existing `BundleManifest` in
`terrane-cap-js-runtime/src/bundle.rs`, two new optional fields):

```jsonc
{
  "id": "todo", "name": "Todo", "runtime": "js", "backend": "main.js",
  "resources": ["kv"],
  "dataVersion": 3,                          // version main.js expects; default 1
  "migrations": [                            // consecutive, sorted, ends at dataVersion
    { "to": 2, "script": "migrations/002-split-title.js" },
    { "to": 3, "script": "migrations/003-add-done-flag.js" }
  ]
}
```

A migration script defines one global `migrate(ctx)` and uses only
`ctx.resource.*` (same sandbox, same grants as the app backend — a migration
can touch nothing the app itself cannot). Scripts should be pure data rewrites;
effects (`net`, `model`) are refused by the migration runtime host.

## Capability surface

Fold keeps `app → { version, history: [{from, to, script_hash}] }`. An app
with no recorded fact is at version 1.

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `migration.apply` | args `app, to_version, script_source` → `Decision::Runtime`; the capability's `run_runtime` executes the script, then records the bump via an internal `migration.commit` write on the runtime host (same batch as the data events) |
| Command | `migration.commit` | args `app, from, to, script_hash` → pure `Commit([migration.applied])`; **trusted-host + runtime-internal only** (gated like `auth.*` in `admit_command`) |
| Event | `migration.applied` | `{ app, from_version, to_version, script_hash }` (sha256 of script source — the audit trail replay preserves) |
| Query | `migration.status` | args `app` → `{version, pending?}` for CLI/MCP |
| (reacts) | `app.removed` | drop the app's version fact |

No `ctx.resource.migration` — apps never migrate themselves; the host does.

## Host gate (the point of the whole thing)

Before `js-runtime.run` of a backend, `terrane-host` compares
`manifest.dataVersion` against the folded version:

- **state < manifest** — refuse to run with a typed error naming the pending
  steps and the fix: `terrane migrate <app>`. The CLI command walks the
  manifest's `migrations` list from the current version, dispatching one
  `migration.apply` per step (each step its own commit batch, so a
  mid-sequence failure leaves a consistent, correctly-versioned intermediate
  state — re-running `terrane migrate` resumes from there).
- **state > manifest** — refuse outright: the data was written by a newer app
  than the code on disk. Forward-only; restore from backup to go back.
- **equal** — run as today. Apps without `dataVersion` are version 1 forever
  and never gated (full backward compatibility).

`relational_db` schema changes ride inside scripts as ordinary
`relational_db.defineTable` calls with a bumped `schema_version` — this
capability adds no second table-versioning mechanism.

## Replay, determinism, limits

- Replay folds `kv.*`/`relational_db.*`/`migration.applied` events; the script
  never re-runs, so even a badly non-deterministic script cannot break
  replay-identity (determinism is still the documented expectation — the
  hash in `migration.applied` is meaningless otherwise).
- Migration steps must be consecutive (`to` = current + 1); gaps, repeats, or
  `to` ≤ current are typed errors in decide.
- Limits: script ≤ 512 KiB; per-step runtime budget reuses the js-runtime
  execution limits; ≤ 10 000 recorded events per step (typed error — a bigger
  rewrite should ship as several versions).

## Implementation plan

1. **Crate:** `terrane-cap-migration` — `lib.rs` (Capability: manifest,
   decide for `apply`/`commit`, fold, describe, `run_runtime` reusing the
   QuickJS engine exported by `terrane-cap-js-runtime`), `doc.rs`,
   `applied_event()` constructor. Add `MigrationState` to `terrane_core::State`
   and register in `default_registry`; gate `migration.*` as trusted-host in
   `admit_command`.
2. **Manifest:** extend `BundleManifest` with `data_version`/`migrations`
   (nserde defaults keep old manifests valid); bundle validation checks the
   list is consecutive and ends at `dataVersion`, and that script files exist.
3. **Host gate + CLI:** version check in the `run <app>` path (host/cli and
   `terrane-host` run surfaces); `terrane migrate <app>` and
   `terrane migrate status <app>`; MCP `app_actions` surfaces the pending
   state instead of a raw refusal.
4. **Docs:** `APP_API.md` migration-script contract; app-builder recipe note
   ("bumping data shape ⇒ add a migration, bump dataVersion").
5. **Tests:** engine tests `terrane-core/tests/cap/migration.rs` (apply →
   ordinary kv events + applied fact in one batch, throwing script commits
   nothing, gap/downgrade refusals, replay identity, app.removed); e2e
   `terrane-host/tests/cap/migration.rs` (real bundle: v1 data, upgrade
   manifest, gated run, `terrane migrate`, backend runs; resumed multi-step
   migration after an injected mid-sequence failure). All default-run.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Explicit non-goals (v1)

Down-migrations/rollback (backup is the rollback), automatic migration on run
(explicit `terrane migrate` only), cross-app migrations, migrating `crdt`
document internals (Loro data is schemaless by design), migration of
platform/host data (that is engine versioning, not app data).

## Decisions to confirm

- **Rollback stance** — forward-only; recovery = pre-migration backup —
  alternative: paired down-scripts per step (double the untested code paths).
- **Trigger** — host refuses stale backends and the user runs
  `terrane migrate` explicitly — alternative: auto-migrate on first run
  (silent data rewrites on launch; rejected as default but cheap to add later
  as a flag).
- **Atomicity granularity** — one commit batch per migration step, resumable
  between steps — alternative: all pending steps in a single batch (stronger
  but makes big multi-version upgrades one giant commit).
- **Version-bump channel** — internal `migration.commit` write recorded by the
  runtime host in the same batch — alternative: host dispatches a separate
  bump command after the run (a crash between the two leaves migrated data
  unversioned).
