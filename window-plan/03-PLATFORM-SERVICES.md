# Windows platform services

> Document 03 of the Windows-app plan for **forge**. Companion docs: `01` (architecture
> overview), `02` (the C# ↔ Rust FFI boundary), `04` (the WinUI 3 renderer). This document
> specifies the **platform capabilities the shell provides to the core** (PS-3, PS-14) and the
> exact Windows API used to implement each one.

## 0. Scope, principle, and the M0a line

The Windows shell is a **thin renderer + platform-services host** over the existing native Rust
core in `~/projects/terrane/forge` inside WSL. The shell contains **no business logic**
(prd-merged/06 §intro): it never mutates SQLite, CRDT docs, schemas, permissions, or runtime
state except by calling a `Command` (CR-A1). Platform services are therefore **effect providers
injected into the core**, not features the shell implements on its own.

There are two distinct injection seams, and every capability in this document plugs into exactly
one of them:

| Seam | What the core expects | Where it lives today | This doc's job |
|---|---|---|---|
| **`HostBridge`** (per-run, per-applet `ctx.*`) | `forge_runtime::HostBridge` trait — storage/db/ui/log effects, capability-checked, recordable | `forge/crates/runtime/src/bridge.rs` (trait), `forge/crates/core/src/bridge.rs` (`StorageHostBridge` impl) | Extend with `secrets`, `files`, `net`, `notifications` effect methods backed by Windows APIs |
| **Shell services** (process-level, not per-run) | Behaviour the shell owns: window chrome, file pickers, firewall prompts, toast registration, URI association, sidecar supervision | Not in core; this is shell code | Implement in C#/WinUI 3 with WinRT + Win32 APIs |

**The M0a line.** The committed spine runs *headlessly* today:
`forge-cli 'forge demo'` does TS → SWC → QuickJS → `ctx` → SQLite → UI tree → deterministic
replay. The spine needs only three platform capabilities, **all of which the core already provides
natively** with zero Windows-specific work:

- **SQLite** — bundled inside `forge-storage` (`rusqlite` with `features=["bundled"]`); the
  compiled Rust DLL *contains* SQLite. No separate SQLite install, no SQLitePCLRaw, nothing on the
  C# side. (§3)
- **QuickJS** — `rquickjs 0.12` native inside `forge-runtime`, compiled into the same DLL. (§4)
- **UI tree** — produced by `forge-ui`, surfaced as `ctx.ui.render(tree)`; the shell only *paints*
  it (renderer, doc `04`).

Everything else in this document — secrets, file pickers, notifications, deep links, the firewall
prompt, the type-check sidecar — is **post-spine**. The table in §11 marks each capability M0a vs
later so the implementer knows what blocks "first pixel on Windows" versus what is fast-follow.

**Targets** (PS-14): Windows 11 and Windows 10 22H2+ (build 19045+), x64 **and** arm64. Every API
named below is checked against Win10 22H2 availability; where a WinRT API needs an `HWND` interop
(unpackaged or desktop-process WinRT), it is called out explicitly.

---

## 1. Capability map: core surface ↔ Windows API

| `ctx` namespace / shell service | Core surface (what calls it) | Windows implementation | Milestone |
|---|---|---|---|
| `ctx.storage`, `ctx.db` | `HostBridge` (bundled SQLite) | none — in-DLL `rusqlite` (§3) | **M0a** |
| `ctx.ui.render` | `HostBridge::ui_render` → UI patches | WinUI 3 renderer (doc 04) | **M0a** |
| QuickJS engine | `forge_runtime::QuickJsEngine` | in-DLL `rquickjs` (§4) | **M0a** |
| `ctx.secrets` (write-only refs) | new `HostBridge::secret_*` + `secret.store/revoke` commands | Credential Manager (`PasswordVault` / Win32 `CredWrite`) + DPAPI wrap (§2) | later |
| `ctx.files` (handles, not paths) | new `HostBridge::file_*` + a shell handle table | `FileOpenPicker` / `FileSavePicker` → `StorageFile` → opaque handle (§5) | later |
| `ctx.notifications` | new `HostBridge::notify` + `platform.notify` cap | `AppNotificationManager` (Windows App SDK toast) (§6) | later |
| Deep links `forge://` | shell → `workspace.open` / `runtime.run` commands | `windows.protocol` MSIX extension + `AppInstance` redirect (§7) | later |
| Embedded server firewall | shell-side, around `sync.start` | `INetFwPolicy2` detect + MSIX firewall capability + guidance UI (§8) | later |
| Type-check (`tsgo`) | CR-15 sidecar, shell-supervised | `Process` + Job Object supervision (§9) | later |
| `ctx.net` (allowlisted fetch) | `HostBridge::net_fetch` (planned) | `System.Net.Http.HttpClient` in core's host, gated by manifest (§10) | later |

---

## 2. Secrets — Windows Credential Manager + DPAPI (SC-13)

### What the core expects

Secrets are **write-only references** (CR-3 `secrets`: "write-only references"; SC-13). An applet
**never reads a secret value**. It receives a `secret_ref` (an opaque id), and at the point of a
network call the *host* injects the secret into the request (the `secrets.use` capability:
`constraints.netHosts` + `injectInto: header|query`, see `forge/spec/capabilities.md`). This means:

- Applet JS surface: `ctx.secrets.ref("weather_api")` → returns a ref string; **no `get`**.
- Commands: `secret.store { name, value, scope, allowed_uses }` → `secret_ref`;
  `secret.revoke { secret_ref }` (`forge/spec/commands.md`, Owner/Maintainer only, milestone
  "later").
- The plaintext value crosses the FFI boundary exactly **once**, on `secret.store`, travelling
  C# → core. After that the core persists only an encrypted blob + metadata; the shell persists the
  OS-protected secret. The applet realm never sees plaintext (CR-1: zero ambient capability).

Today there is **no `secrets` crate** and **no `secret_*` method on `HostBridge`**
(`forge/crates/runtime/src/bridge.rs` has storage/db/ui/log only). So this capability requires both
a small core addition and the Windows shell service. Mark **later** (post-spine).

### Windows implementation

Two layers, used together:

1. **Windows Credential Manager** as the *system credential store* (per-user, roams with the
   account, surfaced in Control Panel → Credential Manager). This is the durable home of the secret
   bytes.
2. **DPAPI** (`CryptProtectData`, `CryptUnprotectData`, or `ProtectedData` in .NET) to wrap the
   value *before* it is handed to the core for its own metadata record, so the core's SQLite row
   holds a DPAPI-bound ciphertext that is useless if the DB is copied to another machine/user.

**Credential Manager via WinRT `PasswordVault`** (simplest, packaged-app friendly):

```csharp
using Windows.Security.Credentials;

// store: name = "weather_api", value = plaintext from the secret.store command
var vault = new PasswordVault();
var cred  = new PasswordCredential(
    resource: $"forge/{workspaceId}",   // namespace per workspace
    userName: secretName,               // the secret's logical name
    password: secretValue);
vault.Add(cred);

// the shell returns ONLY a ref to the core; never the value again
string secretRef = $"secret:{workspaceId}:{secretName}";
```

`PasswordVault.Retrieve(resource, userName)` is called **only inside the host's network-injection
path**, never exposed to JS. `vault.Remove(cred)` backs `secret.revoke`.

> **Caveat (document it in diagnostics):** `PasswordVault` has a hard cap of ~10 credentials per
> resource string and per-credential size limits (~512 chars practical). For larger secrets, or to
> exceed the count, fall back to the Win32 Credential Manager API (`CredWriteW` /
> `CredReadW` / `CredDeleteW` with `CRED_TYPE_GENERIC`, target name
> `forge:{workspaceId}:{secretName}`) which stores up to 2560 bytes (`CRED_MAX_CREDENTIAL_BLOB_SIZE`).
> Use `CredProtect`/`CredUnprotect` or DPAPI for the blob. This is the recommended primary path for
> a desktop process; reserve `PasswordVault` for small tokens.

**DPAPI wrap of the metadata blob handed to the core:**

```csharp
using System.Security.Cryptography;   // requires <PackageReference Include="System.Security.Cryptography.ProtectedData" Version="9.0.*" />

byte[] cipher = ProtectedData.Protect(
    userData: Encoding.UTF8.GetBytes(secretValue),
    optionalEntropy: Encoding.UTF8.GetBytes(secretRef), // bind to the ref
    scope: DataProtectionScope.CurrentUser);            // per-user, per-machine
// `cipher` is what crosses FFI into secret.store's payload as `value` (opaque bytes);
// the core stores it but cannot decrypt it — only this user on this machine can.
```

### Core-side addition (small, do once)

Add to `forge_runtime::HostBridge`:

```rust
/// Resolve a secret reference for host-side injection. Returns ONLY the ref's
/// metadata (allowed uses); never the plaintext. The shell performs injection.
fn secret_ref(&mut self, name: &str) -> Result<serde_json::Value>;   // { ref, netHosts, injectInto }
```

The actual plaintext path lives outside `HostBridge` (in the net-fetch host, §10), so the trait
never carries a secret value. `secret.store`/`secret.revoke` are handled in
`WorkspaceCore::handle` (`forge/crates/core/src/workspace.rs`), calling out to a new `forge-secrets`
crate (`forge/crates/secrets/`, per PRD 01 §2 crate layout) whose Windows impl is a thin C-ABI
callback into the C# `PasswordVault`/`CredWrite` code above.

**Acceptance checks (§2):**
- `secret.store` with a 1 KB token round-trips: the credential appears in *Control Panel →
  Credential Manager → Windows Credentials* under `forge:<ws>:<name>`; `secret.revoke` removes it.
- A FFI trace dump of the applet realm proves **no plaintext** ever reaches JS: grep the recorded
  `ctx.*` call log for the secret value → zero hits.
- Copying the workspace SQLite file to a second Windows user account and attempting decrypt of the
  stored metadata blob fails (DPAPI `CurrentUser` scope) → confirms at-rest binding.

---

## 3. SQLite — already native, nothing to do (DL)

### What the core expects

`ctx.storage` and `ctx.db` (`HostBridge::storage_*`, `db_*` in `bridge.rs`) are backed by
`forge_storage::Store`, the SQLite KV/oplog/index layer (`StorageHostBridge` in
`forge/crates/core/src/bridge.rs`).

### Windows implementation: **none required**

`forge-storage/Cargo.toml` pins:

```toml
rusqlite = { version = "0.40.1", features = ["bundled"] }
```

The `bundled` feature compiles the SQLite **amalgamation C source straight into the Rust staticlib**
via `libsqlite3-sys`. When the core is built as a Windows `cdylib` (`forge_core.dll`, see doc 02),
**SQLite is inside that DLL**. Consequences for the Windows implementer:

- **Do not** add `Microsoft.Data.Sqlite`, `System.Data.SQLite`, `SQLitePCLRaw`, or any `sqlite3.dll`
  to the MSIX payload. There is exactly one SQLite, and it is the core's. Shipping a second one is a
  bug (two SQLite builds touching the same file = corruption risk).
- The DB file lives at an app-private path the *shell* chooses and passes to `workspace.open`
  (`forge/spec/commands.md`: `workspace.open` takes a `path`). Use
  `ApplicationData.Current.LocalFolder.Path` (e.g.
  `%LOCALAPPDATA%\Packages\<PackageFamilyName>\LocalState\workspaces\<id>.forgedb`) so MSIX
  containerization and uninstall-cleanup apply.
- **Toolchain note (build-time):** `rust-toolchain.toml` requires stable ≥ 1.93 because
  `libsqlite3-sys 0.38+` uses `cfg_select!`. On Windows the bundled SQLite C compiles with the
  **MSVC** toolchain (`x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`); ensure Visual Studio
  Build Tools 2022 with the C++ workload + Windows 11 SDK are installed (see doc 02 §build). No
  runtime dependency results — purely a build prerequisite.

**Acceptance checks (§3):**
- `dumpbin /dependents forge_core.dll` shows **no** `sqlite3.dll` import; the DLL is self-contained
  for SQLite.
- `forge-cli 'forge demo'` cross-compiled to `x86_64-pc-windows-msvc` writes and reads back the
  demo record on a clean Win10 22H2 VM with no SQLite installed.
- File path is inside the package LocalState; uninstalling the MSIX removes the DB.

---

## 4. QuickJS engine — native rquickjs on Windows (CR-2)

### What the core expects

`forge_runtime::QuickJsEngine` (the `#[cfg(not(target_arch = "wasm32"))]` native impl) is the M0a
spine engine (CR-2: "QuickJS is the non-negotiable spine"). It is compiled into `forge_core.dll`.
The shell never talks to QuickJS directly — it calls `runtime.run` and gets back results + UI
patches + logs.

### Windows implementation: **confirm it builds, then nothing**

`rquickjs 0.12` wraps the QuickJS C library. It builds on Windows, but the implementer **must pin
the C toolchain** because QuickJS C is non-trivial:

- **Recommended: MSVC.** `rquickjs 0.12` builds against `x86_64-pc-windows-msvc` and
  `aarch64-pc-windows-msvc` with the Visual Studio 2022 C++ toolset. This is the default for a
  Windows-native DLL and what the MSIX ships.
- **Alternative: clang.** If a specific QuickJS feature flag misbehaves under MSVC, `rquickjs` can
  build with `clang-cl`/LLVM (`bindgen` requires `libclang` regardless — install LLVM and set
  `LIBCLANG_PATH`). Document whichever is chosen; **do not mix** per-architecture.
- `rquickjs` features: keep the spine's existing set (no `loader`/`bindgen`-of-extras beyond what
  `forge-runtime` already enables). Do **not** enable any feature that pulls a runtime DLL — QuickJS
  must be statically linked into `forge_core.dll`.
- arm64: QuickJS has no arch-specific asm in the path `rquickjs` uses, so arm64 is a clean recompile.
  Validate on a Windows-on-ARM device or the arm64 CI runner.

**Resource limits are shell-independent.** CR-5's fuel/interrupt accounting lives in the *shared
host shim* in `forge-runtime` (the engine interrupt every 10 ms, 100 ms budget), not in C#. The
Windows shell does **not** implement any sandboxing — it relies entirely on the in-DLL QuickJS realm
(zero ambient capability) plus the policy layer. Kill/revoke (CR-5: < 100 ms) is a `runtime.cancel`
command, not an OS process kill.

**Acceptance checks (§4):**
- `cargo build -p forge-runtime --target x86_64-pc-windows-msvc` and `...aarch64-pc-windows-msvc`
  both succeed; the hostile-applet corpus (`forge/corpus/`) runs green on both via
  `forge-cli` cross-built, proving the engine + limits behave identically to macOS/Linux.
- `runtime.replay` of a recorded run produces byte-identical output on Windows vs the macOS
  reference (CR-12 cross-platform determinism), gating against any MSVC float/locale divergence.
- A `forge-runtime` Windows CI job is added so engine regressions are caught (mirrors the
  cross-engine conformance suite CR-12; the Windows job runs QuickJS-native).

---

## 5. File pickers — handles, never raw paths (CR-3)

### What the core expects

CR-3 `files`: "user-granted handles only"; PS-3: "file pickers returning **handles** (never raw
paths to applets)". The applet calls `ctx.files.openPicker(...)` / `ctx.files.savePicker(...)` and
receives an **opaque handle** it can read/write through `ctx.files.read(handle)` /
`ctx.files.write(handle, bytes)`, gated by the `files` capability (`files.read|write|history`,
resource `workspace:/...` or a granted external handle). The applet **never learns the real path**
— that is the whole point (an applet that knew `C:\Users\me\Documents\taxes.xlsx` would defeat the
sandbox).

`HostBridge` has no `file_*` methods today; add them (mark **later**):

```rust
fn file_read(&mut self, handle: &str) -> Result<serde_json::Value>;   // { bytes_b64, name, contentType }
fn file_write(&mut self, handle: &str, bytes_b64: &str) -> Result<()>;
```

### Windows implementation

Use **`FileOpenPicker` / `FileSavePicker`** (WinRT). They return a `StorageFile`, which already
*is* a handle abstraction (an app keeps capability to a file the user picked, even outside its
sandbox). The shell maintains a **handle table** mapping an opaque id → `StorageFile`, and only the
id crosses into the core/applet.

```csharp
using Windows.Storage;
using Windows.Storage.Pickers;
using WinRT.Interop;   // for HWND interop in a desktop/WinUI 3 process

// WinUI 3 (desktop) requires associating the picker with the window HWND,
// otherwise FileOpenPicker throws COMException (no CoreWindow).
var picker = new FileOpenPicker();
var hwnd = WindowNative.GetWindowHandle(App.MainWindow);
InitializeWithWindow.Initialize(picker, hwnd);

picker.FileTypeFilter.Add("*");      // narrow per the manifest's files constraint
StorageFile file = await picker.PickSingleFileAsync();
if (file == null) { /* user cancelled → core gets a typed cancel, not an error */ }

// Persist capability across restarts (optional, for re-grant):
string futureToken = StorageApplicationPermissions.FutureAccessList.Add(file);

// Hand the applet an OPAQUE handle, never file.Path:
string handle = handleTable.Register(file, futureToken);  // e.g. "fh_01H..."
```

`FileSavePicker` mirrors this (`PickSaveFileAsync`). Read/write go through `StorageFile`:

```csharp
// file_read
var buffer = await FileIO.ReadBufferAsync(handleTable.Resolve(handle));
// file_write (use CachedFileManager to coordinate with other apps editing the file)
CachedFileManager.DeferUpdates(file);
await FileIO.WriteBytesAsync(file, bytes);
await CachedFileManager.CompleteUpdatesAsync(file);
```

**Rules the implementer must enforce:**
- The handle is a random id (`fh_` + ULID); the handle→`StorageFile` map lives **only in the shell**
  process memory (plus `FutureAccessList` for persistence). The core stores the *handle string* in
  the capability grant, never the path.
- `StorageFile.Path` is **never** sent across FFI, never logged in `ctx.files` traces, never put in
  a UI tree.
- The picker itself is the OS permission prompt (UI-18 maps cleanly: the user explicitly chose the
  file). A `files.write` capability grant to an external file is recorded with the file's
  *display name* only.
- MSIX capability: declare `broadFileSystemAccess` **only if** unprompted path access is genuinely
  needed (it triggers Store review friction); the picker path needs **no** broad capability —
  prefer it.

**Acceptance checks (§5):**
- An applet granted `files.read` opens a user-picked `.csv`, reads it, renders a `Table` — and a
  trace shows the applet received `{ handle: "fh_...", name: "data.csv" }` with **no `path` field**.
- Killing and relaunching the app, then re-granting via `FutureAccessList`, restores read access to
  the same file without re-prompting beyond the consent UI.
- Revoking the `files` capability (`permission.revoke`) makes the next `file_read(handle)` return
  `CapabilityRequired`.

---

## 6. Notifications — Windows toast (App SDK)

### What the core expects

CR-3 platform capability `notifications`; capability grammar `platform.notify`
(`resource: desktop:notification`, `constraints.urgency`). On unsupported targets it returns
`PlatformUnavailable` (CR-A4). Add `HostBridge::notify(title, body, urgency) -> Result<()>` (mark
**later**).

### Windows implementation

Use **`AppNotificationManager`** from the **Windows App SDK** (`Microsoft.WindowsAppSDK`,
1.5+/1.6 recommended; the Windows App SDK is already the WinUI 3 dependency). This supersedes the
old `ToastNotificationManager`/`Windows.UI.Notifications` path and works for both packaged (MSIX)
and (with a registered AUMID) unpackaged desktop apps.

```csharp
using Microsoft.Windows.AppNotifications;
using Microsoft.Windows.AppNotifications.Builder;

// Once at startup (App.xaml.cs OnLaunched), before showing any toast:
AppNotificationManager.Default.NotificationInvoked += OnToastActivated;
AppNotificationManager.Default.Register();   // registers the COM activator + AUMID

// per ctx.notifications call:
var toast = new AppNotificationBuilder()
    .AddText(title)
    .AddText(body)
    .SetScenario(urgency == "urgent"
        ? AppNotificationScenario.Urgent
        : AppNotificationScenario.Default)
    .Build();
AppNotificationManager.Default.Show(toast);   // returns immediately; non-blocking
```

- The **MSIX manifest** auto-provides the AUMID and an `com.activation` extension; for the
  packaged shell no extra registry work is needed. For a non-MSIX dev build, register the AUMID +
  activator manually (documented in the App SDK toast sample) — but the shipping shell is MSIX, so
  this is a dev-only concern.
- Map `urgency` from the capability constraint: `normal` → `Default`, `urgent`/`high` →
  `Urgent` scenario (which can break through Focus Assist with the right registration).
- `notifications` is a **shell-owned effect**; if the shell is a headless/server build (no UI), the
  bridge returns `PlatformUnavailable` so the applet handles absence gracefully (CR-11
  feature-detect).

**Acceptance checks (§6):**
- An applet with `platform.notify` granted fires a toast that appears in Action Center with the app
  identity; without the grant it gets `CapabilityRequired`.
- Tapping the toast routes back into the app (deep-link-style activation, see §7) and surfaces the
  originating applet/page.
- On a build with notifications stripped, the call returns `PlatformUnavailable`, **not** a crash.

---

## 7. Deep links — `forge://` URI association (PS-3, UI-9)

### What the core expects

PS-3: deep links `forge://`. UI-9 defines the URI shape:
`forge://ws/<workspaceId>/applet/<appletId>/page/<pageId>`. The shell **parses** the URI and turns
it into core commands (`workspace.open`, then navigate / `runtime.run`) — it never lets the URI
reach an applet as raw data. No core change is needed; this is pure shell routing (mark **later**,
but cheap).

### Windows implementation

**MSIX protocol extension** in `Package.appxmanifest`:

```xml
<Extensions>
  <uap:Extension Category="windows.protocol">
    <uap:Protocol Name="forge">
      <uap:DisplayName>Forge deep link</uap:DisplayName>
    </uap:Protocol>
  </uap:Extension>
</Extensions>
```

Handle activation in `App.OnLaunched` / `OnActivated`:

```csharp
protected override void OnLaunched(LaunchActivatedEventArgs args)
{
    var activated = AppInstance.GetCurrent().GetActivatedEventArgs();
    if (activated.Kind == ExtendedActivationKind.Protocol &&
        activated.Data is ProtocolActivatedEventArgs p)
    {
        // p.Uri = forge://ws/<id>/applet/<id>/page/<id>
        var route = ForgeUri.Parse(p.Uri);   // validate against UI-9 grammar
        // → core commands; NEVER pass p.Uri to an applet realm
        Core.Handle(Command.WorkspaceOpen(route.WorkspaceId));
        Navigation.Go(route.AppletId, route.PageId);
    }
}
```

**Single-instance redirection** (so a second `forge://` click focuses the existing window instead
of launching a duplicate):

```csharp
// In Main(): use Windows App SDK AppInstance.FindOrRegisterForKey + RedirectActivationToAsync
var keyInstance = AppInstance.FindOrRegisterForKey("forge-main");
if (!keyInstance.IsCurrent)
{
    await keyInstance.RedirectActivationToAsync(
        AppInstance.GetCurrent().GetActivatedEventArgs());
    return;   // exit this process; the primary handles the URI
}
```

**Security:** treat every incoming URI as untrusted. Validate the workspace id exists and the
actor has access **before** opening; reject malformed routes with a user-visible error; never
auto-`runtime.run` from a URL without the workspace's normal RBAC (CR-A3). Toast activation (§6)
reuses this same router.

**Acceptance checks (§7):**
- Pasting `forge://ws/<valid>/applet/<valid>/page/main` into Run dialog opens the app on that page;
  a second paste focuses the existing window (no duplicate process).
- `forge://ws/<nonexistent>/...` shows a clean "workspace not found" message, no crash, no partial
  state.
- An attacker URI cannot trigger a privileged command: opening still goes through RBAC.

---

## 8. Embedded-server firewall prompt (SS-15, Windows Firewall)

### What the core expects

PS-7/PS-14: embedded server mode (`forge-server`, SS-15..18) binds a TCP port for LAN sync. When the
shell starts the server (around `sync.start` / a settings toggle), Windows Defender Firewall will
prompt or silently block inbound connections. The core just opens a socket; **handling the firewall
is a shell responsibility** (mark **later**; only relevant when embedded-server ships).

### Windows implementation

Three coordinated pieces:

1. **MSIX firewall capability + rule.** Declare the loopback/private-network intent and, for
   packaged apps, the firewall rule is associated with the package identity. In
   `Package.appxmanifest` request the relevant capability (`privateNetworkClientServer`) so inbound
   on private networks is allowed by policy for the package.

2. **Detect the current firewall state before binding**, using the `INetFwPolicy2` COM API, so the
   settings UI can show an honest status *before* the OS popup appears:

```csharp
// Add a COM reference to "NetFwTypeLib" (HNetCfg.FwPolicy2).
Type t = Type.GetTypeFromProgID("HNetCfg.FwPolicy2");
var policy = (INetFwPolicy2)Activator.CreateInstance(t);
var profile = policy.CurrentProfileTypes;   // Domain/Private/Public bitmask
bool fwOn = policy.get_FirewallEnabled((NET_FW_PROFILE_TYPE2_)profile);
// surface in Settings → Embedded server: "Firewall is ON for your Private network"
```

3. **Programmatically add an inbound allow rule** for the chosen port when the user enables embedded
   server (requires elevation; if not elevated, show guided instructions instead of failing
   silently):

```csharp
var ruleType = Type.GetTypeFromProgID("HNetCfg.FwRule");
var rule = (INetFwRule)Activator.CreateInstance(ruleType);
rule.Name        = "Forge embedded server (sync)";
rule.Description = "Allow LAN peers to sync with this workspace";
rule.Protocol    = 6;                 // TCP
rule.LocalPorts  = chosenPort.ToString();
rule.Direction   = NET_FW_RULE_DIRECTION_.NET_FW_RULE_DIR_IN;
rule.Action      = NET_FW_ACTION_.NET_FW_ACTION_ALLOW;
rule.Profiles    = (int)NET_FW_PROFILE_TYPE2_.NET_FW_PROFILE2_PRIVATE; // private only
rule.Enabled     = true;
policy.Rules.Add(rule);
```

**UX rules (SS-15, honest networking posture):**
- Default the bind to **private networks only**; never auto-open on Public profile.
- If the process is not elevated and cannot add the rule, **do not** swallow the failure — show the
  exact port + the manual "Allow an app through firewall" steps, and reflect "blocked" status in
  Settings until resolved.
- On disabling embedded server, **remove** the firewall rule (`policy.Rules.Remove(rule.Name)`) —
  leave no dangling open port.
- The graceful-drain-before-restart requirement (PS-9 analog) is the server's job; the firewall code
  only governs the port.

**Acceptance checks (§8):**
- Enabling embedded server on a Private network adds exactly one named inbound rule visible in
  `wf.msc`; a LAN peer can reach the sync port; disabling removes the rule.
- On Public network the port is **not** opened automatically; the UI states why.
- Non-elevated run shows actionable guidance, never a silent dead port.

---

## 9. Offline type-check sidecar — `tsgo` on Windows (CR-15)

### What the core expects

CR-15: desktop runs a **managed native TS compiler** (`tsgo`/TS7-class) sidecar, version-pinned to
the core release, **supervised by the shell**. It is fully offline (D7), no cloud. The core asks for
a type-check; the shell's supervised process answers. SWC transpile (CR-14) is *in-core* and already
works — the sidecar is the *full type-check* layer, separate from transpile. Mark **later** (the
spine ships with transpile + policy scan; type-check is a fast-follow, exactly as CR-15 allows web
to defer it).

### Windows implementation

Supervise `tsgo.exe` (the native Go-based TypeScript compiler, TS7-class) as a child process with a
**Win32 Job Object** so it dies with the shell and cannot be orphaned:

```csharp
using System.Diagnostics;

var psi = new ProcessStartInfo
{
    FileName  = Path.Combine(AppContext.BaseDirectory, "tools", "tsgo.exe"),
    Arguments = "--lsp --stdio",          // long-running server mode over stdio
    RedirectStandardInput  = true,
    RedirectStandardOutput = true,
    RedirectStandardError  = true,
    UseShellExecute = false,
    CreateNoWindow  = true,
};
var proc = Process.Start(psi);

// Assign to a Job Object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE so the
// checker can NEVER outlive the shell (no zombie tsgo.exe after a crash):
var job = JobObject.Create();
job.SetKillOnClose();
job.Assign(proc);   // P/Invoke AssignProcessToJobObject
```

Supervision requirements (mirror macOS PS-8):
- **Version-pinned:** ship the exact `tsgo.exe` matching the core release **inside the MSIX**
  (`tools\tsgo.exe`); never download at runtime (offline guarantee). Pin both x64 and arm64 binaries
  and pick by `RuntimeInformation.ProcessArchitecture`.
- **Auto-restart with backoff** on crash; cap restarts (e.g. 5 in 60 s) then surface a degraded
  "type-check unavailable, transpile+policy-scan only" state (the same honest-fallback posture
  CR-15 mandates for web).
- **Lifecycle:** start lazily on first edit/generate (not at app launch — keeps cold start < 2 s,
  PS acceptance), stop after idle timeout.
- **Crash-only protocol:** communicate over stdio (LSP-style); treat the sidecar as untrusted-ish
  (it parses user TS) — it runs at the same integrity level but the Job Object bounds it.
- The sidecar's results feed the editor diagnostics (UI-15) and gate `applet.install` exactly like
  on macOS; the *contract* (what a type-check request/response looks like) is identical across
  platforms — only the supervision is Windows-specific.

**Acceptance checks (§9):**
- Editing a `.ts` file with a type error surfaces a diagnostic from the supervised `tsgo.exe`;
  fixing it clears it.
- Killing `tsgo.exe` via Task Manager triggers shell auto-restart within the backoff window;
  killing the shell removes `tsgo.exe` from the process list immediately (Job Object).
- Air-gapped machine (no network) type-checks normally — proves the pinned-binary offline guarantee.
- arm64 device runs the arm64 `tsgo.exe`, not the x64 one under emulation.

---

## 10. `ctx.net` allowlisted fetch + secret injection (CR-3 `net`)

### What the core expects

CR-3 `net`: `ctx.http.fetch` to **manifest-allowlisted domains only**; the capability grammar's
`net` namespace (`forge/spec/capabilities.md`) pins scheme/host/path/method + byte/timeout
constraints, and `secrets.use` injects a stored secret into the request `header`/`query` **without
revealing it to the applet** (ties to §2). `HostBridge::net_fetch` is **planned**, not yet present.
This is a host effect, so it belongs in the core's host layer, with the actual HTTP performed by the
platform — mark **later**.

### Windows implementation

The HTTP itself can be performed by **`System.Net.Http.HttpClient`** in the C# host (or
`Windows.Web.Http.HttpClient` for WinRT-native proxy/credential integration). The **enforcement**
(allowlist match, byte caps, method check, secret injection) lives in the **core/policy** layer — the
shell only executes a request the core has already authorized and *fully specified*, including which
secret to inject and where. The applet realm sees only the response body subset the capability
allows; it never sees the secret or arbitrary headers.

```csharp
// The core hands the shell a fully-resolved, policy-checked request descriptor:
//   { method, url (matched against allowlist), headers (incl. resolved secret),
//     maxResponseBytes, timeoutMs }
// The shell performs it and streams back at most maxResponseBytes.
using var client = new HttpClient { Timeout = TimeSpan.FromMilliseconds(req.TimeoutMs) };
using var msg = new HttpRequestMessage(new HttpMethod(req.Method), req.Url);
foreach (var (k, v) in req.Headers) msg.Headers.TryAddWithoutValidation(k, v); // secret already injected here, by the host, from PasswordVault — never logged
var resp = await client.SendAsync(msg, HttpCompletionOption.ResponseHeadersRead);
// enforce maxResponseBytes while reading; abort if exceeded → ResourceLimitExceeded
```

**Rules:**
- The allowlist check is **never** done in C# (no business logic in the shell). The shell receives
  an already-authorized request or refuses. Wildcard hosts are forbidden by the grammar.
- Secret injection happens at this exact boundary, reading from Credential Manager (§2); the secret
  value appears in process memory only for the duration of the request and is **never** logged,
  traced, or returned to JS.
- Byte/timeout caps map to CR-5 per-run limits → `ResourceLimitExceeded` on breach.

**Acceptance checks (§10):**
- An applet granted `net.request https://api.example.com/public/*` + `secrets.use weather_api`
  succeeds against the allowed host; the same code against `https://evil.example` returns
  `CapabilityRequired`/`ValidationError`, never reaching the network.
- The request trace and logs contain **no** secret value (grep proof, as in §2).
- A response larger than `maxResponseBytes` aborts with `ResourceLimitExceeded`.

---

## 11. Milestone matrix (M0a vs later)

| # | Capability | Seam | Needs core change? | Windows work | Milestone |
|---|---|---|---|---|---|
| §3 | SQLite (`db`/`storage`) | `HostBridge` | no — already native | **none** (in-DLL `rusqlite`) | **M0a** |
| §4 | QuickJS engine | core runtime | no — already native | confirm MSVC/clang build; CI job | **M0a** |
| — | UI tree render | `HostBridge::ui_render` | no | WinUI 3 renderer (doc 04) | **M0a** |
| §2 | Secrets (write-only refs) | `HostBridge` + commands + `forge-secrets` crate | yes (new crate + `secret_ref`) | Credential Manager / `CredWrite` + DPAPI | later |
| §5 | File pickers (handles) | `HostBridge::file_*` | yes (new methods + handle table) | `FileOpenPicker`/`FileSavePicker` + `FutureAccessList` | later |
| §6 | Notifications | `HostBridge::notify` + `platform.notify` cap | yes (new method) | `AppNotificationManager` (App SDK toast) | later |
| §7 | Deep links `forge://` | shell router → commands | no | `windows.protocol` ext + single-instance redirect | later |
| §8 | Embedded-server firewall | shell, around `sync.start` | no | `INetFwPolicy2` + MSIX capability + guidance UI | later |
| §9 | Type-check sidecar (`tsgo`) | CR-15, shell-supervised | no (contract exists) | `Process` + Job Object, pinned binary in MSIX | later |
| §10 | `ctx.net` fetch + injection | `HostBridge::net_fetch` (planned) | yes (planned method) | `HttpClient`, enforcement stays in core | later |

**Bottom line for the implementer:** to get **first pixel on Windows** you implement *nothing* in
this document except confirming the two native-build facts (§3 SQLite-in-DLL, §4 QuickJS-on-MSVC)
and wiring `ui_render` to the renderer (doc 04). The spine's storage + db + UI are already provided
by the core natively. Everything else (§2, §5–§10) is post-spine platform work, each with its own
new `HostBridge` method or shell service, its own MSIX manifest declaration, and its own acceptance
gate above — and none of it puts business logic in the shell (CR-A1).

---

## 12. Cross-cutting constraints (apply to every capability)

- **No business logic in the shell (CR-A1).** Every capability is either an injected `HostBridge`
  effect (capability-checked *in the core's policy layer*) or a shell service that produces a
  *command*. The shell never decides allow/deny, never mutates state directly.
- **Typed errors across FFI (CR-A4).** A Windows API failure maps to a `CoreError` variant
  (`StorageError`, `PlatformUnavailable`, `ResourceLimitExceeded`, `CapabilityRequired`,
  `ValidationError`) and is returned as `CoreResponse.error` — the FFI boundary **never panics**
  (CR-A4: "FFI calls never panic across the boundary").
- **`PlatformUnavailable`, not crash (CR-3).** Any capability the running build doesn't provide
  (e.g. notifications on a headless server build) returns `PlatformUnavailable`; applets
  feature-detect via `forge.has(...)` (CR-11).
- **Secrets never reach JS, never get logged (SC-13).** Plaintext crosses FFI once (store), lives in
  Credential Manager, and is injected host-side; it is absent from every `ctx.*` trace, UI tree, and
  log.
- **Handles, never paths (CR-3).** File and (later) other OS resources are exposed to applets as
  opaque shell-held handles; real paths stay in the shell.
- **MSIX is the distribution unit (PS-14).** Every platform declaration above (protocol, firewall
  capability, toast AUMID, bundled `tsgo.exe`, LocalState DB path) is wired through the single
  signed MSIX package; uninstall cleans up DB, credentials scope, and firewall rule.
- **x64 + arm64 parity (PS-14).** Native builds (core DLL, QuickJS, `tsgo.exe`) ship per-arch; every
  acceptance check is run on both. Windows 10 22H2 is the floor for every WinRT API named here.
