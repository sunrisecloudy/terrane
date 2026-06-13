# Building forge-core for Windows + the C# FFI boundary

**Project:** forge (Terrane) · **Shell:** Windows (PRD 06 PS-14, M6) · **Status:** implementable plan v1
**Audience:** an engineer on a real Windows 11 machine who will build the native lib, generate/author the binding, and stand up the C# host.

This is the crux of the Windows port. The WinUI 3 shell is a *thin* renderer + platform services over the existing `forge-core` Rust workspace at `/Users/vehasuwat/Project/terrane/forge/` (PRD 06 §1: "Shell code contains no business logic"). This document covers four things and nothing else:

1. Building `forge-core` as a Windows native DLL (which crate becomes the `cdylib`, x64 + arm64, the C build deps, the `.dll` output).
2. The binding strategy decision (PS-1 / PS-14): UniFFI-C# vs a hand-written stable C-ABI adapter — with a recommendation and a concrete sketch of both sides.
3. Threading: keeping the UI thread non-blocking over the core's synchronous command facade.
4. (De)serialization of `CoreCommand` / `CoreResponse` / `CoreEvent` on the C# side with `System.Text.Json`, anchored to `forge/spec/commands.md` and `forge/spec/errors.md`.
5. A minimal "hello core" walkthrough: `workspace.create` → `applet.install` → `runtime.run`, receiving the `ui.patch` events.

Everything below references the **real** committed contract. The envelope is `forge_domain::CoreCommand` / `CoreResponse` / `CoreEvent` (verified in `forge/crates/domain/src/lib.rs`); the facade is `forge_core::WorkspaceCore::handle` (`forge/crates/core/src/workspace.rs`); the UI wire format is `forge_ui::Node` / `forge_ui::Patch` (`forge/crates/ui/src/{node,patch}.rs`); the `ui.patch` event payload is `{ applet_id, render_index, tree, patches }` (emitted in `cmd_runtime_run`).

---

## 0. Ground truth: what the core already exposes

Before designing the boundary, pin the exact Rust surface the shell must wrap. These are read off the working M0a spine — do not invent a different shape.

### 0.1 The single entry point

`forge_core::WorkspaceCore` owns one workspace (one portable SQLite file, DECISIONS E1). Its facade is **synchronous** and **single-method**:

```rust
// forge/crates/core/src/workspace.rs
impl WorkspaceCore {
    pub fn open(path: impl AsRef<Path>, workspace_id: impl Into<String>) -> Result<Self>;
    pub fn in_memory(workspace_id: impl Into<String>) -> Result<Self>;
    pub fn handle(&mut self, cmd: CoreCommand) -> CoreResponse;   // never panics (CR-A4)
    pub fn events(&self) -> &EventSink;          // in-memory append-only event log
    pub fn events_mut(&mut self) -> &mut EventSink;
}
```

`handle` takes `&mut self`, so the core is **not** internally synchronized — exactly one thread may call it at a time. This drives the threading design in §3 (a single owning worker thread + a command queue).

### 0.2 The command/response/event envelopes (`forge/crates/domain/src/lib.rs`)

```rust
pub struct CoreCommand {
    pub request_id: RequestId,        // #[serde(transparent)] String
    pub actor: ActorContext,          // { actor: ActorId(String), role: Role }
    pub workspace_id: WorkspaceId,    // String
    pub applet_id: Option<AppletId>,  // omitted when None
    pub name: String,                 // e.g. "applet.install", "runtime.run"
    pub payload: serde_json::Value,   // command-specific (see forge/spec/commands.md)
}

pub struct CoreResponse {
    pub request_id: RequestId,
    pub ok: bool,
    pub payload: serde_json::Value,
    pub warnings: Vec<String>,        // omitted when empty
    pub error: Option<CoreError>,     // present iff !ok
}

pub struct CoreEvent {
    pub event_id: EventId,            // "ev_0", "ev_1", ...
    pub applet_id: Option<AppletId>,
    pub kind: String,                 // "run.started" | "ui.patch" | "run.completed" | ...
    pub payload: serde_json::Value,
    pub created_at_logical: LogicalTimestamp,  // u64, monotone
}

#[serde(tag = "kind", content = "detail")]
pub enum CoreError {               // 12 stable variants (forge/spec/errors.md, CR-A4)
    ValidationError(String), PermissionDenied(String), CapabilityRequired(String),
    StorageError(String), SchemaCompatibilityError(String), QueryError(String),
    RuntimeError(String), ResourceLimitExceeded(String), SyncError(String),
    ConflictRequiresUser(String), ProviderError(String), PlatformUnavailable(String),
}
```

`Role` serializes `snake_case`: `owner | maintainer | editor | runner | viewer | auditor | reviewer`.

**Key consequence:** every type that crosses the FFI is already `Serialize + Deserialize`. The narrowest, most stable boundary is therefore **JSON in, JSON out** — we never marshal Rust structs field-by-field across the ABI. The shell speaks the same JSON the CLI harness already speaks.

### 0.3 The events the shell consumes

`runtime.run` emits, in order (`cmd_runtime_run`):

- `run.started` — `{ applet_id, code_hash }`
- `ui.patch` (one per `ctx.ui.render` call) — `{ applet_id, render_index, tree, patches }`
  - `tree` is a `forge_ui::Node` (full tree for this render).
  - `patches` is a `Vec<forge_ui::Patch>` (the diff vs. the previous render).
- `run.completed` or `run.failed` — `{ run_id, ok }`

`applet.install` emits `applet.installed`; `runtime.replay` emits `run.replayed`. The shell renders the **`ui.patch`** stream (PRD 05). On the first render `patches` is a single root `replace` (no previous tree), so a renderer can start from an empty surface and apply patches uniformly.

### 0.4 The UI wire format the renderer must understand (`forge/crates/ui`)

Node (tagged on `"type"`, M0a catalog from `forge/spec/ui-catalog.md`):

```jsonc
{ "type": "Stack", "direction": "v", "children": [ ... ] }
{ "type": "Text",  "text": "Buy milk" }
{ "type": "Button", "label": "Add", "onTap": "notes.add" }
{ "type": "TextField", "value": "", "label": "Title", "onChange": "notes.title" }
{ "type": "List",  "items": [ ... ] }
// Unknown type → renders a labeled fallback box, never an error (UI-6):
{ "type": "FutureWidget", "title": "...", ... }
```

Patch (tagged on `"op"`, `forge/crates/ui/src/patch.rs`), path = index path from root (`[]` root, `[0]` first child, `[0,2]` third child of first child):

```jsonc
{ "op": "replace",     "path": [..], "node": { .. } }
{ "op": "update_text", "path": [..], "value": ".." }
{ "op": "update_prop", "path": [..], "key": "label", "value": ".." }
{ "op": "insert",      "path": [..], "node": { .. } }
{ "op": "remove",      "path": [..] }
```

The C# renderer (covered in detail in `02-WINUI-RENDERER.md`) only needs to deserialize these and walk an index-addressed tree. This document delivers them across the boundary; it does not build the renderer.

---

## 1. Building forge-core as a Windows native library

### 1.1 New crate: `forge-ffi` (do **not** widen `forge-core`)

The crate layout in PRD 01 §2 already reserves `crates/ffi/` ("UniFFI / C-ABI / wasm-bindgen exports — generated, thin"). The forge workspace doesn't have it yet (members today: domain, storage, crdt, schema, policy, runtime, pipeline, ui, core, cli, testkit). **Add it.** Rationale:

- `forge-core` must stay a clean Rust library (consumed by `forge-cli`, the testkit, future server). Giving it a `crate-type = ["cdylib"]` and `extern "C"` symbols would pollute every downstream consumer's build and pull `libc`/C-string concerns into the business-logic crate. PS-14's "no business logic in the shell" has a mirror rule: **no shell-glue in the core**.
- A dedicated `forge-ffi` `cdylib` is the *only* place the C ABI lives. It depends on `forge-core` + `forge-domain` + `forge-ui` and exposes a tiny C surface. This matches PRD 01's "generated, thin" ffi crate intent.

Create `forge/crates/ffi/Cargo.toml`:

```toml
[package]
name = "forge-ffi"
version.workspace = true
edition.workspace = true
license.workspace = true

[lib]
name = "forge_ffi"
# cdylib  -> the forge_ffi.dll the C# host P/Invokes.
# staticlib -> optional, for a fully-static MSIX bundle if we ever want it.
crate-type = ["cdylib", "staticlib"]

[dependencies]
forge-core.workspace = true
forge-domain.workspace = true
forge-ui.workspace = true
serde.workspace = true
serde_json.workspace = true

[target.'cfg(windows)'.dependencies]
# Windows error-message niceties only; not required for the ABI itself.
```

Add `"crates/ffi"` to the `members` list in `forge/Cargo.toml` and add `forge-ffi = { path = "crates/ffi" }` under `[workspace.dependencies]` (mirroring the existing pattern).

### 1.2 The C build dependencies you must satisfy on Windows

Two transitive deps compile C and need a C toolchain present:

| Dep | Version (pinned in repo) | What it compiles | Windows requirement |
|---|---|---|---|
| `rusqlite` (in `forge-storage`) | `0.40.1`, feature `bundled` | bundled SQLite amalgamation (`libsqlite3-sys`) | a C compiler reachable by `cc`: **MSVC** `cl.exe` (recommended) on the `*-msvc` target. |
| `rquickjs` (in `forge-runtime`, native only) | `0.12` | QuickJS C sources | a C compiler. QuickJS upstream is cleanest with **clang**; on the MSVC target, `rquickjs`'s build uses `cc` → MSVC. If MSVC chokes on a QuickJS C construct, install LLVM and set `CC=clang-cl`. |

The repo already gates `rquickjs` to `cfg(not(target_arch = "wasm32"))` (verified in `forge/crates/runtime/Cargo.toml`), so the native Windows build includes it — good, we want native QuickJS in the shell (PS-14: "QuickJS engine").

**Toolchain to install on the build machine:**

1. **Visual Studio 2022 Build Tools** with the workload "Desktop development with C++". This gives `cl.exe`, the Windows SDK, and the ARM64 cross tools. Select these individual components:
   - MSVC v143 - VS 2022 C++ x64/x86 build tools
   - MSVC v143 - VS 2022 C++ ARM64 build tools
   - Windows 11 SDK (10.0.22621 or newer)
   - C++ CMake tools for Windows (some C deps probe for it)
2. **Rust** via `rustup` (the repo pins `stable` ≥ 1.93 in `rust-toolchain.toml`; "built on 1.96.0"). Then add targets:
   ```powershell
   rustup target add x86_64-pc-windows-msvc
   rustup target add aarch64-pc-windows-msvc
   ```
3. **LLVM/clang (optional but recommended)** — `winget install LLVM.LLVM`. Needed only if MSVC fails on QuickJS C, or if you prefer `clang-cl`. If used:
   ```powershell
   $env:CC="clang-cl"; $env:CXX="clang-cl"
   ```
4. **cargo-c is NOT required** for the C-ABI approach (we hand-roll the header). It *is* convenient if you later want a pkg-config'd cdylib; out of scope here.

> Choose the **`-msvc`** targets, not `-gnu`. The C# host links against the system CRT via MSVC; mixing the GNU CRT (`-gnu`) with .NET on Windows invites CRT mismatches and is unsupported for MSIX. Stay all-MSVC.

### 1.3 Build commands (x64 + arm64)

Run from a "Developer PowerShell for VS 2022" (so `cl.exe` / `link.exe` are on PATH), at the forge workspace root:

```powershell
# x64 release DLL
cargo build -p forge-ffi --release --target x86_64-pc-windows-msvc
#   -> target\x86_64-pc-windows-msvc\release\forge_ffi.dll
#      target\x86_64-pc-windows-msvc\release\forge_ffi.dll.lib   (import lib)
#      target\x86_64-pc-windows-msvc\release\forge_ffi.pdb       (symbols)

# arm64 release DLL (cross-compiles on an x64 host using the ARM64 MSVC tools)
cargo build -p forge-ffi --release --target aarch64-pc-windows-msvc
#   -> target\aarch64-pc-windows-msvc\release\forge_ffi.dll
```

Optional: emit `.def`/symbol verification to confirm the C exports are present:

```powershell
dumpbin /exports target\x86_64-pc-windows-msvc\release\forge_ffi.dll | Select-String forge_
# Expect: forge_open, forge_handle, forge_drain_events, forge_set_event_callback,
#         forge_string_free, forge_close, forge_last_error
```

**Binary size gate (PRD 01 §8: core binary < 12 MB native).** Check with:

```powershell
(Get-Item target\x86_64-pc-windows-msvc\release\forge_ffi.dll).Length / 1MB
```

If over budget, add to `forge/Cargo.toml`:

```toml
[profile.release]
opt-level = "z"   # or "s"
lto = "thin"
codegen-units = 1
strip = "symbols" # keep a separate .pdb for crash reporting
panic = "abort"   # smaller; safe because the ABI catches panics (§2.4) before unwinding the FFI frame
```

> Note on `panic = "abort"`: the FFI functions still wrap the core call in `catch_unwind` (§2.4). With `panic = "abort"` a panic aborts the process rather than unwinding, which is *also* acceptable (the core is built to never panic on real paths per CR-A4). If you want graceful degradation instead of abort, drop `panic = "abort"` and rely on `catch_unwind`. Pick one and document it; default recommendation: keep `catch_unwind`, do **not** set `panic = "abort"`, accept slightly larger binary.

### 1.4 Output layout the C# project consumes

```
forge/target/<triple>/release/forge_ffi.dll      # x64 + arm64 variants
forge/target/<triple>/release/forge_ffi.pdb       # ship to symbol server, not in MSIX
```

The WinUI project references the DLL per-RID (§5.4). The DLL is self-contained: bundled SQLite + bundled QuickJS are statically linked into it. There is **no** external `sqlite3.dll` or `quickjs.dll` to ship.

---

## 2. The binding strategy: hand-written stable C-ABI adapter (recommended) vs UniFFI-C#

### 2.1 The decision

**Recommendation: a hand-written, stable C-ABI adapter in `forge-ffi`, P/Invoked from C#.** Not UniFFI-C#.

PS-1 says "Bindings generated, never hand-written: UniFFI ... C-ABI adapter where generated bindings aren't mature." PS-14 lists "Rust DLL via UniFFI-C#/C-ABI" — explicitly offering both. The deciding factor is the **maturity clause** and the **shape of our surface**:

| Criterion | UniFFI-C# | Hand-written C-ABI (recommended) |
|---|---|---|
| Maturity for C# | UniFFI's first-class targets are Swift/Kotlin/Python. C# is via the third-party `uniffi-bindgen-cs` (community, version-skew risk against the `uniffi` crate). PS-1's escape hatch ("where generated bindings aren't mature") squarely applies. | Stable. `extern "C"` + `DllImport` is the most boring, most supported interop on Windows. |
| Surface size | UniFFI shines when you export *many typed functions/objects*. We export essentially **one** function (`handle(json)->json`) plus lifecycle + a callback. UniFFI's value-add (typed records, enums, error mapping) is redundant — our types are already JSON. | Minimal: 7 C functions total. The "typing" lives in `System.Text.Json` DTOs that mirror the Rust serde types (§4). |
| Wire format | UniFFI would generate C# records mirroring every Rust struct; but our envelopes carry `serde_json::Value` payloads (`CoreCommand.payload`), which UniFFI cannot type — it would surface them as opaque strings anyway. So we'd get JSON-in-a-typed-wrapper. | JSON the whole way; one consistent representation that already matches the CLI harness and the spec docs. |
| Streaming/callbacks | UniFFI callback interfaces for C# are the least-mature part of the immature target. Our event stream is the part we most need to get right. | A plain `extern "C"` function-pointer callback is well-understood from C#/.NET (`[UnmanagedFunctionPointer]` / `GetFunctionPointerForDelegate`). |
| Versioning | Regenerate bindings every core change; bindgen/crate version lockstep. | The C ABI is 7 frozen symbols; the *contract* evolves inside the JSON (CR-A5 capability negotiation), exactly as designed. |

The clinching point: **the core's own contract is already a stable JSON command/event protocol (CR-A1..A5).** The right FFI is a transport for that protocol, not a second type system on top of it. A hand-written C-ABI adapter is the thinnest faithful transport. It also keeps the door open: the same DLL can later add a UniFFI surface for Kotlin/Swift reuse without disturbing the C# host.

> This satisfies PS-1: the binding is hand-written *only at the 7-symbol C-ABI seam*, which is the sanctioned "C-ABI adapter where generated bindings aren't mature" path; the rich, evolving surface (commands/events) remains data, not hand-written code.

### 2.2 The C ABI surface (the whole thing)

Seven symbols. JSON crosses as UTF-8 `char*`. The core handle is an opaque pointer.

```c
// forge_ffi.h  — ship this alongside the DLL; it is the canonical ABI contract.
#include <stdint.h>

typedef struct ForgeCore ForgeCore;   // opaque

// Event callback: invoked (possibly on a worker thread) with one CoreEvent as
// UTF-8 JSON. `user_data` is the pointer passed to forge_set_event_callback.
// The callee must NOT retain `event_json` past the call; copy it.
typedef void (*ForgeEventCallback)(void* user_data, const char* event_json);

// Open (or create) a workspace at `db_path` with logical id `workspace_id`.
// Returns NULL on failure; call forge_last_error() for a UTF-8 JSON CoreError.
ForgeCore* forge_open(const char* db_path_utf8, const char* workspace_id_utf8);

// Register the event sink. Pass NULL to clear. Safe to call before/after open.
void forge_set_event_callback(ForgeCore* core, ForgeEventCallback cb, void* user_data);

// Handle one CoreCommand (UTF-8 JSON) -> a CoreResponse (UTF-8 JSON).
// Always returns a non-NULL, heap-allocated, NUL-terminated UTF-8 string that the
// caller must free with forge_string_free. Never panics across the boundary: a
// caught panic or parse failure is returned as a CoreResponse with ok=false and
// error.kind="RuntimeError"/"ValidationError".
char* forge_handle(ForgeCore* core, const char* request_json_utf8);

// Drain any events the core buffered since the last drain, as a JSON array of
// CoreEvent. Used by the pull path (§3.3). Caller frees with forge_string_free.
char* forge_drain_events(ForgeCore* core);

// Free a string returned by forge_handle / forge_drain_events / forge_last_error.
void forge_string_free(char* s);

// The last error for the current thread as UTF-8 JSON CoreError, or NULL.
char* forge_last_error(void);

// Close the workspace, flush, free the handle. `core` is invalid afterward.
void forge_close(ForgeCore* core);
```

### 2.3 The Rust side (`forge/crates/ffi/src/lib.rs`) — concrete sketch

```rust
//! forge-ffi: the stable C-ABI seam over forge_core::WorkspaceCore.
//! No business logic — pure transport. JSON in, JSON out (CR-A1..A5).

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::Mutex;

use forge_core::WorkspaceCore;
use forge_domain::{CoreCommand, CoreError, CoreEvent, CoreResponse, RequestId};

type EventCb = extern "C" fn(*mut c_void, *const c_char);

/// The opaque handle handed to C#. Holds the core under a Mutex so the C side
/// can call from its single worker thread; the registered callback + the
/// last-drained event cursor live here too.
pub struct ForgeCore {
    core: Mutex<WorkspaceCore>,
    cb: Mutex<Option<(EventCb, usize /* user_data as usize */)>>,
    drained: Mutex<usize>, // index into the EventSink we've already delivered
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<Option<CString>> = const { std::cell::RefCell::new(None) };
}

fn set_last_error(e: &CoreError) {
    let json = serde_json::to_string(e).unwrap_or_else(|_| "{}".into());
    LAST_ERROR.with(|c| *c.borrow_mut() = CString::new(json).ok());
}

/// Convert a Rust String into a heap C string the caller frees via forge_string_free.
fn into_c(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(cs) => cs.into_raw(),
        Err(_) => ptr::null_mut(), // string contained an interior NUL; unreachable for JSON
    }
}

unsafe fn cstr<'a>(p: *const c_char) -> Option<&'a str> {
    if p.is_null() { return None; }
    CStr::from_ptr(p).to_str().ok()
}

#[no_mangle]
pub extern "C" fn forge_open(db_path: *const c_char, workspace_id: *const c_char) -> *mut ForgeCore {
    let result = catch_unwind(|| unsafe {
        let path = cstr(db_path).ok_or_else(|| CoreError::ValidationError("db_path is not UTF-8".into()))?;
        let wsid = cstr(workspace_id).ok_or_else(|| CoreError::ValidationError("workspace_id is not UTF-8".into()))?;
        let core = WorkspaceCore::open(path, wsid)?;
        Ok::<_, CoreError>(Box::into_raw(Box::new(ForgeCore {
            core: Mutex::new(core),
            cb: Mutex::new(None),
            drained: Mutex::new(0),
        })))
    });
    match result {
        Ok(Ok(ptr)) => ptr,
        Ok(Err(e)) => { set_last_error(&e); ptr::null_mut() }
        Err(_) => { set_last_error(&CoreError::RuntimeError("panic in forge_open".into())); ptr::null_mut() }
    }
}

#[no_mangle]
pub extern "C" fn forge_set_event_callback(core: *mut ForgeCore, cb: Option<EventCb>, user_data: *mut c_void) {
    if core.is_null() { return; }
    let h = unsafe { &*core };
    *h.cb.lock().unwrap() = cb.map(|f| (f, user_data as usize));
}

#[no_mangle]
pub extern "C" fn forge_handle(core: *mut ForgeCore, request_json: *const c_char) -> *mut c_char {
    if core.is_null() {
        return into_c(error_response(None, CoreError::RuntimeError("null core handle".into())));
    }
    let h = unsafe { &*core };

    let out = catch_unwind(AssertUnwindSafe(|| {
        let json = match unsafe { cstr(request_json) } {
            Some(s) => s,
            None => return error_response(None, CoreError::ValidationError("request is not UTF-8".into())),
        };
        let cmd: CoreCommand = match serde_json::from_str(json) {
            Ok(c) => c,
            Err(e) => return error_response(None, CoreError::ValidationError(format!("bad command JSON: {e}"))),
        };
        let rid = cmd.request_id.clone();

        // The one real call into the core. handle() never panics on real paths
        // (CR-A4) and returns a CoreResponse for both ok and error.
        let resp: CoreResponse = {
            let mut guard = h.core.lock().unwrap();
            let r = guard.handle(cmd);
            // After the synchronous command, flush newly-emitted events to the
            // registered callback (push path; see §3.2). Drop the lock first so
            // the callback can re-enter forge_drain_events if it wants.
            r
        };

        // Push events emitted by this command.
        deliver_new_events(h);

        serde_json::to_string(&resp)
            .unwrap_or_else(|_| error_response(Some(rid.clone()),
                CoreError::RuntimeError("response serialize failed".into())))
    }));

    match out {
        Ok(s) => into_c(s),
        Err(_) => into_c(error_response(None, CoreError::RuntimeError("panic in forge_handle".into()))),
    }
}

/// Deliver events the EventSink has accumulated past our cursor to the callback.
fn deliver_new_events(h: &ForgeCore) {
    let cb = *h.cb.lock().unwrap();
    let Some((cb, user_data)) = cb else { return; };
    // Snapshot new events while holding the core lock, then call out WITHOUT it.
    let new_events: Vec<CoreEvent> = {
        let guard = h.core.lock().unwrap();
        let all = guard.events().events();
        let mut cursor = h.drained.lock().unwrap();
        let from = *cursor;
        *cursor = all.len();
        all[from..].to_vec()
    };
    for ev in new_events {
        if let Ok(js) = serde_json::to_string(&ev) {
            if let Ok(c) = CString::new(js) {
                cb(user_data as *mut c_void, c.as_ptr()); // callee copies; we own `c`
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn forge_drain_events(core: *mut ForgeCore) -> *mut c_char {
    if core.is_null() { return into_c("[]".into()); }
    let h = unsafe { &*core };
    let evs: Vec<CoreEvent> = {
        let guard = h.core.lock().unwrap();
        let all = guard.events().events();
        let mut cursor = h.drained.lock().unwrap();
        let from = *cursor;
        *cursor = all.len();
        all[from..].to_vec()
    };
    into_c(serde_json::to_string(&evs).unwrap_or_else(|_| "[]".into()))
}

#[no_mangle]
pub extern "C" fn forge_string_free(s: *mut c_char) {
    if !s.is_null() { unsafe { drop(CString::from_raw(s)); } }
}

#[no_mangle]
pub extern "C" fn forge_last_error() -> *mut c_char {
    LAST_ERROR.with(|c| c.borrow().as_ref().map(|cs| into_c(cs.to_string_lossy().into_owned())))
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn forge_close(core: *mut ForgeCore) {
    if !core.is_null() { unsafe { drop(Box::from_raw(core)); } }
}

fn error_response(rid: Option<RequestId>, err: CoreError) -> String {
    let resp = CoreResponse::err(rid.unwrap_or_else(|| RequestId::new("ffi-unknown")), err);
    serde_json::to_string(&resp).unwrap_or_else(|_| "{\"ok\":false}".into())
}
```

This is the entire native binding. It is "thin" per PRD 01 §2: it does no validation, no policy, no storage — it only marshals JSON and forwards to `WorkspaceCore::handle`.

### 2.4 No panics across the boundary (CR-A4 / F:CR-13)

Every `extern "C"` function wraps its body in `catch_unwind`. A caught panic becomes a `CoreResponse { ok:false, error: RuntimeError(...) }` (for `forge_handle`) or a null return + `forge_last_error` (for `forge_open`). This is mandatory: unwinding across the C ABI is UB. The core is *designed* not to panic on real paths (`errors.md`: "FFI/shell boundaries should return `CoreResponse.error` instead of panicking"), so `catch_unwind` is a backstop, not the primary path.

### 2.5 The C# P/Invoke side (`Forge.Interop/NativeMethods.cs`)

```csharp
using System;
using System.Runtime.InteropServices;

namespace Forge.Interop;

// Callback delegate matching ForgeEventCallback. Must be kept alive (GC pinned)
// for the lifetime of the registration — store it in a field, never inline.
[UnmanagedFunctionPointer(CallingConvention.Cdecl)]
internal delegate void ForgeEventCallback(IntPtr userData, IntPtr eventJsonUtf8);

internal static partial class NativeMethods
{
    private const string Dll = "forge_ffi"; // resolves forge_ffi.dll per-RID

    [LibraryImport(Dll, EntryPoint = "forge_open", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr Open(string dbPath, string workspaceId);

    [LibraryImport(Dll, EntryPoint = "forge_set_event_callback")]
    internal static partial void SetEventCallback(IntPtr core, IntPtr cb, IntPtr userData);

    [LibraryImport(Dll, EntryPoint = "forge_handle", StringMarshalling = StringMarshalling.Utf8)]
    internal static partial IntPtr Handle(IntPtr core, string requestJson);

    [LibraryImport(Dll, EntryPoint = "forge_drain_events")]
    internal static partial IntPtr DrainEvents(IntPtr core);

    [LibraryImport(Dll, EntryPoint = "forge_string_free")]
    internal static partial void StringFree(IntPtr s);

    [LibraryImport(Dll, EntryPoint = "forge_last_error")]
    internal static partial IntPtr LastError();

    [LibraryImport(Dll, EntryPoint = "forge_close")]
    internal static partial void Close(IntPtr core);
}
```

`[LibraryImport]` (source-generated P/Invoke, .NET 7+) is preferred over legacy `[DllImport]`: it's AOT-friendly (matters for MSIX self-contained ARM64) and generates the UTF-8 marshalling. Note the native ABI uses the C default calling convention; on x64/arm64 Windows there is effectively one convention, so `Cdecl` on the callback delegate is correct and `[LibraryImport]` needs no `CallConv` attribute.

A safe wrapper that owns the native string lifetime:

```csharp
internal static string TakeUtf8(IntPtr ptr)   // consumes a forge-owned string
{
    if (ptr == IntPtr.Zero) return string.Empty;
    try { return Marshal.PtrToStringUTF8(ptr) ?? string.Empty; }
    finally { NativeMethods.StringFree(ptr); }
}
```

The callback marshals the incoming `const char*` *without* freeing it (the Rust side owns and frees that buffer after the call returns):

```csharp
private void OnNativeEvent(IntPtr userData, IntPtr eventJsonUtf8)
{
    var json = Marshal.PtrToStringUTF8(eventJsonUtf8) ?? "{}"; // copy out; do NOT free
    _eventChannel.Writer.TryWrite(json);                       // hand to the UI side (§3)
}
```

---

## 3. Threading: never block the UI thread

### 3.1 The constraint

- `WorkspaceCore::handle` is synchronous and `&mut self`. A `runtime.run` can take tens of milliseconds (QuickJS + SQLite). Calling it on the WinUI dispatcher thread would jank the UI.
- The core is not internally synchronized; exactly one thread may call `handle` at a time. The `Mutex<WorkspaceCore>` in `ForgeCore` enforces this even if C# misbehaves, but the *design* is: **one owning background thread + a serialized command queue.**

### 3.2 The model

```
WinUI UI thread ──post CoreCommand──▶ Channel<Work> ──▶ Core worker thread
      ▲                                                      │ (owns the IntPtr core)
      │                                                      │ forge_handle(json) [blocking]
      │◀── DispatcherQueue.TryEnqueue(apply CoreResponse) ───┘
      │
      └◀── Channel<string> events ◀── ForgeEventCallback (called on worker thread)
```

- A single **core worker** owns the native handle and is the only caller of `forge_handle` / `forge_drain_events`. It reads work items from a `System.Threading.Channels.Channel<Work>`.
- Each `Work` is `(CoreCommand cmd, TaskCompletionSource<CoreResponse> tcs)`. The worker dequeues, calls `forge_handle`, completes the `tcs`. Callers `await` the `Task`.
- The event callback fires *on the worker thread* (it runs inside `forge_handle`'s `deliver_new_events`). It writes raw JSON into an event `Channel<string>`. A small pump task reads that channel and marshals each event onto the UI thread via `DispatcherQueue.TryEnqueue` for the renderer.

This guarantees: the UI thread only ever does cheap JSON (de)serialization and tree-patch application; all core work is off-thread; commands are serialized (no concurrent `&mut` into the core).

### 3.3 The C# core client

```csharp
public sealed class ForgeClient : IAsyncDisposable
{
    private readonly IntPtr _core;
    private readonly ForgeEventCallback _callback;     // kept alive (field!) so GC can't collect it
    private readonly Channel<Work> _commands = Channel.CreateUnbounded<Work>(new() { SingleReader = true });
    private readonly Channel<string> _events   = Channel.CreateUnbounded<string>();
    private readonly Task _worker;
    private readonly JsonSerializerOptions _json = ForgeJson.Options; // §4

    private record Work(string RequestJson, TaskCompletionSource<string> Tcs);

    public ChannelReader<string> Events => _events.Reader;   // pump reads this onto the UI thread

    public ForgeClient(string dbPath, string workspaceId)
    {
        _callback = OnNativeEvent;                            // store BEFORE registering
        _core = NativeMethods.Open(dbPath, workspaceId);
        if (_core == IntPtr.Zero)
            throw new ForgeException(ReadLastError());        // surfaces a CoreError

        var fnPtr = Marshal.GetFunctionPointerForDelegate(_callback);
        NativeMethods.SetEventCallback(_core, fnPtr, IntPtr.Zero);

        _worker = Task.Factory.StartNew(RunWorker, TaskCreationOptions.LongRunning).Unwrap();
    }

    public async Task<CoreResponse> SendAsync(CoreCommand cmd, CancellationToken ct = default)
    {
        var requestJson = JsonSerializer.Serialize(cmd, _json);
        var tcs = new TaskCompletionSource<string>(TaskCreationOptions.RunContinuationsAsynchronously);
        await _commands.Writer.WriteAsync(new Work(requestJson, tcs), ct);
        var responseJson = await tcs.Task.WaitAsync(ct);
        return JsonSerializer.Deserialize<CoreResponse>(responseJson, _json)!;
    }

    private async Task RunWorker()
    {
        await foreach (var work in _commands.Reader.ReadAllAsync())
        {
            // forge_handle is blocking; we're on the dedicated worker thread, so that's fine.
            // The native side fires OnNativeEvent (event push) DURING this call.
            var outPtr = NativeMethods.Handle(_core, work.RequestJson);
            work.Tcs.TrySetResult(TakeUtf8(outPtr));
        }
    }

    private void OnNativeEvent(IntPtr userData, IntPtr eventJsonUtf8)
        => _events.Writer.TryWrite(Marshal.PtrToStringUTF8(eventJsonUtf8) ?? "{}");

    private static string ReadLastError() => TakeUtf8(NativeMethods.LastError());

    public async ValueTask DisposeAsync()
    {
        _commands.Writer.TryComplete();
        await _worker;
        NativeMethods.SetEventCallback(_core, IntPtr.Zero, IntPtr.Zero);
        NativeMethods.Close(_core);
        _events.Writer.TryComplete();
    }
}
```

Critical lifetime rules baked in above:

- `_callback` is a field → the delegate is not GC-collected while registered (a classic interop crash).
- `SetEventCallback(_core, IntPtr.Zero, …)` is cleared **before** `Close`, so no event can fire into freed state during teardown.
- The worker is `LongRunning` (its own thread), so blocking `forge_handle` never starves the thread pool.

### 3.4 UI-thread event pump (in the renderer host)

```csharp
// Started once, on the UI thread, after constructing ForgeClient.
async Task PumpEvents(ForgeClient client, DispatcherQueue ui, IRenderHost host)
{
    await foreach (var json in client.Events.ReadAllAsync())
    {
        var ev = JsonSerializer.Deserialize<CoreEvent>(json, ForgeJson.Options)!;
        if (ev.Kind == "ui.patch")
        {
            var p = ev.Payload.Deserialize<UiPatchPayload>(ForgeJson.Options)!;
            ui.TryEnqueue(() => host.ApplyPatches(p.AppletId, p.Tree, p.Patches)); // renderer (doc 02)
        }
        else if (ev.Kind is "run.started" or "run.completed" or "run.failed")
        {
            ui.TryEnqueue(() => host.UpdateRunStatus(ev));   // status chrome
        }
        // logs etc. routed to the diagnostics screen (PS-2)
    }
}
```

The push callback (§3.2) keeps latency low; `forge_drain_events` is the **pull fallback** — used at startup to flush anything emitted before the callback was registered, or in tests. Both share the `drained` cursor in Rust so events are delivered exactly once.

---

## 4. (De)serialization with `System.Text.Json`

The C# DTOs mirror the Rust serde types exactly. Anchor names to `forge/spec/commands.md` (command names + payloads) and `forge/spec/errors.md` (the 12 error kinds). Configure one shared `JsonSerializerOptions`.

### 4.1 Options (`Forge.Interop/ForgeJson.cs`)

```csharp
public static class ForgeJson
{
    public static readonly JsonSerializerOptions Options = new()
    {
        PropertyNamingPolicy = JsonNamingPolicy.SnakeCaseLower, // request_id, workspace_id, created_at_logical
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull,
        Converters = { new CoreErrorConverter(), new JsonStringEnumConverter(JsonNamingPolicy.SnakeCaseLower) },
    };
}
```

`JsonNamingPolicy.SnakeCaseLower` (.NET 8) matches Rust's field names (which are already snake_case). `Role` is a `snake_case` enum → `JsonStringEnumConverter` with the snake policy. The newtype IDs (`RequestId`, `WorkspaceId`, …) are `#[serde(transparent)]` over `String` → C# can model them as plain `string`.

### 4.2 DTOs

```csharp
public sealed record CoreCommand
{
    public required string RequestId { get; init; }
    public required ActorContext Actor { get; init; }
    public required string WorkspaceId { get; init; }
    public string? AppletId { get; init; }                 // omitted when null
    public required string Name { get; init; }             // "applet.install" etc. (spec/commands.md)
    public JsonElement Payload { get; init; }              // command-specific; opaque here
}

public sealed record ActorContext
{
    public required string Actor { get; init; }
    public required Role Role { get; init; }
}

public enum Role { Owner, Maintainer, Editor, Runner, Viewer, Auditor, Reviewer }

public sealed record CoreResponse
{
    public required string RequestId { get; init; }
    public required bool Ok { get; init; }
    public JsonElement Payload { get; init; }
    public List<string> Warnings { get; init; } = new();
    public CoreError? Error { get; init; }                 // present iff !Ok
}

public sealed record CoreEvent
{
    public required string EventId { get; init; }
    public string? AppletId { get; init; }
    public required string Kind { get; init; }             // "ui.patch", "run.completed", ...
    public JsonElement Payload { get; init; }
    public required ulong CreatedAtLogical { get; init; }
}

// The ui.patch event payload (cmd_runtime_run): { applet_id, render_index, tree, patches }
public sealed record UiPatchPayload
{
    public required string AppletId { get; init; }
    public required int RenderIndex { get; init; }
    public required JsonElement Tree { get; init; }        // forge_ui::Node (see doc 02)
    public required JsonElement Patches { get; init; }     // forge_ui::Patch[]
}
```

### 4.3 `CoreError` converter (the one non-trivial mapping)

Rust serializes `CoreError` as `#[serde(tag = "kind", content = "detail")]`, e.g.:

```json
{ "kind": "PermissionDenied", "detail": "Viewer attempts record.put" }
```

The `kind` token is exactly the `.code()` token in `forge/spec/errors.md`. A small converter maps it to a typed C# enum + message:

```csharp
public enum CoreErrorKind {
    ValidationError, PermissionDenied, CapabilityRequired, StorageError,
    SchemaCompatibilityError, QueryError, RuntimeError, ResourceLimitExceeded,
    SyncError, ConflictRequiresUser, ProviderError, PlatformUnavailable
}

public sealed record CoreError(CoreErrorKind Kind, string Detail);

public sealed class CoreErrorConverter : JsonConverter<CoreError>
{
    public override CoreError Read(ref Utf8JsonReader r, Type t, JsonSerializerOptions o)
    {
        using var doc = JsonDocument.ParseValue(ref r);
        var root = doc.RootElement;
        var kind = Enum.Parse<CoreErrorKind>(root.GetProperty("kind").GetString()!); // PascalCase tokens
        var detail = root.TryGetProperty("detail", out var d) ? d.GetString() ?? "" : "";
        return new CoreError(kind, detail);
    }
    public override void Write(Utf8JsonWriter w, CoreError v, JsonSerializerOptions o)
    {
        w.WriteStartObject();
        w.WriteString("kind", v.Kind.ToString());
        w.WriteString("detail", v.Detail);
        w.WriteEndObject();
    }
}
```

The shell maps `CoreErrorKind` to user-facing UX per `errors.md` (e.g. `PermissionDenied`/`CapabilityRequired` → a grant prompt routed to `permission.request_grant`; `ResourceLimitExceeded` → a suspension banner; `PlatformUnavailable` → a "not supported on Windows" notice). The shell **never** invents error semantics — it renders the typed kind.

> Acceptance check for §4: round-trip every variant. A test fixture (`Forge.Interop.Tests`) deserializes one canned JSON per command in `forge/spec/commands.md` and per error in `forge/spec/errors.md`, then re-serializes and asserts equality against the Rust-emitted bytes captured from `forge demo`'s event log. This is the C# analogue of the Rust golden corpus.

---

## 5. "Hello core" walkthrough

Goal: from C#, open a workspace, install the notes-lite applet, run it, and receive the `ui.patch` events — proving the boundary carries the full spine (PRD 01 §10 acceptance loop: install → run → store → UI tree → patch). The payload shapes below are taken verbatim from `forge/crates/cli/src/lib.rs` (`forge demo`) and `forge/crates/core/src/workspace.rs`.

### 5.1 Sequence (each line is one `await client.SendAsync(...)`)

```csharp
var actor = new ActorContext { Actor = "owner@local", Role = Role.Owner };
const string ws = "ws_demo";

// 1) workspace.create — reports identity + base version. (Owner role required.)
var create = await client.SendAsync(new CoreCommand {
    RequestId = "r1", Actor = actor, WorkspaceId = ws,
    Name = "workspace.create",
    Payload = J("""{ "name": "Demo" }"""),
});
// create.Payload -> { "workspace_id": "ws_demo", "root_version": 0 }

// 2) applet.install — manifest + sources (the entrypoint key must match manifest.entrypoint).
//    Source-of-record payload: { manifest, sources: { "<path>": "<ts>" } }  (cmd_applet_install)
var install = await client.SendAsync(new CoreCommand {
    RequestId = "r2", Actor = actor, WorkspaceId = ws, AppletId = "notes-lite",
    Name = "applet.install",
    Payload = J($$"""
    {
      "manifest": {{NotesLiteManifestJson}},
      "sources": { "main.ts": {{JsonEncodedTs}} }
    }
    """),
});
// install.Payload -> { "applet_id": "notes-lite", "version": 1, "code_hash": "sha256:...", "warnings": [] }
// Also emits event: kind="applet.installed"

// 3) runtime.run — execute the entrypoint with input. (Runner/Editor/Maintainer/Owner + caps.)
//    Payload: { input }  (cmd_runtime_run)
var run = await client.SendAsync(new CoreCommand {
    RequestId = "r3", Actor = actor, WorkspaceId = ws, AppletId = "notes-lite",
    Name = "runtime.run",
    Payload = J("""{ "input": { "title": "Buy milk" } }"""),
});
// run.Payload -> { "run_id": "...", "code_hash": "sha256:...", "ok": true,
//                  "result": { "ok": true, "value": { "count": 1 } },
//                  "summary": {...}, "ui_renders": [ <Node>, ... ] }

static JsonElement J(string s) => JsonDocument.Parse(s).RootElement.Clone();
```

`NotesLiteManifestJson` and `JsonEncodedTs` come from the embedded demo applet (`forge/examples/notes-lite/`). The shell will normally read these from a CRDT-backed file in the workspace, but for the hello-core smoke they can be embedded resources, exactly as `forge demo` embeds them via `include_str!`.

### 5.2 The events you must observe (delivered to the pump in §3.4)

During step 3 the core emits, in order:

```jsonc
{ "event_id":"ev_1", "applet_id":"notes-lite", "kind":"run.started",
  "payload": { "applet_id":"notes-lite", "code_hash":"sha256:..." }, "created_at_logical": 2 }

{ "event_id":"ev_2", "applet_id":"notes-lite", "kind":"ui.patch",
  "payload": { "applet_id":"notes-lite", "render_index":0,
    "tree": { "type":"Stack", "direction":"v", "children":[
                { "type":"Text", "text":"Notes" },
                { "type":"List", "items":[ { "type":"Text", "text":"Buy milk" } ] } ] },
    "patches": [ { "op":"replace", "path":[], "node": { /* the tree above */ } } ] },
  "created_at_logical": 3 }

{ "event_id":"ev_3", "applet_id":"notes-lite", "kind":"run.completed",
  "payload": { "run_id":"...", "ok":true }, "created_at_logical": 4 }
```

(Exact node contents depend on the notes-lite applet; the `kind` order and payload *shape* are fixed.) The first `ui.patch` always carries a root `replace` patch (no previous tree), so the renderer initializes from empty and applies patches uniformly thereafter.

### 5.3 Acceptance checks for the walkthrough

1. `install.Ok == true` and `install.Payload.code_hash` starts with `"sha256:"`.
2. `run.Ok == true` and `run.Payload.result.value.count == 1` (matches the Rust integration test in `forge/crates/cli/src/lib.rs`).
3. The pump observes exactly one `run.started`, ≥ 1 `ui.patch`, and one `run.completed` (no `run.failed`), in `created_at_logical` order.
4. Applying the `ui.patch` patches to an empty tree (renderer, doc 02) yields a tree equal to the event's `tree` field — the round-trip property (`apply(diff(old,new)) == new`, UI-1).
5. A negative check: send `runtime.run` with `Actor.Role = Role.Viewer`. Expect `Ok == false`, `Error.Kind == PermissionDenied` (RBAC gate in `WorkspaceCore::handle` → `authorize`), proving the boundary surfaces typed errors and the shell holds no business logic.

### 5.4 Wiring the DLL into the C# project (.csproj)

A WinUI 3 / C# project that consumes the native DLL per-RID. Place the built DLLs under `native\<rid>\`.

```xml
<!-- Forge.App/Forge.App.csproj (WinUI 3, packaged) -->
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <TargetFramework>net8.0-windows10.0.22621.0</TargetFramework>
    <TargetPlatformMinVersion>10.0.19041.0</TargetPlatformMinVersion> <!-- Win10 22H2 floor, PS-14 -->
    <RuntimeIdentifiers>win-x64;win-arm64</RuntimeIdentifiers>
    <UseWinUI>true</UseWinUI>
    <Platforms>x64;ARM64</Platforms>
    <AllowUnsafeBlocks>true</AllowUnsafeBlocks>
    <!-- [LibraryImport] source generator needs LangVersion >= 11 (default on net8) -->
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="Microsoft.WindowsAppSDK" Version="1.6.*" />   <!-- WinUI 3 -->
    <PackageReference Include="Microsoft.Windows.SDK.BuildTools" Version="10.0.*" />
  </ItemGroup>

  <!-- Ship forge_ffi.dll alongside the app, selected by RID. Build these from
       cargo build -p forge-ffi --release --target {x86_64,aarch64}-pc-windows-msvc -->
  <ItemGroup>
    <None Include="..\..\forge\target\x86_64-pc-windows-msvc\release\forge_ffi.dll"
          Condition="'$(RuntimeIdentifier)'=='win-x64'"
          Link="forge_ffi.dll"><CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory></None>
    <None Include="..\..\forge\target\aarch64-pc-windows-msvc\release\forge_ffi.dll"
          Condition="'$(RuntimeIdentifier)'=='win-arm64'"
          Link="forge_ffi.dll"><CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory></None>
  </ItemGroup>
</Project>
```

The `const string Dll = "forge_ffi"` in `NativeMethods` resolves to `forge_ffi.dll` in the app directory; the per-RID `<None>` copy ensures the matching architecture's DLL is present. For MSIX both RIDs are bundled and Windows picks the right one at install (covered in `03-PACKAGING-AND-SERVICES.md`).

### 5.5 End-to-end smoke as a CI gate

A headless console harness (`Forge.SmokeTest`, `net8.0-windows`) that runs §5.1 and asserts §5.3 with no WinUI dependency. This is the Windows analogue of `forge demo` and the per-platform "platform smoke of the demo workspace" gate in PS-4:

```powershell
dotnet run --project Forge.SmokeTest -- --db .\smoke.fdb
# exit 0 iff install.ok && run.ok && events {run.started, ui.patch+, run.completed} observed in order
```

Wire this into the Windows CI job after `cargo build -p forge-ffi --release`. It proves the boundary carries the full spine before any pixel is drawn — the same discipline PRD 01 §10 / PS-4 require.

---

## 6. Build + boundary acceptance checklist (definition of done for this document's scope)

- [ ] `forge/crates/ffi/` exists, is in the workspace `members`, builds `forge_ffi.dll` for `x86_64-pc-windows-msvc` **and** `aarch64-pc-windows-msvc`.
- [ ] `dumpbin /exports forge_ffi.dll` lists the 7 symbols; bundled SQLite + QuickJS are statically linked (no external `sqlite3.dll`/`quickjs.dll`).
- [ ] `forge_ffi.dll` size is within the < 12 MB native budget (PRD 01 §8); a `.pdb` is produced for symbol upload.
- [ ] `forge_handle` never unwinds across the ABI (a forced `panic!` in a test build returns an `ok:false` `RuntimeError` response, not a crash) — CR-A4.
- [ ] C# `ForgeClient` runs all core calls on a dedicated `LongRunning` worker; the UI thread is never blocked; the event callback delegate is held in a field (no GC crash).
- [ ] `System.Text.Json` round-trips `CoreCommand`/`CoreResponse`/`CoreEvent`/`CoreError` against captured Rust bytes for every command in `spec/commands.md` and every error in `spec/errors.md`.
- [ ] `Forge.SmokeTest` performs `workspace.create` → `applet.install` → `runtime.run`, asserts `run.ok`, `result.value.count == 1`, and observes `run.started` / `ui.patch`+ / `run.completed` in logical order; the Viewer-role negative test yields `PermissionDenied`.
- [ ] No business logic added to the C# side: it only (de)serializes JSON, manages threads/lifetimes, and forwards commands — PS-14 / PRD 06 §1.

---

## 7. Cross-references

- Renderer (applying `ui.patch` `tree`/`patches` into WinUI XAML, the M0a catalog Stack/Text/Button/TextField/List, UI-6 fallback, action wiring back to `runtime.run`): **`02-WINUI-RENDERER.md`**.
- Platform services (Credential Manager/DPAPI for `secrets/`, file-picker handles, firewall prompt for server mode), MSIX packaging, signing, auto-update, the `tsgo` sidecar (CR-15): **`03-PACKAGING-AND-SERVICES.md`**.
- The PS-15 Tauri-fallback gate (reuse the web renderer if the WinUI estimate exceeds budget at M3 exit) is a program decision recorded against this build/FFI work; the C-ABI boundary in this document is reused unchanged by a Tauri shell (Tauri's Rust side calls `forge-core` directly; the JSON command/event protocol is identical), so the work here is not wasted under either branch.
