# Phase A — Data first (steps A1–A5)

**Theme:** stand up `forge/data/` and the per-shell loader, then move hard-coded enums / catalogs /
config / command tables out of source into shared JSON, and collapse the SQLite schema to one
authoritative migration set. **Low risk, mostly no app-visible behavior change.** This phase alone
delivers the "information as data, not hard-coded Swift" ask and is the foundation everything else
plugs into.

**Validation that applies to all of Phase A:** `swift build && swift test` for the macOS package
(buildable in this environment); `cargo test -p forge-domain` for enum roundtrips; per-shell unit
test asserting the loaded data equals the previous hard-coded literal; export + verify public
contract whenever an app-visible data file changes. Windows/Linux/Android changes are reviewed +
gated by their own CI (can't build them on macOS).

---

## A1 — `forge/data/` + per-shell loader plumbing

**Goal:** create the seam every later data step plugs into. No logic moves yet.

**Do:**
- Create `forge/data/` with a short `README.md` describing the directory's contract (single source
  of truth, generated-vs-authoritative rule from [02](02-target-architecture.md)).
- Add a minimal per-shell **loader** that reads a named JSON resource into typed structs once at
  startup, starting with macOS (`RuntimeDataCatalog` / `ForgeData` Swift type). Bundle the data
  files as build resources (SwiftPM `.copy`/`.process`, CMake/meson resource copy, Android assets).
- Document the directory in `forge/spec/` (e.g. `forge/spec/data-catalog.md`) and wire `forge/data/`
  into `tools/export-public-contract.mjs` as a contract input dir.

**Validation:** `node tools/export-public-contract.mjs … && node tools/verify-public-contract.mjs …`;
each shell still builds and starts (macOS: `swift build`).

**Risk:** low. **App-visible:** no (infra). **Commits:** 1 (macOS loader + dir + spec + contract
wiring). Loader fan-out to the other four shells can be folded into A2's first real consumer.

---

## A2 — Extract non-replay enums/config to data

**Goal:** move the safe, not-replay-sensitive hard-coded data into JSON.

**Data files:** `bundled-apps.json`, `mime-types.json`, `env-variables.json`,
`control-plane-config.json`, `runtime-config.json` (the non-enum parts). See
[08-data-files.md](08-data-files.md) for schemas.

**Replace in source:**
- Bundled-app IDs + metadata — `native/ios/.../BundledAppCatalog.swift:4-11` and the inline copies in
  `native/windows/.../DevControlPlane.cpp:198-199`, Linux/Android control planes → load from
  `bundled-apps.json`.
- MIME map — `native/macos/.../WebHostView.swift` (`mimeType`), `windows/.../DevControlPlane.cpp:1913-1928`,
  `linux/.../dev_control_plane.c:5318-5332` → `mime-types.json`.
- Env var names — `native/macos/.../DevControlPlane.swift:20,24`, `ios/...:18-30`,
  `windows/...:57`, Linux/Android → `env-variables.json`.
- Control session-id prefixes + signing-key account/service patterns — `DevControlPlane.swift:16,80,106`,
  `ios/...:68`, `android/...:31` → `control-plane-config.json`.
- Runtime capability constants (byte limits, platform/target strings) — `WebBridge.swift:148-149,155-156`
  → `runtime-config.json`. **Resolve the runtime-version bug here** (one build-injected value; see
  [01-findings.md](01-findings.md) §D and the open question in [09](09-decisions-and-open-questions.md)).
- Engine-room hard-coded table list + featureFlags — `NativeEngineRoomSnapshotProvider.swift:73-90`
  → `engine-room-tables.json` (the telemetry stays macOS-only; only its *table list* becomes data).

**Validation:** per-shell test that the loaded list equals the old literal; macOS XCTest for
`BundledAppCatalog`; bundled-app list resolves identically.

**Risk:** low. **App-visible:** the bundled-app list and runtime constants are app-visible →
export/verify contract. **Commits:** ~1 per data file (5–6 small commits).

---

## A3 — Replay-sensitive enums, pinned in `forge-domain`

**Goal:** the enums that affect app-visible/replay behavior get a single Rust source of truth, with
the JSON **generated from** the Rust enum so they cannot drift.

**Add to `forge-domain`:** `AppletStatus { Enabled, Installed, Quarantined, RolledBack, Uninstalled,
Disabled }`, `TrustLevel { Developer, Trusted, Untrusted }` (default `Developer`), `SnapshotType`
(bug-report, pre-install, pre-migration, post-test, golden, manual, debug-bundle; import-only:
backup, test-fixture). Reference them in `forge/spec/applet-lifecycle.md` + `commands.md`.

**Generate** `forge/data/{snapshot-types,app-status-enums,trust-levels,package-manifest}.json` from
the enums (a generator test that fails if the JSON is stale).

**Replace in source:** the string literals / `Set` allowlists in all five DevControlPlanes
(`DevControlPlane.swift:81-89`, `ios/...:3515-3521`, `windows/...:5036-5042`, `linux/...:6705-6711`,
`android/...:2090`) and the status/trust literals embedded in `PlatformAppRegistry.swift:68-99,84`
and the SQL in the other shells → loaded values.

**Validation:** `cargo test -p forge-domain` (enum ⇄ JSON roundtrip); export/verify contract (these
are app-visible + replay-sensitive); per-shell test that validation accepts/rejects the same set.

**Risk:** medium (replay-sensitive). **App-visible:** yes. **Commits:** ~2 (Rust enums + generator,
then shell swap).

---

## A4 — Control-tools catalog + response envelope as data

**Goal:** fix the canonical *contract* of the debug control surface before consolidating its logic
in Phase B. Shells keep their dispatch `switch` but **drive name validation, param presence,
capability gating, and response formatting from the catalog**.

**Data files:** `control-commands.json` (105+ tools: name, namespace, category, params schema,
returns, per-platform capability matrix reflecting the iOS/Android reduced sets) and
`control-response-schema.json` (the `{ok, result/error{code,message,details}, diagnostics{target,
sessionId,timestamp}}` envelope).

**Replace in source:** the tool-name allowlists and response formatting in all five control planes
(`DevControlPlane.swift:336-504,6159-6195`, `ios/...:535-669,401-430`, `linux/...:166-334,7150-8000`,
`windows/...`, `android/...:201-263,420-440`) → catalog-driven validation + a shared formatter shape.

**Validation:** a schema self-test that every tool a shell dispatches exists in the catalog and
vice-versa; per-shell test that an unknown tool returns the same 400/501 envelope as before.

**Risk:** medium. **App-visible:** the response envelope is observed by test harnesses → treat as
app-visible-ish; lock it with the schema. **Commits:** 1 (catalog + schema) + 1 per shell wiring.

> The **capability matrix** in `control-commands.json` is where iOS/Android's reduced surface
> becomes *explicit data* instead of accidental drift. Confirm the intended matrix — see the open
> question in [09](09-decisions-and-open-questions.md).

---

## A5 — One authoritative SQLite schema; delete the unsafe fallback

**Goal:** stop hand-maintaining the `apps/app_versions/app_installations` (+ debug tables) schema in
five `PlatformDatabase` files. Make `forge/crates/storage` migrations the single source.

**Do:**
- Confirm/extend forge migrations to cover the shell tables that must persist
  (`apps, app_versions, app_installations`, and the debug tables `runtime_snapshots, bridge_calls,
  core_events, runtime_sessions, network_mocks`). Every shell applies the **same** migration set;
  Android maps the same logical schema via Room.
- **Delete** the partial-fallback `CREATE TABLE` in each `PlatformDatabase`
  (`native/macos/.../PlatformDatabase.swift:55-56` + four siblings) — the path that builds only
  `apps`+`app_storage` on migration failure (bug §D.2). Migration failure should fail loudly.
- Add `forge/data/tables.json` as **derived documentation only** (generated from migrations), never a
  second source of schema.

**Validation:** `cargo test -p forge-storage` (migration apply + idempotency); per-shell test that a
fresh DB yields the full schema with no fallback path; host persistence regression check
(in-memory vs on-disk — see the host-persistence regression note in project memory).

**Risk:** medium. **App-visible:** no (persistence contract, but no behavior change if schema
matches). **Commits:** 1 (migrations + tables.json) + 1 per shell deleting fallback.

---

## Phase A exit criteria

- `forge/data/` exists, is loaded by all shells, and is a contract input.
- No bundled-app list, MIME map, snapshot/status/trust enum, control-tool table, env-var name, or
  config constant remains hard-coded in any shell — all come from data.
- Exactly one SQLite schema (the migrations); the unsafe fallback is gone.
- Public contract re-exported and verified; runtime-version bug fixed.
- Every commit green: `swift build/test` (macOS) + `cargo test -p forge-{domain,storage}`.
