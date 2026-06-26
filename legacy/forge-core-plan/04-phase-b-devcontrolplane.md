# Phase B — DevControlPlane → shared Rust (steps B6–B7)

**Theme:** the single biggest raw-line win. The DevControlPlane is **28,680 lines reimplemented 5×**
and it is **DEBUG-only test-automation surface**, so it has the *lowest behavioral blast radius* of
any large target. Consolidate its pure algorithm logic into one Rust module behind the existing JSON
seam; keep only HTTP transport and DB I/O in each shell.

**Prerequisite:** Phase A steps A3 (enums), A4 (control-tools catalog + response envelope), A5
(shared schema) — they fix the contract this phase implements against.

**Key technique:** **golden vectors.** Before changing a shell, capture the current Swift/C++/C/Kt
output for a representative set of tool calls. The Rust module must reproduce them byte-for-byte.
Migrate macOS first, prove identical, then fan out.

---

## What moves into `forge-controlcore` (pure, debug-gated)

All of these are "JSON in → JSON out" with no platform dependency; the citations are the macOS
source but each has 3–4 siblings:

| Logic | macOS source | Siblings |
|---|---|---|
| Snapshot create / restore / **compare** (volatile-field stripping, storage-row normalization) | `DevControlPlane.swift:1514-1680` | iOS 1969-2051, Lin 4100-4400, Win 7763-7828, And 1045-1184 |
| Static-HTML query / selector match / text extraction | `DevControlPlane.swift:5770-6020` | iOS 1557-1650, Lin 2242-2350, Win 5000-6000 |
| WCAG accessibility audit | `:912-930, 4950-5020` | iOS 1138-1222, Lin 2838-2960, Win 5800-6000 |
| Smoke-test step matching / validation | `:1350-1383, 5426-5473` | iOS 1368-1435, Lin 2650-2750, Win 7276-7330 |
| Fault-inject matcher | `:883-912, 4850-4900` | iOS 789-837, Lin 686-768, Win 7507-7580, And 520-568 |
| Network-mock matcher (URL pattern) | `:1116-1155, 4504-4530` | iOS 837-896, Lin 769-883, And 568-622 |
| Dialog-mock matcher | `:1155-1177` | iOS 896-928, Lin 883-943, And 622-675 |
| Bridge-call assertion (`jsonMatchesSubset`) | `:1177-1213` | iOS 1850-1877, Lin 2127-2210, Win 7585-7610, And 488-520 |
| Core-event replay / core-action assertion | `:1256-1301` | iOS 1877-1969, Lin 1801-1979, Win 7609-7700, And 373-488 |
| Backup export/import document format | `:2486-2750` | iOS 2592-2954, Lin 3360-3700, Win 7855-8000, And 1231-1400 |
| Package read / validate / hash (SHA256 over files) | `:2750-3000` | Lin 3000-3200, Win 2000-2500 |

**~7,200 LOC of duplicated debug logic** collapses to one Rust implementation.

## What stays in each shell

- HTTP/TCP listener + request parsing (`DevControlPlane.swift:125-194` etc.) — genuine OS glue.
- SQLite I/O: reading `app_storage`/`runtime_snapshots`/`bridge_calls` rows and writing snapshots.
  The shell **fetches rows and passes them as JSON to the core**, then writes back what the core
  returns. Core stays pure; transport stays native.
- Per-language JSON marshalling and the dispatch `switch` itself (now catalog-validated from A4).

---

## B6 — Build `forge-controlcore`, migrate macOS

**Goal:** stand up the module and prove it byte-identical on one platform.

**Do:**
- Add the module — either a new crate `forge/crates/controlcore` or a debug-gated module inside
  `forge-testkit`. **Decide the location** (open question in [09](09-decisions-and-open-questions.md));
  the seam auditor recommends **extending `COMMANDS`** with debug-gated `control.*` commands rather
  than adding a new FFI entry point.
- Implement the pure functions above, taking JSON in / returning JSON out. Reuse `forge-domain`
  types and the A3 enums. Keep it `wasm32`-clean where possible (HTML parsing may need a gate).
- Capture golden vectors from the current macOS Swift output for each tool.
- Migrate macOS `DevControlPlane` to call `forge-controlcore` (via the JSON seam) for these
  operations; keep its HTTP listener + DB I/O. Delete the now-dead Swift algorithm code.

**Validation:** `cargo test -p forge-controlcore` against the golden vectors; a macOS DevControlPlane
integration test asserting identical tool results pre/post; `swift build && swift test`.

**Risk:** medium (debug-only). **App-visible:** the control surface is observed by test harnesses,
not by generated apps — **not** part of the public contract. **Effort:** XL. **Commits:** several
(module scaffold; then one or two per logic group as macOS migrates).

---

## B7 — Fan out to iOS / Linux / Windows / Android

**Goal:** replace the other four copies with calls into `forge-controlcore`.

**Do:** one shell per commit, in order iOS → Linux → Windows → Android. Replace each shell's
snapshot/html/a11y/mock/replay/backup/package code with FFI/JNI/interop calls into the same module;
delete the dead per-platform logic. Respect the A4 capability matrix (iOS/Android have no
install/uninstall and a reduced tool set; Android has no static-HTML analysis — it uses the WebView
bridge).

**Validation:** per-shell control-plane integration tests against the **same** golden vectors; a
cross-platform parity test asserting the same `tool + args` yields the same result envelope on every
platform that implements it. (macOS validated locally; the other shells validated in their CI.)

**Risk:** medium. **App-visible:** no. **Effort:** XL. **Commits:** ~4 (one per shell), each
deleting thousands of lines.

---

## Phase B exit criteria

- One Rust implementation of the DevControlPlane algorithms; ~7.2K LOC of duplicated debug logic
  deleted across the shells.
- Every platform's control surface produces identical results for shared tools (parity test green).
- Per-platform code reduced to HTTP transport + DB I/O + catalog-driven dispatch.
- Public contract unaffected (debug surface is internal); macOS green locally, others green in CI.
