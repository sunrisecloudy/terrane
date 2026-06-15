# Phase 0 Inventory, Snapshot, and Baseline

Date: 2026-06-15
Branch: `forge-m0a`
Frozen baseline: `498213987611bea97e76518e2a507347a8e0835a`
Slice: Phase 0.1-0.4 from `task-jun-15/03-LEGACY-REMOVAL-MIGRATION.md`

## Recovery Ref

Created recovery branch:

```text
legacy-archive/pre-removal -> 498213987611bea97e76518e2a507347a8e0835a
```

Verification:

```text
git rev-parse legacy-archive/pre-removal
498213987611bea97e76518e2a507347a8e0835a
```

## Working Tree Preservation

The migration started with existing dirty and untracked work. This Phase 0 slice
does not reset, clean, overwrite, or delete any of it. The only intended new
tracked artifacts for this slice are this inventory document and the paired
`review-from-claude/001-phase0-inventory.md` review file.

Pre-existing dirty/high-risk paths include remote-team-owned migration surfaces:

- `.github/workflows/release.yml`
- `README.md`
- `docs/12_RELEASE_AND_CI.md`
- `forge/spec/sync-protocol.md`
- `native/macos/*`
- `runtime-web/*`
- `tools/*`

They remain untouched by this slice.

## Tracked Legacy Inventory

```text
server/README.md
server/build.zig
server/src/main.zig
zig-core/README.md
zig-core/build.zig
zig-core/build.zig.zon
zig-core/include/zig_core.h
zig-core/src/lib.zig
zig-crdt/README.md
zig-crdt/build.zig
zig-crdt/build.zig.zon
zig-crdt/include/zig_crdt.h
zig-crdt/src/lib.zig
```

| Component | Tracked files | Source LOC | Notes |
|---|---:|---:|---|
| `zig-core/` | 5 | 434 | `src/lib.zig` 400 + `include/zig_core.h` 34 |
| `zig-crdt/` | 5 | 1,464 | `src/lib.zig` 1,416 + `include/zig_crdt.h` 48 |
| `server/` | 3 | 14,762 | `server/src/main.zig` |
| Total | 13 | 16,660 | Source plus public C headers |

`src/`-only LOC, matching the migration-plan scope, is 16,578:

```text
14762 server/src/main.zig
  400 zig-core/src/lib.zig
 1416 zig-crdt/src/lib.zig
16578 total
```

## Legacy Reference Map

Search terms:

```text
zig_core
zig-core
libzig_core
zig_crdt
zig-crdt
terrane_zig_core_
terrane_zig_crdt_
core_step_json
build-zig-core
build-server
server/src/main.zig
zig build
```

Search excluded `.git`, `forge/target`, `node_modules`, `build` directories,
Zig caches, generic caches, and `artifacts`.

### Live CI, Tooling, and Release References

| Path | Live dependency | Planned phase |
|---|---|---|
| `.github/workflows/ci.yml` | Setup/build Zig core/server and `package-release --build-zig-core/--build-server` | 2.8 |
| `.github/workflows/release.yml` | Setup Zig for release packaging | 2.9 |
| `tools/package-release.mjs` | Builds `libzig_core`/`zig_core.dll`, builds Zig server with `zig_core` + `zig_crdt`, stages native Zig libs | 2.6 |
| `tools/check-repo.mjs` | Hard-asserts Zig CI/package snippets, native Zig bridge snippets, and reads `server/src/main.zig` | 2.7 |
| `tools/README.md` | Documents Zig release artifacts | 2.6-2.7 |
| `docs/12_RELEASE_AND_CI.md` | Documents Zig release pipeline and native `libzig_core` bundles | 2.8-2.9 |

### Live Reference-Host Tests

| Path | Live dependency | Planned phase |
|---|---|---|
| `tools/reference-host/test/zig-core-build.test.js` | Builds `zig-core`, asserts `core_step_json` symbol | 2.10 |
| `tools/reference-host/test/zig-crdt-build.test.js` | Builds `zig-crdt`, asserts CRDT symbols | 2.10 |
| `tools/reference-host/test/server-zig-build.test.js` | Builds `server/src/main.zig` with Zig deps | 2.10 |
| `tools/reference-host/test/server-bridge-contract.test.js` | Exercises Zig server `/bridge` contract | 1.3, 2.10 |
| `tools/reference-host/test/server-db-schema-acceptance.test.js` | Reads `server/src/main.zig` | 1.3, 2.10 |
| `tools/reference-host/test/release-packaging.test.js` | Expects Zig artifacts in release output | 2.6, 2.10 |
| `tools/reference-host/test/runtime-capabilities-contract.test.js` | References `server/src/main.zig` capabilities | 1.3, 2.10 |
| `tools/reference-host/test/*-native-build.test.js` | Expects native hosts to build/link/load `libzig_core` or `zig_core.dll` | 2.1-2.5, 2.10 |
| `tools/reference-host/test/native-core-timeout-source.test.js` | Asserts native `core_step_json` loader behavior | 2.1-2.5, 2.10 |
| `tools/reference-host/test/native-runtime-resource-mapping.test.js` | Asserts native Zig bridge/resource behavior | 2.1-2.5, 2.10 |
| `tools/reference-host/test/linux-dev-control-source.test.js` | Asserts Linux dev control routes through `zig_core_bridge_*` | 2.2, 2.10 |

### Live Native Host References

| Path | Live dependency | Planned phase |
|---|---|---|
| `native/android/app/build.gradle.kts` | Builds `libzig_core.so` through Zig | 2.3 |
| `native/android/app/src/main/cpp/CMakeLists.txt` | Builds JNI wrapper named around `zig_core` | 2.3 |
| `native/android/app/src/main/cpp/zig_core_jni.cpp` | `dlopen("libzig_core.so")`, `dlsym("core_step_json")` | 2.3 |
| `native/android/app/src/main/java/com/terrane/platform/ZigCoreBridge.kt` | Kotlin bridge for `core_step_json` | 2.3 |
| `native/ios/Sources/CZigCoreBridge/*` | C shim resolving `core_step_json` | 2.1 |
| `native/ios/Sources/TerraneHostIOS/ZigCoreBridge.swift` | Swift bridge and fallback `libzig_core.dylib` paths | 2.1 |
| `native/linux/meson.build` | Builds `zig_core_bridge.c` | 2.2 |
| `native/linux/src/zig_core_bridge.c/.h` | Loads `libzig_core.so`, calls `core_step_json` | 2.2 |
| `native/linux/src/dev_control_plane.c` | Replay/control paths call `zig_core_bridge_*` | 2.2 |
| `native/linux/src/web_bridge.c/.h` | Bridge surface references `zig_core` paths | 2.2 |
| `native/macos/Sources/CZigCoreBridge/*` | C shim resolving `core_step_json` | 2.5 |
| `native/macos/Sources/TerraneHostMac/ZigCoreBridge.swift` | Swift bridge and fallback `libzig_core.dylib` paths | 2.5 |
| `native/macos/Sources/CZigCrdtBridge/*` | Loads `libzig_crdt`, calls `crdt_*` ABI | 1.2, 2.5 |
| `native/macos/Sources/TerraneHostMac/ZigCrdtBridge.swift` | Swift direct CRDT bridge | 1.2, 2.5 |
| `native/macos/Tests/TerraneHostMacTests/NativeHostTests.swift` | Test environment expects Zig core symbols | 2.5 |
| `native/windows/CMakeLists.txt` | Legacy Windows `zig_core` bridge path | 2.4 |
| `native/windows/src/ZigCoreBridge.cpp` | Loads `zig_core.dll`, calls `core_step_json` | 2.4 |

Top-level `windows/` is already a Forge consumer:

```text
windows/src/Forge.Core/NativeMethods.cs -> forge_core_open / forge_core_handle_command / forge_core_drain_events / forge_core_last_error / forge_core_close
```

### Legacy Implementation Paths

| Path | Live dependency | Planned phase |
|---|---|---|
| `zig-core/*` | Legacy core implementation and ABI | 3.1 after gates |
| `zig-crdt/*` | Legacy CRDT implementation and ABI | 3.2 after gates |
| `server/*` | Legacy HTTP `/bridge`, `/control`, and sync implementation | 3.3 after gates |

### Docs, Task, and Historical Hits

These are not deletion blockers by themselves, but they must be updated or
reclassified as archived references before Phase 4 zero-live-reference closure:

- `README.md`
- `CONTRIBUTION.md`
- `IMPLEMENTATION_STATUS.md`
- `windom-plan.md`
- `codex/PLATFORM_BOOTSTRAP_TASKS.md`
- `docs/00_PRD.md`
- `docs/00_V1_PIVOT.md`
- `docs/01_ARCHITECTURE.md`
- `docs/02_PROJECT_STRUCTURE.md`
- `docs/05_NATIVE_PLATFORM_REQUIREMENTS.md`
- `docs/06_ZIG_CORE_SPEC.md`
- `docs/08_TEST_PLAN.md`
- `docs/09_CODEX_IMPLEMENTATION_PLAN.md`
- `docs/10_ACCEPTANCE_CHECKLIST.md`
- `docs/12_RELEASE_AND_CI.md`
- `docs/32_REFERENCE_HOST_SPEC.md`
- `docs/33_CRDT_COLLAB_NOTEBOOK_PRD.md`
- `docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md`
- Native README files
- Legacy README files
- `task-jun-15/*`
- `review-from-claude/PROMPT.md`

## Forge Baseline Gate

Commands run from `forge/`:

```text
cargo test --workspace --locked
```

Result: passed.

```text
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Result: passed.

```text
cargo run -p forge-cli -- demo
```

Result: passed; output included `REPLAY IDENTICAL: true`.

## Phase 1 Blockers Confirmed

`forge-ffi` currently exports:

- `forge_core_open`
- `forge_core_open_in_memory`
- `forge_core_handle_command`
- `forge_core_drain_events`
- `forge_core_last_error`
- `forge_core_close`
- `forge_string_free`

Immediate blockers before deletion or host cutover:

- The Forge ABI is not a drop-in replacement for the legacy `core_step_json` ABI.
- `forge-ffi` currently has no checked-in C header and builds as `cdylib` + `rlib`; host cutover needs explicit static/dynamic artifact decisions.
- There is no `forge_crdt_*` C ABI.
- `WorkspaceCore::sync_with` is in-process only and not exported through FFI or a command.
- There is no `forge/crates/server`; the Zig server's active `/bridge`, `/control`, and sync surfaces still need a Forge replacement.
- All five legacy native host paths still contain load-bearing Zig references.
- Packaging, repo checks, reference-host tests, and CI/release workflows still assert and build Zig artifacts.

## Forge Server Replacement Scope

`server/` is still load-bearing. Active consumers include:

- `POST /bridge` for channel-derived app id/mount token requests, strict request
  shape validation, permission/budget/runtime checks, bridge-call audit,
  `core.step`, runtime capabilities, storage, mock-backed network/dialogs,
  notification, app log, and notebook sync.
- `POST /control/command` and `/db/*`/`/control/db/*` routes for package
  validation/signing/install/rollback/quarantine, runtime controls, assertions,
  smoke/microtests, replay, snapshots, faults/mocks, fixed DB queries, backup
  import/export, debug bundles, and audit/control-session tables.
- Legacy notebook sync through `/bridge` via `notebook.sync_pull` and
  `notebook.sync_push`, gated by `notebook.sync`.
- Reference-host tests that spawn/build the Zig server and call `/bridge` and
  `/control/command`.
- `runtime-web` tests that still model host traffic as `fetch("/bridge")`.

Minimum Forge parity before deleting `server/`:

1. Add a Forge server crate/binary or embedded loopback artifact, not just
   in-process `forge-sync`.
2. Provide a `/bridge` replacement or explicit new endpoint consumed by the
   surviving runtime/native tests, with equivalent authentication, request
   validation, policy, budget, audit, and mock behavior.
3. Provide `/control` parity or formally retire every current consumer.
4. Implement the v1 sync-server path from `prd-merged/03-sync-server-prd.md`
   far enough to replace active legacy notebook sync consumers.
5. Repoint `package-release`, `check-repo`, CI/release workflows, and
   reference-host tests in lockstep.
6. Re-run the zero-reference grep for `server/src/main.zig`, `--build-server`,
   `terrane-server`, and Zig server-specific test assumptions before deletion.

## Deletion Status

No legacy component is eligible for deletion after this Phase 0 slice. The gates
for `zig-core/`, `zig-crdt/`, and `server/` remain blocked by the live references
and replacement-surface gaps listed above.
