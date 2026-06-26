# 08 — `forge/data/*.json` inventory

Every shared data file: its purpose, a schema sketch, who is the source of truth, who consumes it,
and the phase that creates it. **Rule:** files that mirror a Rust enum are *generated from* the enum
(Rust is authoritative); pure-config files are authoritative as JSON; `tables.json` is always
*derived docs* of the migrations.

Legend — **SoT** = source of truth. **P0/P1/P2** = priority within Phase A.

---

### `bundled-apps.json` — P0 — A2
6 bundled app IDs + metadata. **SoT:** JSON (authoritative). **Consumers:** all 5 shells.
Replaces inline arrays in `BundledAppCatalog.swift:4-11`, `windows/DevControlPlane.cpp:198-199`, etc.
```json
[ {"id":"notes-lite","name":"Notes Lite","version":"…","description":"…",
   "contentRating":{"minimumAge":0}}, … 6 entries: notes-lite, task-workbench,
   file-transformer, api-dashboard, core-replay-lab, calendar-planner ]
```

### `snapshot-types.json` — P0 — A3
Allowed snapshot types + import-only set. **SoT:** generated from `forge-domain` `SnapshotType`.
**Consumers:** all 5 control planes. Replaces literals at `DevControlPlane.swift:81-89` + siblings.
```json
{ "types":["bug-report","pre-install","pre-migration","post-test","golden","manual","debug-bundle"],
  "importOnly":["backup","test-fixture"] }
```

### `app-status-enums.json` — P0 — A3
App + version status values. **SoT:** generated from `forge-domain` `AppletStatus`. **Consumers:** all
shells + `PlatformAppRegistry`. Replaces SQL literals at `PlatformAppRegistry.swift:68-99`.
```json
{ "app_status":["enabled","disabled","quarantined"],
  "version_status":["enabled","installed","rolled-back","quarantined","uninstalled"] }
```

### `trust-levels.json` — P0 — A3
Trust levels + default. **SoT:** generated from `forge-domain` `TrustLevel`. **Consumers:** all shells.
Replaces `'developer'` literals at `PlatformAppRegistry.swift:84`, `DevControlPlane.swift:2647,3533,3831`.
```json
{ "levels":["developer","trusted","untrusted"], "default":"developer" }
```

### `tables.json` — P0 — A5 (DERIVED DOCS ONLY)
Human-readable documentation of the SQLite schema. **SoT:** the `forge-storage` migrations (this file
is generated from them, never the reverse). **Consumers:** docs/tooling. Replaces nothing at runtime;
exists so the schema is discoverable.

### `mime-types.json` — P1 — A2
Extension → content-type + default. **SoT:** JSON. **Consumers:** mac/iOS/Win/Lin static file serving.
Replaces `WebHostView.swift` `mimeType`, `windows/...:1913-1928`, `linux/...:5318-5332`.
```json
{ "extensions":{".html":"text/html",".css":"text/css",".js":"text/javascript",
  ".json":"application/json"}, "default":"text/plain" }
```

### `package-manifest.json` — P1 — A3
Required + allowed package files / prefixes. **SoT:** JSON (validated against `forge-domain`).
**Consumers:** mac/Linux/Windows package validation. Replaces `linux/...:5341-5347`,
`WebHostView.swift:492,514,570-574`, `ios/...:1171,1371`.
```json
{ "required":["manifest.json"], "entry_point":"index.html",
  "allowed_files":["index.html","styles.css","app.js","smoke-tests.json","README.md"],
  "allowed_prefixes":["migrations/","assets/"] }
```

### `control-commands.json` — P1 — A4
The 105+ control tool catalog + **per-platform capability matrix**. **SoT:** JSON. **Consumers:** all
5 control planes (name/param/capability validation). Replaces the switch allowlists at
`DevControlPlane.swift:336-504` + siblings.
```json
[ {"name":"platform.health","namespace":"platform","category":"meta",
   "params":{}, "returns":{}, "platforms":["macos","ios","linux","windows","android"]},
  {"name":"platform.install_webapp_package","platforms":["macos","linux","windows"]}, … ]
```

### `runtime-config.json` — P1 — A2/A3
Runtime constants: **the canonical `runtimeVersion`** (fixes the 0.1.0/0.4.0 bug), platform/target
ids, byte limits. **SoT:** build-injected from `forge-runtime` (per the canonical-version decision in
[09](09-decisions-and-open-questions.md)). **Consumers:** all shells. Replaces `WebBridge.swift:148-149,155-156,158`.
```json
{ "runtimeVersion":"<one canonical value>", "maxPackageBytes":1048576,
  "maxFileBytes":524288, "defaultFileMaxBytes":1048576,
  "platform":"<per-shell override>", "target":"<per-shell override>" }
```

### `control-response-schema.json` — P2 — A4
The control-plane response envelope (JSON Schema). **SoT:** JSON Schema. **Consumers:** all 5 control
planes (response formatting). Locks `{ok, result/error{code,message,details}, diagnostics{target,
sessionId,timestamp}}` across `DevControlPlane.swift:6159-6195` + siblings.

### `control-plane-config.json` — P2 — A2
Control session-id prefixes + signing-key account/service patterns per platform. **SoT:** JSON.
**Consumers:** all 5 control planes. Replaces `DevControlPlane.swift:16,80,106`, `ios/...:68`,
`android/...:31`.
```json
{ "signingKey":{"service":"terrane.dev-control","accountFormat":"terrane.{platform}.dev-control.platform-key"},
  "sessionIdPrefix":{"macos":"control_","ios":"control_ios_","android":"control_android_"},
  "tokenFileLocation":"terrane/control.token" }
```

### `env-variables.json` — P2 — A2
Per-platform env-var contract for control-plane config. **SoT:** JSON. **Consumers:** all 5 shells.
Documents `TERRANE_MACOS_CONTROL_PORT`, `TERRANE_IOS_*`, `PLATFORM_CONTROL_TOKEN*`, etc.
(`DevControlPlane.swift:20,24`, `ios/...:18-30`, `windows/...:57`).
```json
{ "portEnvVars":{"macos":"TERRANE_MACOS_CONTROL_PORT","ios":"TERRANE_IOS_CONTROL_PORT", …},
  "devControlEnvVars":{…}, "signingKeyEnvVars":{…}, "tokenFileEnvVars":{…} }
```

### `engine-room-tables.json` — (macOS telemetry) — A2
The table list the engine-room snapshot enumerates. **SoT:** JSON (derived from the schema).
**Consumers:** macOS `NativeEngineRoomSnapshotProvider` (telemetry stays macOS-only; only its table
list becomes data). Replaces hard-coded list at `NativeEngineRoomSnapshotProvider.swift:73-90`.

---

## What is NOT a loose JSON file

Two data-shaped things stay in **Rust**, not `forge/data/`, because they are tightly coupled to code
and already modeled as data there:

- **The command registry** (`name → handler`) — already a static table in
  `forge/crates/core/src/commands/mod.rs`. New commands are rows there, not JSON.
- **The bridge method → permission map** — belongs in `forge-domain` `manifest.rs` / surfaced in
  `forge/spec/capabilities.md` as a Rust data table, not a loose file (it is consumed by Rust policy
  logic, C10).
