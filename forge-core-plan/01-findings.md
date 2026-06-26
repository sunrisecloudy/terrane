# 01 — Findings (the evidence)

This is the audit result that justifies the plan. Every row is backed by file:line citations from
the five shells and the Forge core. Numbers are line-count estimates of the *duplicated logic*
(not total file size).

## A. Duplication inventory (what is reimplemented per platform)

| # | Concern | ~LOC (×platforms) | Platforms | Verdict | Phase |
|--:|---|--:|---|---|---|
| 1 | DevControlPlane command routing + tool dispatch (70–105 tools) | 1700 | all 5 | extract-to-data + core | A4 → B6 |
| 2 | Snapshot create / restore / compare logic | 600 | all 5 | move-to-core | B6 |
| 3 | Static-HTML query / selector / text-extraction + a11y audit | 1800 | mac,iOS,Lin,Win | move-to-core | B6 |
| 4 | Fault-inject / network-mock / dialog-mock / bridge-call / core-replay matchers | 1700 | all 5 | move-to-core | B6 |
| 5 | Backup export/import format + package validate/hash | 1400 | all 5 | move-to-core | B6 |
| 6 | HTTP/TCP listener + request parsing | 800 | all 5 | **stays per-platform** | — |
| 7 | Ed25519 token gen + package sign/verify + key custody | 550 | all 5 | shared crypto seam | E13 |
| 8 | WebBridge envelope parse + dispatch + permission routing | 2825 | all 5 | move-to-core | C10 |
| 9 | PlatformNetwork policy match + private-IP (v4/v6) detection | 3069 | all 5 | move-to-core | C8 |
| 10 | Rate-budget enforcement (bridge/network/log per-minute windows) | 150 | all 5 | move-to-core | C10 |
| 11 | AppSandboxContext manifest parsing (perms/netPolicy/budget/denyPrivate) | 200 | all 5 | move-to-core | C9 |
| 12 | Storage key-prefix (`appId:`) enforcement | 80 | all 5 | move-to-core | C10 |
| 13 | Bridge-call / core-event / runtime-session recording schema + INSERTs | 250 | all 5 | data (schema) + core | A5 → C11 |
| 14 | `isFiniteJSONNumber` + JSON subset-match validation helpers | 200 | mac,iOS,Win,Lin | move-to-core | C10 |
| 15 | SQLite bind helpers + transport (sqlite3 C API / Room) | 280 | all 5 | **stays per-platform** | — |
| 16 | App version install / rollback / activation (raw SQL on `app_versions`) | 800 | all 5 | move-to-core | D12 |
| 17 | Auto-quarantine on 3+ budget errors / 60s + prior-version restore | 310 | macOS only | move-to-core | D12 |
| 18 | Runtime session + crash-recovery state writes | 250 | all 5 | move-to-core | C11 |
| 19 | Engine-room snapshot table enumeration (dev telemetry) | 300 | macOS only | data + stays per-platform | A2 |
| 20 | Bundled-app catalog IDs (6 apps) replicated inline | 70 | all 5 | extract-to-data | A2 |
| 21 | Snapshot-type / app-status / trust-level / MIME / package-whitelist enums | 200 | all 5 | extract-to-data | A2/A3 |
| 22 | HTTP response envelope `{ok,result/error,diagnostics}` formatting | 150 | all 5 | extract-to-data | A4 |
| 23 | Control-plane env var names + signing key account/session-id patterns | 90 | all 5 | extract-to-data | A2 |
| 24 | Per-platform JSON (Codable/GLib/rapidjson/Room) serialization glue | 400 | all 5 | **stays per-platform** | — |

**~18K lines of duplicated logic.** Items 6, 15, 24 (~1.5K) are genuine OS glue and stay. Everything
else is a reuse or data-extraction target.

## B. Drift proof — the copies are already out of sync

The DevControlPlane is one debug/test control surface, copied by hand into five languages. It has
**not** stayed identical. Distinct HTTP control routes per platform:

| Platform | Distinct control routes |
|---|--:|
| macOS (`DevControlPlane.swift`) | **24** |
| iOS (`IOSDevControlPlane.swift`) | **21** |
| Linux (`dev_control_plane.c`) | **11** |
| Windows (`DevControlPlane.cpp`) | **10** |
| Android (`AndroidDevControlPlane.kt`) | **4** |

Same surface (`/apps`, `/rollback`, `/versions`, `/snapshot`, `/engine-room/snapshot`, `/db/*`,
`/command`, …), wildly different coverage. Some of this is intentional product constraint
(iOS/Android can't side-load apps), but much is unmanaged drift. A single declarative
`forge/data/control-commands.json` + a shared core makes the surface uniform and the per-platform
*capability matrix* explicit instead of accidental. (See open question on the capability matrix in
[09](09-decisions-and-open-questions.md).)

## C. Two divergent SQLite schemas (the domain-state split)

There are **two separate databases with two separate schemas**:

- **Forge core** (`forge/crates/storage/src/{store,index}.rs`) owns the workspace DB:
  `kv, records, oplog, runs, run_logs, attachments, audit_log, crdt_chunks, crdt_snapshots, meta`.
- **The shells** hand-maintain a *second* schema in every `PlatformDatabase.{swift,c,cpp,kt}` plus
  `PlatformAppRegistry`: `apps, app_versions, app_installations` (+ debug tables `runtime_snapshots,
  bridge_calls, core_events, runtime_sessions, network_mocks`).

The core already exposes `applet.install / enable / suspend / uninstall / upgrade`, `quota.*`,
`permission.*` — yet the shells run their **own** app-registry with raw SQL `UPDATE
app_versions.status` for rollback / quarantine / activation, with no core command, no audit row, no
atomic guarantee, and no replay. This is exactly the "domain logic at the shell edge" that CLAUDE.md
forbids. Phase D closes it.

`forge/crates/core/src/commands/mod.rs` already models the command catalog as **data** — a static
`name → handler` table. That is the pattern every new command in this plan follows.

## D. Two concrete bugs found in passing

1. **Runtime-version mismatch.** `WebBridge` reports `runtimeVersion = "0.1.0"`
   (`native/macos/.../WebBridge.swift:158`) while `NativeEngineRoomSnapshotProvider` hard-codes
   `"0.4.0"` (`native/macos/.../NativeEngineRoomSnapshotProvider.swift:75`). Two different "runtime
   versions" are reported by the same app. Fix: one build-injected constant in `forge-runtime`,
   surfaced via data/config (see [08-data-files.md](08-data-files.md), `runtime-config.json`).
2. **Unsafe partial-fallback schema.** Each `PlatformDatabase` has a fallback path that creates only
   `apps` + `app_storage` if migration fails (`native/macos/.../PlatformDatabase.swift:55-56` and the
   four siblings). On a migration failure the app runs against a *half-built* schema instead of
   failing loudly. Fix in A5: single authoritative migration set; delete the fallback.

## E. The existing Forge core command surface (51 commands)

What the shells can already delegate to today (from `forge/crates/core/src/commands/` +
`legacy.core_step`):

```
applet.install / enable / suspend / uninstall / upgrade   (+ *.enabled/suspended/... events)
runtime.run / replay / replay_session                     run.save/started/completed/failed/replayed
schema.apply_change / validate_compatibility / rebuild_indexes / changed
query.execute      audit.query      manifest.validate     signature.verify
db.watch / unwatch / history / restore                    permission.grant / revoke (+ events)
quota.set / status / approaching                          net.fetch    network.egress
secret.use (+ used)    ctx.files / secrets / ui / future / timetravel    capabilities.files
required_features.negotiate    session.replayed    workspace.export / import    legacy.core_step
```

**Gaps this plan fills with new commands** (all behind the same JSON seam, no new FFI entry points):

| New command(s) | Fills | Phase |
|---|---|---|
| `bridge.validate_envelope` | WebBridge envelope/permission/budget decision | C10 |
| `bridge.validate_network_request` | private-IP + network-policy decision | C8 |
| `applet.get_manifest`, `applet.get_permissions` | trusted manifest parse (no shell re-parse) | C9 |
| `applet.list_versions / activate_version / rollback_version / set_status` | version history + authority | D12 |
| `quota.auto_quarantine` (+ `quota.status` fields) | budget-error-driven quarantine | D12 |
| `control.*` (debug-gated) routed into `forge-controlcore` | DevControlPlane pure logic | B6 |

## F. What genuinely stays per-platform

Do **not** move these — they are real OS differences:

- HTTP/TCP listeners: `NWListener` (Darwin), `libsoup` (Linux), WinHTTP (Windows), Android HTTP.
- WebView host + dialogs: WKWebView, WebView2, WebKitGTK, Android WebView; `NSAlert`/`UIAlert`/etc.
- SQLite transport (the `sqlite3_*` C API calls / Room) and per-language JSON (Codable / GLib /
  rapidjson / Room) marshalling.
- Key custody: Keychain / Windows CNG / libsecret / Android Keystore.
- OS-level app install/uninstall filesystem operations.

Only the **decision** crosses into the core; the **transport** stays in the shell.
