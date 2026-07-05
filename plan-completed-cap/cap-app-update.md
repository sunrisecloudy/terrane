# Capability: `app` upgrade — versioned bundles, data migration, rollback stance

An **extension of the existing `app` crate**, not a new namespace. Today the
catalog is `app.add` / `app.import` / `app.remove`; `app.added` records
`{id, name, source, runtime}` with **no version field**, `BundleManifest`
(`terrane-host/src/lib.rs`) has no `version`, and `app.import` refuses an
existing id outright (`AppExists`) — there is no upgrade path at all. Bundle
bytes today live **in the event log**: `import_app_bundle` records one
`kv.set` per file under `__terrane/app-bundle/<path>` (source
`kv://app-bundle/<id>`), so folded kv state *is* the installed bundle and
replay already rebuilds it.

## Locked decision

**Forward-only, migration-gated.** An upgrade is a recorded fact plus ordinary
kv events; there is no undo event. "Rollback" is just another `app.upgrade` to
an older bundle, and it is allowed **only if that bundle's `dataVersion`
equals the currently folded one** — data migrations run forward and are not
required to be reversible, so code can go back but data cannot. This is an
honest asymmetry, stated in `doc.rs`, not hidden behind a fake `app.rollback`.

## Manifest: the `version` field

`BundleManifest` gains `version` (semver string, default `"0.0.0"` when
absent — every existing bundle is implicitly `0.0.0`) and `dataVersion`
(integer, default `0`, owned by
[cap-schema-migration.md](cap-schema-migration.md) — **hard dependency**).
`app.added` stays byte-compatible; version enters the log through the new
event below.

## Command / event surface

| Surface | Name | Notes |
| --- | --- | --- |
| Command | `app.upgrade` | args `id, bundle_path` → `Decision::Effect(Effect::UpgradeAppBundle { id, source })`; decide checks the app exists (else `AppNotFound`) |
| Event (new) | `app.upgraded` | `{ id, from_version, to_version, bundle_hash }` |
| Events (reused) | `kv.set` / `kv.rm` | replace changed bundle files under `__terrane/app-bundle/`, remove files absent from the new bundle |
| Events (reused) | `blob.stored` | archive the outgoing + incoming bundle in the CAS (below) |
| Command (kept) | `app.import` | unchanged: fresh install only, still `AppExists` on collision |

Edge behavior (`Effect::UpgradeAppBundle`): read + validate the bundle exactly
as `import_app_bundle` does (manifest, id match, runtime, symlink/text rules),
compare `version` (must differ from current; downgrade allowed under the
dataVersion rule), ask schema-migration for pending steps between the two
`dataVersion`s, and return one batch: migration events (per
cap-schema-migration.md), `app.upgraded`, then the kv file diff. One batch ⇒
upgrade is atomic in the log: replay never observes a half-upgraded app.

`bundle_hash` = sha256 over the sorted `(path, sha256(content))` list —
computed at the edge, recorded, and stable across replicas.

## Version history + bundle bytes

- Fold: `AppRecord` gains `version` and `history: Vec<VersionEntry { version,
  bundle_hash, seq }>` — the catalog remembers every version it has run, cheap
  because it is metadata only.
- Bytes: current bundle stays in kv (unchanged, replay-native). For **history**,
  the edge content-addresses each bundle as a tar-shaped canonical archive
  into the blob CAS ([cap-blob.md](cap-blob.md)) via `blob.stored` under
  `__app__/<id>/<version>` — so an old version is one CAS read away instead of
  event-log archaeology, and rollback-by-upgrade can take a `version` instead
  of a path: `app.upgrade <id> --to-version <v>` re-installs from the CAS.

## Preview / staging

No second staging area. The builder/MCP draft system
(`app_build_start/put_file/validate/commit`, `builder.*` events, and the host
preview store in `terrane-host/src/preview.rs`) already stages bundle files
and serves them at a preview route. Upgrade previews reuse it: seed a draft
from the incoming bundle, preview, then `app.upgrade` with the draft's files.
The only new piece is a `--from-draft <draftId>` arg on `app.upgrade`.

## Replay story

Everything an upgrade changes is events: `app.upgraded` (fold: version +
history), kv file diff (fold: the bundle apps actually run), migration events
(fold: migrated data). Replaying the log reproduces the exact sequence of
installed bundles and migrated data with no filesystem or CAS access; the CAS
archive is a convenience artifact verified by hash, exactly the cap-blob
stance.

## Security / permissions

- `app.upgrade` is an owner/CLI/admin verb, not an app-callable resource — an
  app must never upgrade itself or others. No grant resource is added.
- The bundle validation rules of `app.import` (id safety, symlink rejection,
  text-only, runtime whitelist) apply unchanged; a signed-archive install path
  is [cap-publish.md](cap-publish.md)'s job and composes on top.

## Limits

- `version` must parse as semver (`X.Y.Z`, optional pre-release); ≤ 64 chars.
- History capped at 100 entries per app (oldest metadata dropped; CAS rows are
  kept and remain GC-eligible when unreferenced).
- Same-version upgrade with identical `bundle_hash` is a typed no-op error
  ("already at 1.2.0"); identical version with different hash is rejected
  (versions are immutable — bump it).

## Implementation plan

1. **Manifest:** `version` + `dataVersion` on `BundleManifest` with defaults;
   validation (semver parse) beside `validate_bundle_id`.
2. **Interface:** `Effect::UpgradeAppBundle { id, source }`.
3. **Crate `terrane-cap-app`:** `app.upgrade` decide arm, `upgraded_event()`
   constructor, fold (version + history on `AppRecord`), describe, `doc.rs`.
4. **Edge:** `UpgradeAppBundle` arm in `EdgeRunner::run` — validate, diff kv
   file set, call schema-migration's pending-steps API, CAS-archive both
   bundles, return the single batch. `--from-draft` wiring via the preview
   store.
5. **CLI:** `terrane app upgrade <id> <bundle|--to-version v|--from-draft d>`;
   MCP mirrors it as `app_upgrade`.
6. **Tests:** engine tests `terrane-core/tests/cap/app.rs` extension (upgrade
   fold, history, downgrade gate on dataVersion, replay identity across
   upgrade+migration batch); e2e `terrane-host/tests/cap/app.rs` (real bundle
   dir → upgrade → files replaced/removed, version history, CAS archive
   round-trip, same-version rejection) — default-run, no network.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Reversible migrations, automatic update checks and update feeds (that is
[cap-publish.md](cap-publish.md) + a later auto-update pass), delta/binary
patching, per-file version pinning, wasm bundle upgrades (blocked on wasm
bundles getting the kv storage treatment first).

## Decisions to confirm

- **CAS-archive every version** — *recommendation:* yes, both outgoing and
  incoming bundle at upgrade time; it is cheap (text bundles, deduped) and
  makes rollback-by-version real. *Alternative:* history as metadata only,
  rollback requires the original bundle directory — simpler, but rollback then
  depends on files Terrane does not control.
- **Downgrade gate strictness** — *recommendation:* `dataVersion` equality.
  *Alternative:* allow downgrade across dataVersions when the migration chain
  declares itself reversible — deferred until cap-schema-migration.md grows
  reversible steps.
- **Version history retention** — *recommendation:* cap at 100 entries.
  *Alternative:* unbounded (it is only metadata) — fine until sync multiplies
  it across replicas.
