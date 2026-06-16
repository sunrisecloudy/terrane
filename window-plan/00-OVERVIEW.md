# 00 — Overview & Architecture

**Project:** forge (codename; Terrane brand assets are a candidate) — Windows desktop shell
**Document role:** the entry point for the Windows-app implementation plan. Read this first, then the
per-area documents (bindings, renderer, platform services, packaging) referenced in §11.
**Status target:** v1.x fast-follow shell (PRD 00 §7 "Windows desktop app"; roadmap milestone **M6**).
**Audience:** an engineer implementing on a real Windows 11 machine. Everything here is concrete:
exact crates/versions, folder layout, command lines, and acceptance checks.

---

## 1. What we are building

A **native Windows 11 / 10 desktop application** that is a **thin shell** — native UX chrome + a renderer
for the declarative UI-tree protocol + platform services — sitting on top of the **existing, working
`forge-core` Rust workspace** (`~/projects/terrane/forge/` inside WSL).

- **Shell language/UI:** C# + **WinUI 3** (Windows App SDK), packaged as **MSIX**.
- **Engine inside the core:** **QuickJS** (`rquickjs`, native) — already the M0a spine engine. WinUI does
  *not* embed a webview or a separate JS engine; JS runs inside `forge-core`, never in C#.
- **Data:** SQLite (`rusqlite`, `bundled`) inside the core — already working.
- **Boundary:** a generated **C-ABI** surface over `forge-core`, consumed from C# via a small,
  hand-reviewed-but-generated P/Invoke layer (PS-1: *bindings generated, never hand-written*).

The product rule that shapes every decision: **the shell contains no business logic** (PRD 06 line 6,
PRD 00 §8). No C# code may mutate SQLite, CRDT docs, permissions, schema, or runtime state. The only
way the shell affects state is by issuing a versioned `Command` and observing `Event`/`Stream` output
(PRD 01 CR-A1). The binding surface is built so that no other path even exists.

### 1.1 Non-negotiable invariants this shell must preserve

| Invariant | Source | How the Windows shell honors it |
|---|---|---|
| Shell has zero business logic | PRD 06 (thin-shell rule) | C# only marshals `CoreCommand` JSON in, `CoreResponse`/`CoreEvent` JSON out. No SQLite/CRDT/JS in C#. |
| All mutation via commands | PRD 01 CR-A1 | The C-ABI exposes exactly one mutating entry point: `forge_core_handle(command_json) -> response_json`. |
| FFI never panics across the boundary | PRD 01 CR-A4 / CR-13 | Rust FFI wraps every call in `catch_unwind`, maps panics → `RuntimeError` `CoreResponse`. |
| Errors are typed + stable | PRD 01 CR-A4 | The 12 `CoreError` variants cross the boundary verbatim as `{kind, detail}` JSON; C# maps to typed exceptions. |
| Forward compatibility | PRD 01 CR-A5, UI-6 | Unknown commands rejected gracefully; unknown UI components render as labeled fallback (already in core). |
| Deterministic replay parity | PRD 01 CR-12 acceptance | Same workspace file + same run replays byte-identically on Windows as on macOS/Linux. |

---

## 2. Architecture diagram

```
┌────────────────────────────────────────────────────────────────────────────┐
│  WINDOWS SHELL  (C# / WinUI 3 — thin, no business logic)                     │
│                                                                              │
│   App.xaml / MainWindow ── NavigationView chrome (workspaces, applets,       │
│        editor, data browser, permissions, settings, diagnostics)  [UI-15..21]│
│                                                                              │
│   ┌──────────────────────┐      ┌─────────────────────────────────────────┐ │
│   │ Applet host surface   │      │ Platform services (C#)                   │ │
│   │  WinUI renderer:       │      │  • Credential Manager + DPAPI secrets    │ │
│   │  UI tree → XAML view-   │      │  • File pickers → opaque handles         │ │
│   │  model graph; patches   │      │  • Notifications (App SDK)               │ │
│   │  → in-place XAML edits   │      │  • forge:// deep links (AppActivation)   │ │
│   │  ItemsRepeater virt.     │      │  • Firewall prompt (server mode)         │ │
│   └──────────┬────────────┘      └────────────────┬─────────────────────────┘ │
│              │  CoreCommand (JSON)                 │  capability callbacks      │
│              ▼  CoreResponse / Event / Stream (JSON)▼                          │
│   ┌──────────────────────────────────────────────────────────────────────┐   │
│   │  Forge.Interop  (C#)  — generated P/Invoke over the C-ABI             │   │
│   │  forge_core_open / _handle / _next_event / _free  (UTF-8 JSON in/out) │   │
│   └──────────────────────────────────┬───────────────────────────────────┘   │
└──────────────────────────────────────┼───────────────────────────────────────┘
                                        │  C ABI  (cdecl, UTF-8 *const c_char)
┌───────────────────────────────────────▼──────────────────────────────────────┐
│  forge_core_ffi.dll   (Rust, cdylib — NEW crate: crates/ffi-cabi)            │
│   thin: JSON ⇄ CoreCommand/CoreResponse · catch_unwind · event ring buffer   │
├──────────────────────────────────────────────────────────────────────────────┤
│  forge-core  (EXISTING, WORKING)  WorkspaceCore::handle(CoreCommand)          │
│    domain · storage(rusqlite bundled) · crdt(loro) · schema · policy          │
│    runtime(rquickjs ── QuickJS realm) · pipeline(swc) · ui(tree+diff/patch)   │
│                                                                              │
│    applet.install → SWC transpile + policy scan → store                      │
│    runtime.run    → QuickJS ctx capability gate → SQLite write → UI patch     │
│    runtime.replay → deterministic re-execution (byte-identical)              │
└──────────────────────────────────────────────────────────────────────────────┘
```

**Read the diagram top-to-bottom:** WinUI shell → `Forge.Interop` (P/Invoke) → `forge_core_ffi.dll`
(thin JSON marshaller) → `forge-core` (`WorkspaceCore::handle`) → SQLite/QuickJS. Each downward arrow is a
**command**; each upward arrow is a **response, event, or stream patch**. The two heavy boxes
(`forge_core_ffi.dll` + `forge-core`) are Rust and are byte-for-byte the same logic that runs today on
macOS/Linux; only the thin top layer is new Windows code.

---

## 3. How this maps to the working headless spine

The M0a spine runs **today**, headlessly, on macOS/Linux. Walk the existing entry points:

- `forge-cli` runs `forge demo`, which calls `forge_cli::run_demo(input)`
  (`~/projects/terrane/forge/crates/cli/src/lib.rs`). That function drives the whole jewel:
  install `notes-lite` → run it → capture UI trees + stored records → replay and assert byte-identity.
- `run_demo` does this **only** through `WorkspaceCore::handle(CoreCommand)`
  (`crates/core/src/workspace.rs`). The CLI is, in PRD language, *"a shell like any other"* (PRD 01 §1).
- `WorkspaceCore::handle` already dispatches the M0a command set:
  `workspace.create/open`, `applet.install`, `runtime.run`, `runtime.replay`, `query.execute`
  (see the `match cmd.name.as_str()` block in `workspace.rs`), each returning a `CoreResponse` and
  emitting `CoreEvent`s (`run.started`, `ui.patch`, `run.completed`, …) through the `EventSink`.

**The Windows shell replaces the CLI as the client of `WorkspaceCore::handle`.** Nothing below the
facade changes. The mapping is one-to-one:

| What the CLI does today | What the Windows shell does |
|---|---|
| Builds a `CoreCommand` in Rust and calls `core.handle(cmd)` | Builds the **same** `CoreCommand` as JSON in C#, sends it through `forge_core_handle` |
| Reads the `CoreResponse` struct | Deserializes the **same** `CoreResponse` JSON (`{ok, payload, warnings, error}`) |
| Inspects `EventSink` for `ui.patch` events | Drains `ui.patch` events from the FFI event ring and feeds them to the WinUI renderer |
| Asserts golden UI trees in tests | Renders the UI trees as live XAML; conformance kit (UI-14) asserts parity |
| Replays and checks `replays_identically` | Surfaces replay in the Debug panel (UI-21); same byte-identity guarantee |

Because `CoreCommand`, `CoreResponse`, `CoreEvent`, and `CoreError` are all `serde`-serializable types
already (`crates/domain/src/lib.rs`), the boundary is **JSON over a C string** — no schema is invented
for Windows; the wire shape is the existing envelope.

**Concrete envelope shapes the C# side will produce/consume** (from `crates/domain/src/lib.rs`):

```jsonc
// CoreCommand (C# → core)
{
  "request_id": "req-0001",
  "actor":   { "actor": "local-owner", "role": "owner" },
  "workspace_id": "ws-default",
  "applet_id": "notes-lite",            // optional
  "name": "runtime.run",
  "payload": { "input": { "title": "Buy milk" } }
}

// CoreResponse (core → C#)
{ "request_id": "req-0001", "ok": true, "payload": { "run_id": "...", "ui_renders": [ /* tree */ ] } }

// CoreError on failure (the 12 stable variants)
{ "request_id": "req-0001", "ok": false, "error": { "kind": "PermissionDenied", "detail": "actor role Viewer ..." } }
```

---

## 4. The thin-shell rule, made enforceable

"No business logic in C#" is not a guideline here — it is enforced by the **binding surface**:

1. **One mutating entry point.** `forge_core_ffi.dll` exports exactly one state-changing function,
   `forge_core_handle(handle, command_json) -> response_json`. There is no exported function that writes
   SQLite, edits a CRDT doc, evaluates JS, grants a permission, or applies a schema change directly.
   C# *cannot* reach those paths because the symbols are not in the DLL.
2. **The core already guards itself.** `WorkspaceCore::handle` runs the **command-level RBAC gate**
   (`authorize()` in `workspace.rs`) before dispatch, and the **per-call capability gate** inside the
   QuickJS `HostContext` at `ctx.*` call time. The shell cannot bypass either; it only chooses which
   command to send and which actor/role to claim (and even the role is validated server-side in the core).
3. **Review gate.** Any PR adding a C# method that performs domain logic (validation, diffing, hashing,
   storage shaping) instead of delegating to a command is rejected. The renderer is the one allowed piece
   of "logic," and it is *presentation only* — it maps an already-diffed patch stream to XAML; it never
   computes diffs (the core's `forge-ui` crate does that).

**Litmus test for any proposed C# code:** *"If I deleted this and issued a command instead, would the
behavior be identical?"* If yes, it must be a command. The only C# that survives this test is: rendering,
native chrome, OS platform services (secrets/files/notifications/firewall/deep links), and event plumbing.

---

## 5. v1 scope for Windows (PS-14 / PS-15)

**In scope (PS-14):**

- C#/WinUI 3 desktop app over `forge-core` via the C-ABI.
- QuickJS engine (already in core; no work beyond building it for Windows).
- Renderer for the ~26-component UI catalog (PRD 05 UI-2) with `ItemsRepeater` virtualization (UI-5).
- Platform app surfaces UI-15..21 (editor, schema designer, data browser, permission UX, time travel,
  LLM panel, debug panel) — shell-native chrome.
- Platform services PS-3: **Credential Manager + DPAPI** secrets, file pickers returning **handles**
  (never raw paths to applets), OS permission prompts mapped to capabilities, notifications,
  `forge://` deep links.
- **MSIX-class signed installer + updates.**
- Embedded server mode is **optional** for the Windows shell at v1.x; when enabled, the shell must handle
  the **Windows Defender Firewall prompt** for inbound connections (PS-14). The embedded-server *logic*
  lives in the core's `server` crate (PRD 03) — the shell only toggles it and surfaces status.
- Targets: **Windows 11** and **Windows 10 22H2+**, **x64 + arm64** (PS-14).

**Out of scope for the first Windows release:**

- No business logic in C# (rule, not a deferral).
- No separate JS engine, no embedded WebView for applet UI (PRD 05 §4: WebView component deferred).
- No npm/native/WASM applets (PRD 01 §9, PRD 00 §7 non-goals).
- Marketplace UI inside the Windows app follows the macOS/web feature set; it is not Windows-specific work.

**The Tauri fallback gate (PS-15 / PRD 00 risk table):** if, at the **M3 exit checkpoint**, the WinUI
estimate exceeds budget (proposal in PRD 06 open-questions: **> 2 engineer-quarters**), fall back to a
**Tauri 2** shell that reuses the **web renderer** (M3 lineage) over the same WASM/native core, instead of
the native WinUI renderer. This plan assumes the WinUI path is taken; the Tauri fallback is a documented
escape hatch, scoped in the bindings document, not the default.

---

## 6. What already works vs. what this plan builds

| Capability | Status | Where it lives | This plan |
|---|---|---|---|
| TS → SWC transpile + static policy scan | **Works** | `forge-pipeline` (swc) | reuse unchanged |
| QuickJS sandbox, zero ambient caps, `ctx.*` gating | **Works** | `forge-runtime` (rquickjs) | reuse; build for Windows |
| SQLite KV/oplog/records | **Works** | `forge-storage` (rusqlite bundled) | reuse; build for Windows |
| Loro CRDT, dynamic schema | **Works** | `forge-crdt`, `forge-schema` | reuse |
| RBAC command gate + capability gate | **Works** | `forge-policy`, `WorkspaceCore::authorize` | reuse |
| UI component tree + diff/patch + golden fixtures | **Works** | `forge-ui` (+ `crates/ui/tests/golden`) | reuse as the renderer contract |
| Command/Event/Stream facade | **Works** | `forge-core` `WorkspaceCore::handle` | reuse as the shell contract |
| Deterministic record/replay (byte-identical) | **Works** | `forge-runtime` recorder | reuse; surfaced in Debug panel |
| **C-ABI / FFI export of the facade** | **BUILD** | new `crates/ffi-cabi` (cdylib) | §10 + bindings doc |
| **Build `forge-core` as a Windows native DLL (x64+arm64)** | **BUILD** | cargo + clang toolchain | §9 + bindings doc |
| **C# P/Invoke interop layer (`Forge.Interop`)** | **BUILD** | new C# project | bindings doc |
| **WinUI 3 renderer (tree/patch → XAML, ItemsRepeater)** | **BUILD** | new C# project | renderer doc |
| **Platform app surfaces (editor/data/permission/debug UI)** | **BUILD** | WinUI XAML | renderer doc |
| **Platform services (Credential Mgr/DPAPI, pickers, firewall, deep links)** | **BUILD** | C# | platform-services doc |
| **MSIX packaging, signing, auto-update, CI** | **BUILD** | MSIX + GitHub Actions `windows-latest` | packaging doc |

The honest one-line summary: **the brain works; we are building the body and skin for Windows.** The hard,
risky parts are exactly three: (1) build the Rust core as a Windows DLL (C builds for `rquickjs`/`rusqlite`
need clang), (2) expose the command/event/stream API across the C# boundary without leaking a mutation path,
and (3) render the UI-tree protocol natively in WinUI 3 and pass the renderer conformance kit (UI-14).

---

## 7. OS targets & support matrix

| Axis | Target | Source |
|---|---|---|
| Windows 11 | All shipping builds (22000+) | PS-14 |
| Windows 10 | **22H2+** (build 19045+) | PS-14, PRD 00 §13 Q5 |
| CPU architectures | **x64** and **arm64** | PS-14 |
| Runtime dependency | Windows App SDK 1.x (self-contained or framework-dependent — see packaging doc) | PS-14 |
| Cold start to interactive workspace | **< 2 s** (desktop SLO) | PRD 06 §9 acceptance |
| Input → patched frame (renderer) | **< 16 ms p95** | PRD 05 UI-4 |
| 100k-row table | **60 fps** via `ItemsRepeater` virtualization | PRD 05 UI-5 acceptance |

**ARM64 note:** `rquickjs` and `rusqlite` both compile native C; an arm64 build requires the arm64 MSVC +
clang toolchain and the `aarch64-pc-windows-msvc` Rust target. Treat arm64 as a first-class CI target, not an
afterthought (PS-14 lists it explicitly). Details in §9 and the packaging doc.

---

## 8. Conformance gates before this shell ships (PS-4)

A Windows build is not shippable until **all four** gates are green — the same bar every shell clears:

1. **Engine conformance (CR-12):** QuickJS-native passes the covered `conformance-engines` corpus **on Windows**
   (same covered-vector suite that runs on macOS/Linux). No Windows-specific JS divergence in those vectors.
2. **Renderer conformance kit (UI-14):** golden trees + scripted-interaction + screenshot tests pass against
   the WinUI renderer. Behavioral divergence from the reference is **release-blocking**. The golden corpus
   already exists at `crates/ui/tests/golden/` — the WinUI renderer is validated against it.
3. **Data fixtures (DL / PRD 09):** the demo workspace SQLite file produced on macOS opens and renders
   identically on Windows; schema-change fixtures survive.
4. **Platform smoke:** the demo workspace runs end-to-end on Windows — install → run → render → event →
   patch → replay — equivalent to `forge demo` (PRD 06 §9: *"same workspace file opens on every shipped
   platform"*).

---

## 9. Prerequisites (install on the Windows dev machine)

| Tool | Version / channel | Why |
|---|---|---|
| **Visual Studio 2022** (17.8+) | with workloads: *.NET Desktop Development*, *Desktop development with C++*, *Windows App SDK C# Templates* | WinUI 3 tooling, MSVC linker, MSIX packaging |
| **Windows App SDK / WinUI 3** | 1.5+ (1.x line) | the WinUI 3 shell framework (PS-14) |
| **.NET SDK** | 8.0+ (LTS) | C# / WinUI 3 target framework `net8.0-windows10.0.19041.0` |
| **Rust** | **1.96.0** (pinned by `rust-toolchain.toml` → `channel = "stable"`) | builds `forge-core`; note the toolchain comment: *libsqlite3-sys 0.38+ needs `cfg_select!` (stable ≥ 1.93), built on 1.96.0* |
| **Rust targets** | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | x64 + arm64 DLLs |
| **cargo** | bundled with Rust 1.96 | builds the `cdylib` |
| **clang / LLVM** | clang 17+ (via VS "C++ Clang tools" or `winget install LLVM.LLVM`) | **required** to compile the C in `rquickjs` (QuickJS) and `rusqlite` (`bundled` SQLite); set `LIBCLANG_PATH` |
| **Windows SDK** | 10.0.22621+ (22H2) | headers/libs for App SDK + arm64 |
| **MSVC build tools** | x64 + arm64 (`VC++ ARM64 build tools` component) | linking native DLLs for both arches |

Confirm the pin and the C toolchain before anything else:

```powershell
# From the repo root on Windows:
rustup show                                  # expect: stable-x86_64-pc-windows-msvc, rustc 1.96.0
rustup target add aarch64-pc-windows-msvc    # add arm64 if missing
clang --version                              # expect 17+
# rquickjs/rusqlite bindgen needs libclang:
$env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
# Smoke-build the core's native deps (proves clang + MSVC wire up):
cargo build -p forge-storage -p forge-runtime --target x86_64-pc-windows-msvc
```

> `forge-storage` (`rusqlite` `bundled`) and `forge-runtime` (`rquickjs`) are the two crates with C
> dependencies. If both build for `x86_64-pc-windows-msvc`, the Windows native-lib risk is retired — the
> rest of the core is pure Rust. The bindings document covers the `cdylib` build of `crates/ffi-cabi`.

---

## 10. The new FFI crate at a glance (`crates/ffi-cabi`)

This plan adds **one Rust crate** to the existing workspace — a thin `cdylib` that wraps `WorkspaceCore`.
It contains **no business logic** (CR-A4: *FFI exports — generated, thin*, PRD 01 §2 crate `ffi/`). Sketch
(full design in the bindings document):

```rust
// crates/ffi-cabi/src/lib.rs  (cdylib; cdecl; UTF-8 JSON in/out; never panics across the boundary)
use std::ffi::{c_char, CStr, CString};
use std::panic::catch_unwind;
use forge_core::WorkspaceCore;
use forge_domain::{CoreCommand, CoreResponse, RequestId};

/// Open (or create) a workspace file; returns an opaque handle pointer (or null).
#[no_mangle]
pub extern "C" fn forge_core_open(path_utf8: *const c_char, ws_id_utf8: *const c_char) -> *mut WorkspaceCore { /* ... */ }

/// Issue ONE command as JSON; returns a heap CString of the CoreResponse JSON.
/// The ONLY mutating entry point. Wrapped in catch_unwind → never unwinds into C#.
#[no_mangle]
pub extern "C" fn forge_core_handle(core: *mut WorkspaceCore, cmd_json: *const c_char) -> *mut c_char {
    let result = catch_unwind(|| {
        let core = unsafe { &mut *core };
        let json = unsafe { CStr::from_ptr(cmd_json) }.to_str().unwrap_or("");
        let cmd: CoreCommand = serde_json::from_str(json)
            .unwrap_or_else(|e| /* synth a ValidationError command-id-less response */ todo!());
        let resp = core.handle(cmd);                       // <-- the existing facade, unchanged
        serde_json::to_string(&resp).unwrap()
    }).unwrap_or_else(|_| panic_to_response_json());        // map panic → RuntimeError CoreResponse
    CString::new(result).unwrap().into_raw()
}

/// Drain the next queued CoreEvent JSON (ui.patch, run.*) or return null if empty.
#[no_mangle]
pub extern "C" fn forge_core_next_event(core: *mut WorkspaceCore) -> *mut c_char { /* ... */ }

/// Free a string previously returned by this DLL (C# calls this; no cross-allocator frees).
#[no_mangle]
pub extern "C" fn forge_core_free(p: *mut c_char) { if !p.is_null() { unsafe { drop(CString::from_raw(p)); } } }
```

Cargo manifest addition (in the bindings doc, summarized here):

```toml
# crates/ffi-cabi/Cargo.toml
[lib]
crate-type = ["cdylib"]      # → forge_core_ffi.dll
[dependencies]
forge-core = { path = "../core" }
forge-domain = { path = "../domain" }
serde_json = "1"
```

Build for both arches:

```powershell
cargo build -p forge-core-ffi --release --target x86_64-pc-windows-msvc
cargo build -p forge-core-ffi --release --target aarch64-pc-windows-msvc
# → target\<triple>\release\forge_core_ffi.dll  (bundled into the MSIX)
```

---

## 11. How to read the rest of this plan

| # | Document | Covers |
|---|---|---|
| 00 | **this file** | architecture, scope, targets, prerequisites, mapping to the spine |
| — | **Bindings & native lib** | `crates/ffi-cabi` full design, C-ABI contract, `Forge.Interop` P/Invoke, event ring buffer, error marshalling, building x64+arm64 DLLs, Tauri fallback notes |
| — | **WinUI 3 renderer** | UI-tree → XAML view-model graph; patch application; `ItemsRepeater` virtualization (UI-5); the ~26-component catalog mapping (UI-2); fallback for unknown components (UI-6); platform app surfaces (UI-15..21); renderer conformance kit (UI-14) |
| — | **Platform services** | Credential Manager + DPAPI secrets; file pickers → handles; capability permission prompts (UI-18); notifications; `forge://` deep links; embedded-server firewall handling |
| — | **Packaging & CI** | MSIX layout, code signing, auto-update, `windows-latest` GitHub Actions matrix (x64+arm64), conformance gates wired into CI |

---

## 12. Acceptance checks for this overview's claims

The overview is "done" when an implementer can, on a fresh Windows machine, verify each below:

- [ ] **Prereqs install cleanly:** `rustup show` reports stable **1.96.0**; `clang --version` ≥ 17; VS 2022
      has the *Desktop development with C++* and *Windows App SDK* workloads.
- [ ] **Native deps build:** `cargo build -p forge-storage -p forge-runtime --target x86_64-pc-windows-msvc`
      succeeds (proves `rusqlite bundled` + `rquickjs` C builds work with the clang/MSVC toolchain).
- [ ] **ARM64 target present:** `rustup target list --installed` includes `aarch64-pc-windows-msvc`.
- [ ] **The facade is the only client surface:** grepping the (future) C# tree finds calls to
      `forge_core_handle` / `forge_core_next_event` and **no** direct SQLite/JS/CRDT access — the thin-shell
      rule (§4) holds by construction.
- [ ] **Spine parity goal is stated and testable:** the Windows platform smoke (install → run → render →
      replay) is defined to match `forge demo` byte-for-byte on UI trees and replay fingerprint (§3, §8).
- [ ] **Four conformance gates enumerated** (§8) and each maps to an existing artifact (engine suite,
      `crates/ui/tests/golden`, DL fixtures, demo workspace).
- [ ] **Scope is unambiguous:** desktop app yes; embedded server optional with firewall handling; Tauri is a
      documented fallback gated at M3 exit, not the default (§5).

---

**Sections written: 12** (1. What we are building · 2. Architecture diagram · 3. Mapping to the headless
spine · 4. Thin-shell rule made enforceable · 5. v1 scope (PS-14/PS-15) · 6. Works vs. builds · 7. OS
targets & SLOs · 8. Conformance gates · 9. Prerequisites · 10. The new FFI crate · 11. Reading guide ·
12. Acceptance checks).
