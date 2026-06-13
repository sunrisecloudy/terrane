# 05 — Implementation milestones & acceptance gates

**Project:** forge — Windows desktop shell (codename per `prd-merged/00`)
**Scope of this doc:** an ordered, implementable plan a single developer can execute on a real Windows 11 / Windows 10 22H2+ machine to bring up the **thin** C#/WinUI 3 shell over the existing Rust `forge-core` (`~/projects/terrane/forge` inside WSL).
**Non-negotiable rule (carried from `prd-merged/06` §intro):** the shell contains **no business logic**. It only (a) sends `CoreCommand` and receives `CoreResponse`, (b) subscribes to events/streams, (c) renders the UI tree + patches, and (d) provides platform services the core calls back into. Every state mutation goes through a command (CR-A1). This rule is itself an acceptance gate (see W2/W3).

This plan satisfies the `prd-merged` Windows requirements **PS-1, PS-3, PS-4, PS-14, PS-15** and the UI requirements **UI-1, UI-2, UI-5, UI-6, UI-12, UI-14**, mapping onto roadmap milestone **M6** (`prd-merged/00` §11), and gates the **Tauri fallback decision (PS-15)** at the end of W1.

---

## 0. Conventions used by every milestone

- **Repo layout (new).** The shell lives in a sibling tree to the Rust core, so the core stays untouched:
  ```
  terrane/
    forge/                         # existing Rust core (do not fork)
    windows/                       # NEW — everything in this plan
      ffi/                         # cdylib crate that re-exports forge-core over C-ABI
      bindings/                    # generated C# (UniFFI) + handwritten C-ABI P/Invoke
      Forge.Windows.sln
      src/
        Forge.Core/                # C# class lib: CoreClient, command builders, event pump
        Forge.Renderer/            # C# class lib: UI-tree → WinUI visual tree + patch apply
        Forge.Shell/               # WinUI 3 packaged app (App.xaml, windows, panels)
        Forge.PlatformServices/    # secrets, file pickers, notifications callbacks
      tests/
        Forge.Renderer.Tests/      # golden-fixture + conformance-kit runner (xUnit)
        Forge.Core.Tests/          # hello-core + command round-trip tests
      packaging/                   # MSIX manifest, cert tooling, install scripts
      .github/workflows/windows.yml
  ```
- **Toolchain pins (install once, verify at W0).**
  | Tool | Version | Install command |
  |---|---|---|
  | Rust | 1.96.0 (matches `forge/rust-toolchain.toml`) | `rustup toolchain install 1.96.0` |
  | Rust targets | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` | `rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc` |
  | MSVC Build Tools | VS 2022 17.10+ (Desktop C++, Win11 SDK 10.0.22621+) | `winget install Microsoft.VisualStudio.2022.BuildTools` |
  | .NET SDK | 8.0.x (LTS) | `winget install Microsoft.DotNet.SDK.8` |
  | Windows App SDK | 1.6.x (WinUI 3, stable) | NuGet `Microsoft.WindowsAppSDK` 1.6.* |
  | UniFFI | `uniffi` 0.28.x + `uniffi-bindgen-cs` 0.8.x (community C# backend) | cargo dep + `cargo install uniffi-bindgen-cs` |
  | Node.js | 20 LTS (for the renderer-zero parity test harness only) | `winget install OpenJS.NodeJS.LTS` |
  All versions are floors; bump within the same minor as long as W0/W1 gates stay green. Pin exact versions in `windows/ffi/Cargo.toml` and `Directory.Packages.props`.
- **Definition of "gate".** Each milestone ends with one mechanical command/observation a CI job or the developer runs; a milestone is **not** done until that command exits 0 (or the stated observable is literally seen on screen). Gates are cumulative — later gates re-run earlier ones.
- **What does NOT change.** No file under `forge/` is edited except (a) adding a `forge-ffi` crate to the workspace `members` list, and (b) `Cargo.lock`. The command/event contract (`forge/spec/commands.md`), the UI catalog (`forge/std/ui-catalog.d.ts`), the patch wire format (`forge/crates/ui/src/patch.rs`), and the golden fixtures (`forge/crates/ui/tests/golden/`) are **the spec** the shell implements against. The shell must not "fix" them.

---

## W0 — Build `forge-core.dll` on Windows + "hello core" over the boundary

**Goal:** prove the Rust core compiles to a Windows native library, a C# process can load it, send one real `CoreCommand`, and get back the matching `CoreResponse` — with zero business logic in C#.

### Deliverables
1. **`windows/ffi/` crate** — a new `cdylib` workspace member (`forge-ffi`) that depends on `forge-core`, `forge-domain`, and `serde_json`, and exposes a *narrow, generated* surface. Two binding paths are stood up side by side so PS-1 ("generated, never hand-written") is satisfiable and the C-ABI fallback is proven:
   - **Primary (UniFFI-C#):** a `forge.udl` (or proc-macro) describing `CoreHandle` with:
     ```udl
     namespace forge {};
     interface CoreHandle {
       [Throws=FfiError] constructor(string workspace_id);   // in-memory workspace for W0
       [Throws=FfiError] string handle_json(string command_json);  // CoreCommand JSON -> CoreResponse JSON
       u64 subscribe(EventCallback cb);                      // returns subscription id
       void unsubscribe(u64 sub_id);
     };
     callback interface EventCallback { void on_event(string event_json); };
     [Error] enum FfiError { "Serde", "Core" };
     ```
     The single `handle_json` entry point is deliberate: it mirrors `WorkspaceCore::handle(CoreCommand) -> CoreResponse` (see `forge/crates/core/src/lib.rs`), so the *entire* command catalog flows through one generated function and the binding surface exposes **no** direct path to SQLite/CRDT/policy (enforces CR-A1 / PS-14).
   - **Fallback (C-ABI):** the same crate also exports `#[no_mangle] extern "C"` functions:
     ```rust
     #[no_mangle] pub extern "C" fn forge_core_new(ws_id: *const c_char, out: *mut *mut CoreHandle) -> i32;
     #[no_mangle] pub extern "C" fn forge_core_handle(h: *mut CoreHandle, cmd_json: *const c_char, out_json: *mut *mut c_char) -> i32;
     #[no_mangle] pub extern "C" fn forge_string_free(p: *mut c_char);
     #[no_mangle] pub extern "C" fn forge_core_free(h: *mut CoreHandle);
     ```
     All return an `i32` status; **no `panic!` crosses the boundary** — every entry wraps the call in `std::panic::catch_unwind` and maps to `CoreError` JSON (honors CR-A4 "FFI calls never panic"). Strings are UTF-8, caller frees via `forge_string_free`.
2. **`build` outputs** — `forge_ffi.dll` + `forge_ffi.dll.lib` (import lib) for `x86_64-pc-windows-msvc`. arm64 cross-build attempted but allowed to be "compiles" not "tested on device" at W0.
3. **Generated C# bindings** in `windows/bindings/` (UniFFI: `forge.cs`; C-ABI: a small hand-written `NativeMethods.cs` with `[LibraryImport]` source-gen P/Invoke).
4. **`Forge.Core` C# class lib** with a `CoreClient` that wraps a handle and exposes `Task<CoreResponse> HandleAsync(CoreCommand)`. `CoreCommand`/`CoreResponse`/`CoreError`/`ActorContext`/`Role` are C# records whose JSON shape **exactly** matches `forge/crates/domain/src/lib.rs` (snake_case where the Rust uses it; `CoreError` is `{ "kind": "...", "detail": "..." }`).
5. **A "hello core" console smoke** (`Forge.Core.Tests` xUnit fact) that:
   - creates a handle for an in-memory workspace,
   - sends `workspace.open` and one `record.put` into a `notes` collection,
   - then `query.execute` and asserts the record round-trips.

### Exact build commands (Developer PowerShell for VS 2022)
```powershell
# from terrane\windows\ffi
cargo build --release --target x86_64-pc-windows-msvc
cargo build --release --target aarch64-pc-windows-msvc      # may use a cross linker; allowed to be untested
# generate C# bindings (UniFFI path)
uniffi-bindgen-cs --out-dir ..\bindings\csharp ..\..\windows\ffi\src\forge.udl
# build + test the C# side
dotnet test ..\Forge.Windows.sln -c Release
```

### Acceptance gate (W0 exit)
- `cargo build --release --target x86_64-pc-windows-msvc` produces `forge_ffi.dll` **and** `rusqlite` (bundled, see `forge/crates/storage`) links statically — no external `sqlite3.dll` is required next to the binary.
- `dotnet test` is **green**, and one test literally does:
  ```csharp
  var core = await CoreClient.OpenInMemoryAsync("ws-hello");
  var put = await core.HandleAsync(Cmd.RecordPut("notes", new { title = "hi" }));
  Assert.True(put.Ok);
  var rows = await core.HandleAsync(Cmd.QueryExecute("notes"));
  Assert.Contains("hi", rows.Payload.ToString());
  ```
- A 10-line C-ABI smoke (`Forge.Core.Tests/CAbiSmoke.cs`) sends the **same** command JSON through `forge_core_handle` and gets a byte-identical `CoreResponse` JSON to the UniFFI path. This dual-path pass is what proves PS-1 ("C-ABI adapter where generated bindings aren't mature") is a real, exercised fallback, not a hope.

**Satisfies:** PS-1, PS-14 (Rust DLL via UniFFI-C#/C-ABI), CR-A1/A2/A4 (envelope, no-panic boundary). **Maps to:** M6 entry; reuses the M0a contract proven headlessly by `forge demo`.

---

## W1 — WinUI renderer renders a static UI tree + applies patches (golden fixtures pass)

**Goal:** stand up `Forge.Renderer` that turns a UI-tree (the JSON in `forge/std/ui-catalog.d.ts` shape) into a WinUI 3 visual tree, then applies a `Patch[]` stream and ends up with the same visual tree the full re-render would produce. **This milestone gates the PS-15 Tauri fallback decision.**

### Deliverables
1. **`Forge.Renderer`** — a `WinUI 3` class lib that maps the **M0a catalog subset actually emitted by the core today** (`Stack`, `Text`, `Button`, `TextField`, `List`, plus `Node::Unknown`) to WinUI controls, and is structured to extend to the full ~26-component catalog (UI-2). Initial mapping:
   | Node `type` | WinUI control | Notes |
   |---|---|---|
   | `Stack` (`direction` h/v, `gap`) | `StackPanel` (Orientation, Spacing) | gap tokens → density-scaled px |
   | `Text` (`variant`) | `TextBlock` | variant → theme `Style` resource |
   | `Button` (`label`, `variant`, `onTap`) | `Button` | `Click` raises an event payload with the `onTap` ActionRef |
   | `TextField` (`value`, `label`, `placeholder`, `onChange`) | `TextBox` (controlled) | two-way guarded; emits `onChange` with text |
   | `List` (`items`, `virtualized`) | `ItemsRepeater` in `ScrollViewer` | virtualization per UI-5 (lazy realize) |
   | `Node::Unknown` | bordered `TextBlock` fallback box showing `type` + `Text`-coercible props | **UI-6 normative** — never throws |
2. **Patch applier** that consumes the exact wire ops from `forge/crates/ui/src/patch.rs` (`replace`, `update_text`, `update_prop`, `insert`, `remove`) addressed by **index path** (`[]` root, `[0]`, `[0,2]`). It mutates the live WinUI tree in place (no full re-render), exactly mirroring `apply()` semantics, including: `update_prop` keys (`id`, `testId`, `label`, `value`, `variant`, `onTap`, `onChange`, `gap`, `placeholder`), `insert`/`remove` only on container nodes (`Stack.children`, `List.items`), and erroring (surfaced, not crashing) on a leaf-with-no-children path — same contract as `children_mut`.
3. **Golden-fixture conformance runner** (`Forge.Renderer.Tests`, xUnit) that loads every file in `forge/crates/ui/tests/golden/` and:
   - **roundtrip\_\*** : parse `tree` → build visual tree → serialize the visual tree back to catalog JSON → assert equal to input (proves the build map is lossless).
   - **diff\_\*** : start from `old`, apply `expect_patches`, assert the resulting catalog JSON equals applying the same patches to the Rust model — i.e. `apply(old, expect_patches) == new`. (The fixtures already carry `old`, `new`/`expect_patches`.)
   - **unknown\_\*** : assert the fallback box renders and **no exception** is thrown (UI-6 fuzz floor).
4. **Renderer-zero parity check.** A small Node script (renderer-zero, `forge`'s DOM reference, UI-13) and the WinUI renderer are fed the same fixture set; their post-apply serialized trees must match. This is the cheap cross-renderer divergence detector before the heavier conformance kit lands in W2.

### Acceptance gate (W1 exit)
- `dotnet test Forge.Renderer.Tests` is **green for 100% of files** in `forge/crates/ui/tests/golden/` (21 fixtures at time of writing: 7 `roundtrip_*`, 11 `diff_*` incl. reorder/append/remove/replace, 3 `unknown_*`).
- An on-screen smoke: launch `Forge.Shell` in a "fixtures" dev mode, pick `roundtrip_nested_stack_list_button.json`, see the rendered controls; click "apply" to run `diff_button_label_change.json` patches and **see the button label change in place** (no flicker / full rebuild).
- Unknown-node fuzz: feed 100 synthetic trees containing unknown `type` and unknown props → zero exceptions, 100% fallback boxes (UI-6 acceptance floor).
- **PS-15 DECISION GATE (record the outcome in `windows/DECISION-PS15.md`):** tally actual engineering effort spent W0+W1 and estimate W2–W5. If the projected total WinUI effort exceeds the budget in `prd-merged/06` open-question 1 (**> 2 engineer-quarters**), invoke the **Tauri 2 fallback** (reuse the web renderer from `prd-merged/06` PS-10/PS-15) and re-scope W2–W5 onto Tauri. The W0 FFI work and the golden-fixture conformance runner are **reused unchanged** either way, so this gate is cheap to honor.

**Satisfies:** UI-1, UI-2, UI-5 (ItemsRepeater virtualization), UI-6, UI-12 (versioned wire format + golden trees), and PS-15 (decision gate). **Maps to:** M6 renderer conformance row in `prd-merged/05` §3.

---

## W2 — Full spine in the shell (the desktop equivalent of `forge demo`)

**Goal:** install the **notes-lite** applet (`forge/examples/notes-lite/`), run it, render its real UI tree, let the user edit, re-run, and see the rendered list update — entirely through commands. This is `forge demo` (`forge/crates/cli/src/lib.rs`) reproduced inside the WinUI shell.

### Deliverables
1. **Applet install flow** — shell reads `manifest.json` + `src/main.ts` from disk (or the embedded demo copy) and sends `applet.install` (payload: `manifest`, `source files`). The shell does **not** transpile or scan — that happens in-core via `forge-pipeline` (SWC) exactly as in the CLI path.
2. **Run + render loop** — shell sends `runtime.run` (payload `{ "input": ... }`); the response carries `run_id`, `result`, and `ui_renders` (the canonical catalog trees, same field the CLI reads). The renderer (W1) draws the first tree; subsequent trees arrive as patch streams over the event/stream subscription (`subscribe` from W0) and are applied in place.
3. **Event round-trip** — a `Button.onTap` / `TextField.onChange` in the rendered UI sends a UI-event command back into the core's event queue (CR-6 / UI-4); the core re-renders, diffs, and streams a patch; the renderer applies it. This closes the headless loop `prd-merged/05` UI-12 specifies (`simulate onTap → expect Modal/patch in next frame`) but with a real user and real controls.
4. **Replay/inspect affordance (read-only)** — a debug strip showing `run_id` + replay fingerprint, calling `runtime.replay` and asserting `replays_identically` (proves the desktop path is as deterministic as `forge demo`).

### Acceptance gate (W2 exit)
> The app installs **notes-lite**, shows the **Notes list** rendered from the core's UI tree; the user types a new note title into the `TextField`, taps **Add**, and the **List updates** with the new note; re-running (or the watch-driven re-render) reflects the edit. Concretely:
- Click **Install notes-lite** → an `applet.install` succeeds (response `ok: true`, an `applet_id`).
- Click **Run** → `runtime.run` returns `ok: true` and the **Notes** UI renders (a `Stack` containing a `TextField` + `Button` + a `List` of existing notes), matching what `forge demo` prints as `ui_trees[0]`.
- Type "Buy milk", tap **Add** → a UI event command flows to the core, a `record.put`-backed re-render streams a `List` `insert` patch, and the new row appears **without a full rebuild**.
- The debug strip shows `replays_identically = true` for the run.
- **No-business-logic audit (mechanical):** grep the C# tree for forbidden direct access — `Assert` in CI that `Forge.Shell` / `Forge.Renderer` contain zero references to SQLite, Loro, file writes to the workspace DB, or schema/policy types. The only persistence path is `CoreClient.HandleAsync`. (Enforces the PS-14 / CR-A1 "shell has no business logic" rule.)

**Satisfies:** the `prd-merged/00` core loop on Windows; UI-1/UI-4/UI-12 round-trip; CR-A1/A2/A3 (commands carry `ActorContext`, validated in-core). **Maps to:** M6 platform smoke of the demo workspace (`prd-merged/06` PS-4), mirroring M0a.

---

## W3 — Platform services (secrets, file pickers, notifications) + LLM/editor panels

**Goal:** provide the per-platform services the core calls back into (PS-3) and the shell-native workshop surfaces (`prd-merged/05` §B) so the shell is a real workstation, not just a viewer.

### Deliverables
1. **Secrets store** (`Forge.PlatformServices.Secrets`) — implements the core's `secrets`/Keychain-equivalent callback using **Windows Credential Manager** (`CredWrite`/`CredRead` via `Windows.Security.Credentials.PasswordVault` in packaged apps) with **DPAPI** (`ProtectedData.Protect`, `DataProtectionScope.CurrentUser`) for at-rest blobs that exceed Credential Manager size limits. Backs `secret.store` / `secret.revoke`. Applets only ever receive **write-only references** (CR-3), never raw secret values — the resolution happens host-side.
2. **File pickers returning handles** — `FileOpenPicker` / `FileSavePicker` (WinUI 3) wrapped so the shell hands the **core** an opaque handle (a `StorageFile` token via `StorageApplicationPermissions.FutureAccessList`), and the applet receives only a `files` handle (CR-3 / PS-3: "handles, never raw paths to applets").
3. **Notifications** — `AppNotificationBuilder` (Windows App SDK toast) bound to the core's `notifications` capability; returns `PlatformUnavailable` cleanly if toast registration fails (CR-3).
4. **OS permission ↔ capability mapping + permission UX** — the resource-specific prompt required by `prd-merged/05` UI-18 ("Allow `FetchWeather` to GET `https://api.weather.example/*` up to 1 MB per run?"), driven by `permission.request_grant` / `permission.revoke`. "Allow network?" is forbidden.
5. **Editor panel** (UI-15) — multi-file TS editor (Monaco or CodeMirror via WebView2; choice tracked in `prd-merged/00` open-q 3) showing generated `ctx`/schema types, inline diagnostics from the in-core pipeline (SWC + policy scan), and a run console wired to `runtime.get_logs`. The editor edits **CRDT text** via `file.write` (CR-10) — it never writes the DB directly.
6. **LLM panel** (UI-20) — prompt input, provider/model selector, plan/diff preview, apply/rollback, budget display; backed by `ai.generate_patch` / `ai.apply_patch` / `ai.run_fix_loop` / `ai.set_context_mode`. The shell renders the diff + **permission diff** and requires explicit approval for new grants (core loop step 4). The shell holds no model keys in plaintext — they live in the secrets store (1).

### Acceptance gate (W3 exit)
- **Secrets round-trip:** `secret.store` a value, restart the app, `secret.revoke` it; assert it was readable from Credential Manager between the two and unreadable after revoke. An applet requesting the secret receives only a reference and a host-resolved use, never the bytes.
- **File handle:** pick a file via the OS dialog; the applet receives a handle and can read it through `ctx.files`; the **raw path never appears** in any command payload sent from the shell (mechanical grep assert).
- **Notification:** an applet `ctx.notifications` call raises a real Windows toast; with toasts disabled in OS settings, the call returns `PlatformUnavailable` and the shell shows the honest fallback (no crash).
- **Permission prompt:** triggering a network capability shows the **resource-specific** prompt (name, method, domain glob, byte limit, duration) — verified against the UI-18 bar in a UX review checklist.
- **LLM/editor:** generate a one-field change to notes-lite via the LLM panel, see the code diff + permission diff, approve, and the change applies via `ai.apply_patch` → the running applet reflects it. (Cloud provider may be stubbed/BYOK; the gate is the command + review-UX path, not model quality.)

**Satisfies:** PS-2 (common surface), PS-3 (platform services), UI-15/18/20; CR-3 (capability-scoped host APIs), CR-10 (code-as-CRDT). **Maps to:** M6 shell parity with macOS PS-6/PS-8 surfaces.

---

## W4 — Embedded-server mode + firewall + auto-update

**Goal:** make the Windows shell host the embedded sync server (same `server` crate as macOS PS-7) with correct firewall handling, and ship a signed auto-update channel.

### Deliverables
1. **Embedded server toggle** — a Settings switch + a status indicator (system tray / title-bar status item) that starts/stops the `forge-server` (`prd-merged/03` SS-15..18) in-process via a command surface; port + relay config; backup schedule; pairing-QR display (reuse the macOS PS-7 feature set, render the QR with WinUI).
2. **Firewall prompt handling** — on first server start, trigger the standard Windows Defender Firewall inbound prompt; provide a packaged install-time `firewallRules` capability (MSIX) / `New-NetFirewallRule` helper for silent enterprise installs. Document the LAN vs relay posture. The shell must **not** open the port until the user enables server mode (PS-14 "firewall prompt handling for server mode").
3. **Auto-update** — an MSIX update channel. Two implementable options, pick one and record it:
   - **App Installer / `.appinstaller`** (sideload or web-hosted): `Windows.ApplicationModel.Store.Preview` / `AppInstallerManager` checks + applies updates; signed with the same cert as packaging (W5).
   - **Store-managed** updates if shipping via Microsoft Store.
   Before restart for an update, the **embedded server drains connections** (mirrors macOS PS-9 "drains connections before restart").
4. **Drain + restart correctness** — graceful shutdown that finishes in-flight sync frames, persists frontier, then relaunches.

### Acceptance gate (W4 exit)
- Toggle server mode **on** → the firewall prompt appears exactly once; accept → a second device (or a `forge` CLI client on the LAN) pairs via the QR/token and a 2-client sync exchange converges (smoke-level, not the full SS soak).
- Toggle server mode **off** → the listening port is closed (verify with `Get-NetTCPConnection`); no background listener remains.
- Stage a newer signed MSIX in the update channel → the app detects it, **drains the embedded server** (no dropped in-flight frame, frontier persisted), updates, relaunches, and the workspace opens unchanged.
- Cold start to interactive workspace **< 2 s** on a mid-tier Win11 laptop (the desktop bar in `prd-merged/06` §9 / `prd-merged/00` §10).

**Satisfies:** PS-14 (firewall, MSIX-class updates), PS-7-equivalent embedded server, SS-15..18; perf cold-start gate. **Maps to:** M6 (Windows) + M2 sync machinery reused.

---

## W5 — Packaging (MSIX, signed) + CI green + conformance-kit gate

**Goal:** produce a **signed MSIX** for x64 **and** arm64, wire a Windows CI pipeline, and pass the full per-platform conformance gates (PS-4) before declaring the shell shippable.

### Deliverables
1. **MSIX packaging** — `Package.appxmanifest` with: identity, `runFullTrust` (for the embedded native server) only if justified, capability declarations mapped from the core's capability set, file-type/`forge://` deep-link associations (PS-3), and per-arch packages (`x64`, `arm64`) bundled into an `.msixbundle`.
2. **Signing** — packages signed with a trusted code-signing certificate (`SignTool sign /fd SHA256 /a /f cert.pfx`); for CI, an Azure Trusted Signing or self-managed cert; the same cert chains the auto-update channel from W4.
3. **Windows CI** (`.github/workflows/windows.yml`, `windows-latest` runner) that, on every PR:
   - builds `forge_ffi.dll` for x64 (and cross-builds arm64),
   - regenerates + diffs the C# bindings (fails if bindings drift from `forge.udl`),
   - runs `dotnet test` (W0 hello-core, W1 golden fixtures, W2 spine smoke headless where possible),
   - runs the **renderer conformance kit** (UI-14): golden trees + scripted-interaction + screenshot tests shared with every renderer; **behavioral divergence is release-blocking** (same bar as CR-12),
   - builds + signs the MSIX, runs a packaged smoke launch.
4. **Conformance gate wiring (PS-4):** engine conformance is the core's (`forge` CI already runs the CR-12 suite); the Windows shell additionally must pass **renderer kit (UI-14)**, **data fixtures (DL/09)** loadability of a shared workspace file, and a **platform smoke of the demo workspace** (W2 reproduced in CI/headless + one manual on-device run).

### Acceptance gate (W5 exit — the milestone that says "ship-candidate")
- `windows.yml` is **green** on `main`: build + bindings-diff + `dotnet test` + conformance kit + signed MSIX build + packaged smoke.
- `Add-AppxPackage .\Forge.Windows.msixbundle` installs cleanly on a **fresh Win11 x64** VM **and** a **Win10 22H2 x64** VM; the same `.msixbundle` installs on an **arm64** device (Win11 ARM). After install, the W2 acceptance flow (install notes-lite → render → edit → re-render) passes on each.
- **Same-workspace portability:** a workspace file created on macOS (M1) opens on the Windows shell and renders identically (the `prd-merged/06` §9 "same workspace file opens on every shipped platform" gate; uses the DL/09 fixtures).
- Renderer conformance kit (UI-14) reports **zero behavioral divergence** vs renderer-zero / macOS golden screenshots, within documented per-platform tolerances.
- `SignTool verify /pa Forge.Windows.msixbundle` confirms a valid signature chain.

**Satisfies:** PS-4 (per-platform conformance gates), PS-14 (MSIX signed installer, x64+arm64, Win11 & Win10 22H2+), UI-14 (renderer conformance, release-blocking). **Maps to:** M6 exit ("renderer + engine conformance per platform").

---

## Milestone → requirement map (summary)

| Milestone | Primary deliverable | Key `prd-merged` reqs | Roadmap |
|---|---|---|---|
| **W0** | `forge_ffi.dll` + UniFFI-C# & C-ABI "hello core" | PS-1, PS-14, CR-A1/A2/A4 | M6 entry (reuses M0a contract) |
| **W1** | WinUI renderer + patch apply; golden fixtures pass; **PS-15 decision** | UI-1/2/5/6/12, PS-15 | M6 renderer row |
| **W2** | notes-lite install→run→render→edit→re-run (desktop `forge demo`) | core loop, UI-1/4/12, CR-A1/A2/A3 | M6 platform smoke (≈M0a) |
| **W3** | Secrets/pickers/notifications + editor/LLM panels | PS-2/3, UI-15/18/20, CR-3/10 | M6 shell parity |
| **W4** | Embedded server + firewall + auto-update | PS-14, PS-7-equiv, SS-15..18 | M6 + M2 reuse |
| **W5** | Signed MSIX (x64+arm64) + CI + conformance gate | PS-4, PS-14, UI-14 | M6 exit |

---

## Risks & open questions

1. **arm64 toolchain maturity.** Cross-building `forge_ffi.dll` for `aarch64-pc-windows-msvc` is straightforward, but linking native deps (`rusqlite` bundled SQLite; any `cc`-compiled bits) and *testing* on real ARM hardware is the risk. **Mitigation:** treat arm64 as "compiles in W0, bundled in W5, on-device-verified only at W5"; keep an arm64 Win11 device (or Windows-on-ARM VM) in the loop before W5 sign-off. If arm64 on-device slips, ship x64-only first (PS-14 allows it as a sequenced follow within M6) and gate arm64 behind its own smoke.
2. **WinUI 3 maturity vs. the PS-15 Tauri fallback (the big one).** WinUI 3 + Windows App SDK still has rough edges (packaging quirks, `ItemsRepeater` virtualization edge cases for 100k-row tables per UI-5, WebView2 interop for the editor). **Mitigation:** the **W1 decision gate** measures real effort against the > 2-engineer-quarter trigger (`prd-merged/06` open-q 1) and falls back to **Tauri 2 reusing the web renderer** (PS-10/PS-15). The architecture is deliberately fallback-friendly: W0 (FFI) and the golden-fixture/conformance runner are renderer-agnostic and reused unchanged, so a fallback costs the renderer + platform-service shells, not the boundary.
3. **UniFFI-C# vs. C-ABI choice.** `uniffi-bindgen-cs` is a *community* backend (not first-party Mozilla), so its support for callback interfaces (needed for the event/stream subscription) and async may lag. **Mitigation:** W0 stands up **both** paths and proves byte-identical responses; if UniFFI-C# can't cleanly express the `EventCallback` stream, fall back to the hand-written `[LibraryImport]` C-ABI surface (PS-1 explicitly sanctions "C-ABI adapter where generated bindings aren't mature"). Decision recorded in `windows/DECISION-FFI.md` at W0 exit.
4. **`tsgo`/type-checker sidecar on Windows (CR-15).** The full offline type-check needs a version-pinned native TS compiler sidecar supervised by the shell (macOS PS-8 equivalent). Windows process supervision + auto-restart + path/quoting issues are a known sharp edge. **Mitigation:** W3 ships SWC-transpile + policy-scan first (always available, in-core), with `tsgo` supervision as a fast-follow within W3; an honest "type-check pending" state covers the gap (mirrors the web posture in CR-15).
5. **Event/stream backpressure & UI-thread marshaling.** Patch streams arrive on a native callback thread; WinUI mutation must happen on the dispatcher thread. **Mitigation:** funnel all `on_event` callbacks through `DispatcherQueue.TryEnqueue`; load-test with a high-frequency `db.watch` re-render to confirm the < 16 ms p95 input→patched-frame budget (UI-4) holds.
6. **Code-signing cert procurement.** A trusted EV/OV cert (or Azure Trusted Signing onboarding) has lead time and is required for W4 auto-update **and** W5. **Mitigation:** start procurement at W0 so it is not on the W5 critical path; CI can use a self-signed cert for non-release smoke, with the real cert gated to release builds.
7. **Open question — Microsoft Store vs. sideload distribution.** Affects auto-update mechanism (W4) and capability review. **Proposal:** sideload `.appinstaller` first (faster iteration, no Store review latency), Store as a later channel; decide at W4 entry.
8. **Open question — editor component (CodeMirror 6 vs Monaco via WebView2).** Inherited from `prd-merged/00` open-q 3; affects W3 editor panel weight and a11y. **Proposal:** decide at W3 entry based on WebView2 footprint and bundle-size budget.

---

*End of document.* This plan is implementable top-to-bottom on one Windows machine; each milestone's gate is a single command or a single observable on-screen behavior, and every milestone names the exact `forge/` artifacts (commands, patch ops, golden fixtures, catalog) it implements against so the shell stays a thin renderer over the unchanged Rust core.
