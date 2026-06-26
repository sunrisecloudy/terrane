# 02 — Target Architecture

## Three layers, one direction

```
   ┌──────────────────────────────────────────────────────────────────────┐
   │ LAYER 1 — Native shell (per platform, THIN)                           │
   │   genuine OS glue only: WebView/dialogs, HTTP listener, SQLite        │
   │   transport, key custody, OS install/uninstall.                       │
   │   Calls DOWN through the JSON seam. Loads data files for constants.    │
   └───────────────┬──────────────────────────────────┬───────────────────┘
                   │ terrane_forge_core_handle_command │ load once at startup
                   │   (json) -> (json)  + drain_events│
   ┌───────────────▼──────────────────┐   ┌────────────▼───────────────────┐
   │ LAYER 2 — Forge core (Rust)      │   │ LAYER 3 — forge/data/*.json     │
   │   • 51+ existing commands        │   │   shared, declarative,          │
   │   • new bridge.* / package.* /   │◀──│   single-source-of-truth data:  │
   │     quota.* commands             │   │   enums, catalogs, command       │
   │   • forge-controlcore (debug)    │   │   tables, MIME, config.          │
   │   • forge-storage (one schema)   │   │   (some generated from Rust)    │
   └──────────────────────────────────┘   └─────────────────────────────────┘
```

The seam already exists (`native/*/CForgeCoreBridge` → `forge/crates/ffi`). **No new FFI entry
points are introduced** — every new capability is a new *command name* dispatched by the existing
`handle_command`, exactly like the 51 commands today. (The control-plane debug surface is the one
candidate for a separate debug-gated entry point; see the open question in [09](09-decisions-and-open-questions.md).)

## The seam contract (unchanged)

```c
// native/<platform>/.../CForgeCoreBridge.h  — already exists, do not grow it.
TerraneForgeCore *terrane_forge_core_open(const char *lib, const char *db, const char *workspace);
void              terrane_forge_core_close(TerraneForgeCore *);
char             *terrane_forge_core_handle_command(TerraneForgeCore *, const char *command_json);
char             *terrane_forge_core_drain_events(TerraneForgeCore *);
char             *terrane_forge_core_last_error(void);
void              terrane_forge_core_free_string(TerraneForgeCore *, char *);
```

Command envelope (already used by the macOS `ForgeCoreBridge`):

```json
{ "request_id": "...", "actor": {"actor": "...-host", "role": "owner"},
  "workspace_id": "...", "name": "<command>", "payload": { ... } }
```

Adding a command = one row in the `COMMANDS` table in `forge/crates/core/src/commands/mod.rs` + a
handler module. The shell stops doing the work and instead builds this envelope and reads the
`payload` back.

## Decision logic moves; transport stays

The rule that resolves every "does this move?" question:

> **Move the DECISION. Keep the TRANSPORT.**

| Responsibility | DECISION (→ core) | TRANSPORT (stays in shell) |
|---|---|---|
| `network.request` | is this host private? does the policy allow it? | `URLSession` / WinHTTP / Soup actually fetching |
| `storage.*` | is the key prefix valid? is the permission granted? | `sqlite3_*` reads/writes |
| bridge dispatch | is the envelope valid? which permission? over budget? | receiving `WKScriptMessage`, posting the reply |
| snapshot | build/compare the normalized payload | `INSERT INTO runtime_snapshots` |
| app rollback | which version is the target? is it compatible? emit audit | the WebView reload, file moves |
| package install | validate manifest, compute hash, verify signature | copying files into place |

## Two product models during cutover (package vs applet)

The repo is mid-pivot: native shells still run **v0.4 legacy webapp packages** (HTML/CSS/JS on disk)
while Forge core already implements the **v1 workspace applet** lifecycle. They are not the same
model and must not share one command namespace during this program.

```
  Legacy webapp (shells today)              v1 workspace applet (Forge core today)
  ┌────────────────────────────┐          ┌────────────────────────────┐
  │ platform.sqlite            │          │ forge-workspace.sqlite     │
  │ apps / app_versions /      │          │ InstalledApplet in meta/kv │
  │ app_installations          │          │ install_generation +       │
  │ file-based package install │          │ AppletLifecycle Active/    │
  │ legacy.core_step only      │          │ Suspended                  │
  │                            │          │ applet.install / upgrade … │
  └─────────────┬──────────────┘          └─────────────┬──────────────┘
                │                                       │
                │  package.*  (NEW — this program)       │  applet.*  (EXISTING — unchanged)
                └──────────────────┬────────────────────┘
                                   │
                    terrane_forge_core_handle_command(json)
```

| Layer | DB file | Identity | Lifecycle commands | This program |
|---|---|---|---|---|
| Legacy webapp package registry | `platform.sqlite` | `app_id` + `install_id` + integer `version` | **`package.*`** (new) | Shells + reference-host delegate here |
| v1 workspace applet | `forge-workspace.sqlite` | `applet_id` + `install_generation` + `version` + `code_hash` | **`applet.*`** (existing) | Unchanged; shells still call `legacy.core_step` until a later cutover |

**Rules for implementers:**

- New lifecycle authority for rollback / quarantine / activation of **installed webapp packages** →
  `package.list_versions`, `package.activate_version`, `package.rollback_version`,
  `package.set_status`, plus `package.get_manifest` / `package.get_permissions` (C9). **Do not**
  overload `applet.install` or extend `applet.*` to cover the shell registry.
- A3 status enums in `forge-domain` are **`PackageVersionStatus`** / **`PackageAppStatus`**, not
  `AppletStatus` (which would collide with `AppletLifecycle` in `forge/crates/core`).
- Full migration from webapp packages to `applet.install` is a **post-D milestone**, out of scope
  for A–E. See Q8 in [09](09-decisions-and-open-questions.md).

## Database layout during this program

Shells open **two SQLite files** today. This program unifies **authority and migrations**, not
physical file count (see Q9).

| File | Owner | Tables (representative) | Authority after A–E |
|---|---|---|---|
| `platform.sqlite` | Shell transport; core writes via commands from D12 | `apps`, `app_versions`, `app_installations`, `app_storage`, debug tables (`runtime_snapshots`, `bridge_calls`, `core_events`, `runtime_sessions`, `network_mocks`) | **A5:** one shared migration set, no unsafe fallback. **D12:** core owns registry **writes**; shell stops raw SQL on `app_versions.status`. |
| `forge-workspace.sqlite` | Forge core (`forge_core_open`) | `meta`, `kv`, `oplog`, `records`, `runs`, `audit_log`, `crdt_*`, … | Unchanged; v1 applet lifecycle stays here. |

**`app_storage`:** prefix/quota decisions move to core (C10); rows remain in `platform.sqlite` with
shell `sqlite3_*` transport through A–E. Consolidating `app_storage` into forge KV is follow-on work.

**Single-file merge** (one DB for workspace + platform tables) is deferred until post-D once
`package.*` authority, backup/export, and replay semantics are proven.

## Sixth consumer: reference-host

`tools/reference-host/` is not a throwaway dev server — it is the **Node conformance oracle** and
duplicates the same registry + control-plane logic as the five native shells
(`platform-database.js` mirrors `PlatformAppRegistry` rollback/quarantine SQL).

Treat it as a **sixth first-class consumer** in every phase that touches control-plane or lifecycle
logic:

| Phase | reference-host obligation |
|---|---|
| A4 | Load `control-commands.json` + format responses per `control-response-schema.json` |
| B6–B7 | Delegate DevControlPlane algorithms to `forge-controlcore`; delete duplicated JS |
| D12 | Call `package.*` commands; no raw `UPDATE app_versions` in `platform-database.js` |

**Fan-out order:** macOS **and** reference-host together per phase, then iOS → Linux → Windows →
Android. Golden vectors captured from macOS must also pass through reference-host before a B/D step
is marked done.

## Where new core code lives

| New module | Crate | Why |
|---|---|---|
| `bridge.*` validation, permission map, rate budget | `forge-core` + `forge-policy` | policy decisions, replay-sensitive |
| network policy + private-IP detection | `forge-policy` / `forge-domain` | pure, security-critical, must be one impl |
| manifest fields (`permissions`, `networkPolicy`, `resourceBudget`, `denyPrivateNetwork`) | `forge-domain` (`manifest.rs`) | reuse `Manifest`; one parser; wire existing `forge-policy` private-IP (C8 is delegate, not rewrite) |
| `package.list_versions/activate/rollback/set_status`, webapp version history | `forge-core` + `forge-storage` | legacy registry authority + audit; distinct from `applet.*` |
| `quota.auto_quarantine` + status fields | `forge-core` / `forge-policy` | budget-driven decision |
| DevControlPlane pure logic (snapshot/html/a11y/mocks/replay/backup/package) | **`forge-controlcore`** (new, debug-gated) or a module in `forge-testkit` | debug-only, biggest line win; reference-host is a consumer |
| token/signature format + verify | `forge-signing` | delegate to existing `terrane/sig/v1` primitives (E13 is unify callers, not new format) |
| `PackageVersionStatus`, `PackageAppStatus`, `TrustLevel`, `SnapshotType` enums | `forge-domain` | source of truth for the data files; do not collide with `AppletLifecycle` |

Keep pure-logic crates (`domain`, `policy`, `schema`) **`wasm32`-clean**: the private-IP/policy/
manifest/enum logic must compile to wasm; native-only deps stay behind `cfg(not(wasm32))`.

## The `forge/data/` directory (new)

A new top-level data directory, loaded by every shell and (where the data mirrors a Rust enum)
**generated from** the Rust source so the two can't drift:

```
forge/data/
  bundled-apps.json          # 6 app IDs + metadata          (A2)
  mime-types.json            # extension → content-type       (A2)
  env-variables.json         # per-platform env var contract  (A2)
  control-plane-config.json  # session-id prefixes, key names (A2)
  snapshot-types.json        # generated from SnapshotType    (A3)
  app-status-enums.json      # generated from PackageVersionStatus (A3)
  trust-levels.json          # generated from TrustLevel       (A3)
  package-manifest.json      # required/allowed files          (A3)
  control-commands.json      # tool catalog + capability matrix(A4)
  control-response-schema.json # {ok,result/error,diagnostics}(A4)
  runtime-config.json        # runtimeVersion, byte limits     (A2/A3)
  tables.json                # DERIVED docs of the SQL schema  (A5, docs-only)
```

Each shell gets a tiny **loader** (one per language) that reads a named JSON resource into typed
structs once at startup. The loader is the only new "infrastructure" code; everything after it is
deleting hard-coded literals and pointing at the loaded data. See [08-data-files.md](08-data-files.md)
for each file's schema and source of truth.

> **Single source of truth rule:** when a data file mirrors a Rust enum (statuses, trust levels,
> snapshot types), the **Rust enum is authoritative** and the JSON is generated from it (build step
> or a checked-in generator test). When the data is pure config (MIME, env names, catalog), the JSON
> is authoritative. `tables.json` is always *derived docs* of the migrations — never a second schema.

## Public-contract posture

`forge/data/` and any new app-visible command must be wired into
`tools/export-public-contract.mjs` so `artifacts/public-contract.json` reflects them, and verified
with `tools/verify-public-contract.mjs`. Premium consumes the contract; after an accepted
app-visible change, refresh the Premium pin intentionally (see [10](10-validation-and-sequencing.md)).
Debug-only surfaces (`forge-controlcore`, control-plane) are **not** part of the public contract.
