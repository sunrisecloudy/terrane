# 04 — Packaging, Signing, Auto-Update, CI

**Scope:** how the Windows shell (C#/WinUI 3 over the forge Rust core) is built,
packaged into a signed MSIX, distributed, kept up to date (PS-9), and gated in CI
on a GitHub Actions Windows runner. This is the "ship it" document. It assumes the
native library `forge_ffi.dll` from
[`01-BUILD-AND-FFI.md`](./01-BUILD-AND-FFI.md), the WinUI renderer +
conformance harness from [`02-WINUI-RENDERER.md`](./02-WINUI-RENDERER.md), and the
platform services (firewall, `tsgo` sidecar) from
[`03-PLATFORM-SERVICES.md`](./03-PLATFORM-SERVICES.md). It is milestone **W5** in
[`05-MILESTONES.md`](./05-MILESTONES.md).

**PRD anchors:** PS-14 (MSIX-class signed installer/updates, x64 + arm64, Win11 &
Win10 22H2+), PS-9 (auto-update, embedded server drains connections before
restart), PS-4 (per-platform conformance gates: engine CR-12, renderer UI-14, data
fixtures, smoke), CR-12 / UI-14 (conformance is release-blocking).

**Thin-shell rule:** packaging adds no business logic. The MSIX is a vehicle for
`forge_ffi.dll` + the WinUI shell + the version-pinned `tsgo` sidecar. The Rust
core is identical to the binary `forge demo` exercises headlessly today.

---

## Table of contents

1. [Targets, layout, and tooling versions](#1-targets-layout-and-tooling-versions)
2. [Repository layout for the Windows app](#2-repository-layout-for-the-windows-app)
3. [Building forge-core for Windows (x64 + arm64)](#3-building-forge-core-for-windows-x64--arm64)
4. [MSIX packaging with Windows App SDK](#4-msix-packaging-with-windows-app-sdk)
5. [Code signing](#5-code-signing)
6. [Auto-update (PS-9)](#6-auto-update-ps-9)
7. [Installer story & distribution channels](#7-installer-story--distribution-channels)
8. [The conformance gate in packaging](#8-the-conformance-gate-in-packaging)
9. [CI: the Windows GitHub Actions job (`windows.yml`)](#9-ci-the-windows-github-actions-job-windowsyml)
10. [Exact command reference (copy-paste)](#10-exact-command-reference-copy-paste)
11. [Acceptance checks for this document](#11-acceptance-checks-for-this-document)

---

## 1. Targets, layout, and tooling versions

### 1.1 Build/run targets (PS-14)

| Axis | Values |
|---|---|
| OS | Windows 11 (all), Windows 10 22H2+ (build 19045+) |
| CPU | x64, arm64 |
| Rust targets | `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc` |
| WinUI runtime | Windows App SDK 1.6 (self-contained / framework-dependent — see §4.4) |

MSIX requires **per-architecture packages bundled into one `.msixbundle`**; there
is no fat binary on Windows. We build the Rust `cdylib` and the WinUI app **twice**
(x64, arm64) and merge into a bundle (§4.3).

### 1.2 Pinned toolchain versions

Pin everything; the conformance/replay guarantees (CR-12) require reproducible
binaries.

| Tool | Version | Notes |
|---|---|---|
| Rust | `1.96.0` (from `forge/rust-toolchain.toml`) | `cfg_select!` requirement of `libsqlite3-sys 0.38+`. |
| Visual Studio 2022 | 17.10+ | Workloads: **Desktop development with C++**, **.NET desktop**, **WinUI/Windows App SDK**. Components: **MSVC v143 (x64/x86)**, **MSVC v143 (ARM64)**, **C++ Clang tools for Windows (clang 18)**, **Windows 11 SDK 10.0.26100**. |
| .NET SDK | `8.0.x` (LTS) | WinUI 3 / Windows App SDK 1.6 target framework `net8.0-windows10.0.22621.0`. |
| Windows App SDK | `1.6.x` (NuGet `Microsoft.WindowsAppSDK`) | MSIX packaging + self-contained deploy. |
| MSIX tooling | `makeappx.exe`, `signtool.exe` (Windows 11 SDK 10.0.26100) | On `PATH` via the SDK bin dir. |
| LLVM/clang | `18.x` | Needed by `rquickjs` (QuickJS C) and `rusqlite` bundled SQLite C build under MSVC. Ships with VS "C++ Clang tools". |
| Node | `24` | Only for the small packaging/manifest helper scripts (mirrors existing repo `tools/*.mjs` pattern). |

> **Why clang:** `forge-runtime` pulls `rquickjs 0.12` which compiles QuickJS (C),
> and `forge-storage` uses `rusqlite 0.40.1` with `features=["bundled"]` which
> compiles SQLite (C). On `*-pc-windows-msvc` both build with the MSVC toolchain;
> `rquickjs` builds cleaner with `clang-cl`. Install the VS "C++ Clang tools"
> component so `libclang` is discoverable, and set `LIBCLANG_PATH` if `bindgen`
> (transitively via `rquickjs`) cannot locate it. See `01-BUILD-AND-FFI.md` §2.

---

## 2. Repository layout for the Windows app

This plan adds a `windows/` tree alongside `forge/`. Nothing under `forge/`
changes except the new `forge-ffi` crate (defined in `01-BUILD-AND-FFI.md`).

```
terrane/
  forge/                      # the Rust workspace (unchanged; thin-shell rule)
    crates/
      ffi/                    # forge-ffi: the cdylib, exports forge_ffi.dll  (01-BUILD-AND-FFI)
      ...
  windows/
    Forge.sln                 # VS solution: shell + tests
    Forge.Shell/              # the WinUI 3 app  (02-WINUI-RENDERER)
      Forge.Shell.csproj
      Package.appxmanifest    # MSIX manifest (identity, capabilities, forge:// URI)
      Assets/                 # Square44x44Logo etc. (Store-required tiles)
      runtimes/               # native dll dropped here per-RID by the build (see §3.4)
        win-x64/native/forge_ffi.dll
        win-arm64/native/forge_ffi.dll
      sidecar/                # version-pinned tsgo (CR-15) bundled into the package
    Forge.Conformance/        # console runner: golden trees + engine vectors (UI-14 / CR-12)
      Forge.Conformance.csproj
    Forge.Tests/              # C# unit tests for the binding marshaling (xUnit)
    packaging/
      mapping.x64.txt         # makeappx mapping files (if not using MSBuild MSIX)
      mapping.arm64.txt
      bundle.ps1              # builds the .msixbundle from per-arch .msix
      sign.ps1                # signtool wrapper
      appinstaller.ps1        # generates Forge.appinstaller for sideload auto-update
  .github/workflows/
    windows.yml               # the CI job defined in §9
  tools/
    win-package.mjs           # Node helper: version stamping + manifest validation
```

---

## 3. Building forge-core for Windows (x64 + arm64)

Full detail of the FFI crate is in `01-BUILD-AND-FFI.md`; here is the **packaging
contract**: what artifacts the package step consumes.

### 3.1 The cdylib crate

`forge-ffi` (`forge/crates/ffi/Cargo.toml`) declares:

```toml
[lib]
name = "forge_ffi"
crate-type = ["cdylib"]   # -> forge_ffi.dll on Windows
```

It re-exports `forge_core::WorkspaceCore::handle` behind a stable C-ABI
(`forge_handle`, `forge_subscribe`, `forge_free` — see `01-BUILD-AND-FFI.md` §3).
The shell never links the other crates directly; it links **only**
`forge_ffi.dll`. The dll statically contains SQLite (rusqlite bundled) and QuickJS
(rquickjs) — there is no separate SQLite or QuickJS dll (PS-3 / `03-PLATFORM-SERVICES.md` §SQLite).

### 3.2 Install Rust targets (one-time on the build machine / CI)

```powershell
rustup toolchain install 1.96.0
rustup target add x86_64-pc-windows-msvc  --toolchain 1.96.0
rustup target add aarch64-pc-windows-msvc --toolchain 1.96.0
```

### 3.3 Build commands (release, per arch)

Run from the repo root (`cargo` discovers `forge/Cargo.toml` via `--manifest-path`):

```powershell
# x64
cargo build --release --manifest-path forge/Cargo.toml `
  -p forge-ffi --target x86_64-pc-windows-msvc
# -> forge/target/x86_64-pc-windows-msvc/release/forge_ffi.dll

# arm64
cargo build --release --manifest-path forge/Cargo.toml `
  -p forge-ffi --target aarch64-pc-windows-msvc
# -> forge/target/aarch64-pc-windows-msvc/release/forge_ffi.dll
```

> **arm64 native vs cross-build.** On an x64 CI runner, `aarch64-pc-windows-msvc`
> is a **cross-compile**: linking needs the ARM64 MSVC toolset (the "MSVC v143 -
> ARM64 build tools" VS component) and the ARM64 Windows SDK libs. The
> GitHub-hosted `windows-2022` image ships these. The C deps (SQLite, QuickJS)
> cross-compile under `clang-cl --target=arm64-pc-windows-msvc`; if `cc`/`cmake`
> picks the wrong target, set `CC_aarch64_pc_windows_msvc=clang-cl` and
> `CFLAGS_aarch64_pc_windows_msvc=--target=arm64-pc-windows-msvc`. This is the
> single highest-risk item in Windows packaging; the CI matrix (§9) builds arm64
> on every run so regressions surface immediately.

### 3.4 Staging the dll for the package

Copy each dll into the per-RID `runtimes/` folder the `.csproj` references
(§4.2). A small step (PowerShell or `tools/win-package.mjs`):

```powershell
Copy-Item forge/target/x86_64-pc-windows-msvc/release/forge_ffi.dll  `
          windows/Forge.Shell/runtimes/win-x64/native/   -Force
Copy-Item forge/target/aarch64-pc-windows-msvc/release/forge_ffi.dll `
          windows/Forge.Shell/runtimes/win-arm64/native/ -Force
```

### 3.5 Core tests (gate before packaging)

The dll only ships if the Rust core is green (the same suites `forge demo` and CI
run today):

```powershell
cargo test --release --manifest-path forge/Cargo.toml --workspace
cargo run  --release --manifest-path forge/Cargo.toml -p forge-cli -- demo
```

`forge demo` must exit `0` (run succeeded **and** replay is byte-identical — see
`forge/crates/cli/src/main.rs`). A non-zero exit blocks the package.

---

## 4. MSIX packaging with Windows App SDK

### 4.1 Why MSIX (PS-14)

MSIX is the "MSIX-class signed installer/updates" PS-14 asks for: declarative
manifest, clean install/uninstall, per-user or per-machine, capability
declarations, `forge://` URI activation, Store **and** sideload distribution, and
built-in differential auto-update via `.appinstaller`. We use MSIX as the primary
channel and document a Store path; Squirrel is **not** used (PS-9 says
"MSIX/Squirrel-class" — MSIX is the Windows-native choice and we standardize on it).

### 4.2 `.csproj` essentials

`Forge.Shell.csproj` is a packaged WinUI 3 app. Key properties (versions per §1.2):

```xml
<Project Sdk="Microsoft.NET.Sdk">
  <PropertyGroup>
    <OutputType>WinExe</OutputType>
    <TargetFramework>net8.0-windows10.0.22621.0</TargetFramework>
    <TargetPlatformMinVersion>10.0.19041.0</TargetPlatformMinVersion> <!-- Win10 22H2 -->
    <RuntimeIdentifiers>win-x64;win-arm64</RuntimeIdentifiers>
    <UseWinUI>true</UseWinUI>
    <WindowsPackageType>MSIX</WindowsPackageType>
    <EnableMsixTooling>true</EnableMsixTooling>
    <WindowsAppSDKSelfContained>true</WindowsAppSDKSelfContained> <!-- §4.4 -->
    <Platforms>x64;arm64</Platforms>
    <AppxBundle>Always</AppxBundle>
    <AppxBundlePlatforms>x64|arm64</AppxBundlePlatforms>
  </PropertyGroup>

  <ItemGroup>
    <PackageReference Include="Microsoft.WindowsAppSDK" Version="1.6.241114003" />
    <PackageReference Include="Microsoft.Windows.SDK.BuildTools" Version="10.0.26100.1742" />
  </ItemGroup>

  <!-- Ship the native core per-RID; it lands next to the app in the package. -->
  <ItemGroup>
    <Content Include="runtimes\win-x64\native\forge_ffi.dll"
             Condition="'$(RuntimeIdentifier)'=='win-x64'">
      <CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory>
      <Link>forge_ffi.dll</Link>
    </Content>
    <Content Include="runtimes\win-arm64\native\forge_ffi.dll"
             Condition="'$(RuntimeIdentifier)'=='win-arm64'">
      <CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory>
      <Link>forge_ffi.dll</Link>
    </Content>
    <!-- Version-pinned tsgo sidecar (CR-15); see 03-PLATFORM-SERVICES §type-check -->
    <Content Include="sidecar\**\*">
      <CopyToOutputDirectory>PreserveNewest</CopyToOutputDirectory>
    </Content>
  </ItemGroup>
</Project>
```

### 4.3 `Package.appxmanifest` essentials

```xml
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
         xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
         xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities">
  <Identity Name="Terrane.Forge"
            Publisher="CN=Terrane, O=Terrane, C=..."   <!-- MUST match signing cert subject -->
            Version="0.1.0.0" />                         <!-- stamped by CI; see §6.3 -->
  <Properties>
    <DisplayName>Forge</DisplayName>
    <PublisherDisplayName>Terrane</PublisherDisplayName>
    <Logo>Assets\StoreLogo.png</Logo>
  </Properties>
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop"
                        MinVersion="10.0.19045.0"        <!-- Win10 22H2 -->
                        MaxVersionTested="10.0.26100.0" />
  </Dependencies>
  <Applications>
    <Application Id="Forge" Executable="Forge.Shell.exe"
                 EntryPoint="$targetentrypoint$">
      <uap:VisualElements DisplayName="Forge" Description="Local-first workspace"
        Square150x150Logo="Assets\Square150x150Logo.png"
        Square44x44Logo="Assets\Square44x44Logo.png"
        BackgroundColor="transparent" />
      <Extensions>
        <!-- forge:// deep links (PS-3); handled in 03-PLATFORM-SERVICES -->
        <uap:Extension Category="windows.protocol">
          <uap:Protocol Name="forge">
            <uap:DisplayName>Forge link</uap:DisplayName>
          </uap:Protocol>
        </uap:Extension>
      </Extensions>
    </Application>
  </Applications>
  <Capabilities>
    <!-- Embedded-server mode opens a loopback/LAN port -> firewall prompt (SS-15). -->
    <Capability Name="privateNetworkClientServer" />
    <Capability Name="internetClientServer" />
  </Capabilities>
</Package>
```

> No broad `<rescap>` capabilities are required for the spine: SQLite lives inside
> the package container, secrets use Credential Manager/DPAPI (per-user, no
> capability), and file access is via `FileOpenPicker` broker (handles, not paths —
> CR-3). Networking capability is declared **only** for embedded-server mode.

### 4.4 Self-contained vs framework-dependent

Set `WindowsAppSDKSelfContained=true` (§4.2). This embeds the Windows App SDK
runtime in the package so users do not need a separately installed framework
package — simpler for sideload + auto-update and avoids a runtime-version mismatch
class of bugs. Trade-off: larger package (~tens of MB). For the **Store** channel
we may switch to framework-dependent to share the runtime; both are produced from
the same source by toggling the property in the build matrix.

### 4.5 Build the per-arch MSIX and the bundle

```powershell
# Restore + build each arch as a packaged MSIX
msbuild windows\Forge.sln /restore `
  /p:Configuration=Release /p:Platform=x64   /p:AppxPackageDir=out\x64\ `
  /p:UapAppxPackageBuildMode=SideloadOnly /p:GenerateAppxPackageOnBuild=true

msbuild windows\Forge.sln /restore `
  /p:Configuration=Release /p:Platform=arm64 /p:AppxPackageDir=out\arm64\ `
  /p:UapAppxPackageBuildMode=SideloadOnly /p:GenerateAppxPackageOnBuild=true

# Merge x64 + arm64 into one .msixbundle (single download, OS picks the arch)
makeappx bundle /d out\ /p out\Forge_0.1.0.0.msixbundle /o
```

`UapAppxPackageBuildMode=SideloadOnly` produces an unsigned `.msix`/`.msixbundle`
for our own signing in §5. For Store submission, use `StoreUpload` (Store re-signs).

---

## 5. Code signing

### 5.1 What must be signed

The `.msix`/`.msixbundle` (and, for sideload auto-update, the `.appinstaller`'s
referenced packages). Signing must use a cert whose **subject exactly matches**
`Identity/@Publisher` in the manifest (`CN=Terrane, O=Terrane, C=...`), or
`signtool` rejects it.

### 5.2 Cert options

| Channel | Cert | Notes |
|---|---|---|
| Microsoft Store | none needed | Store signs on submission with your Publisher identity. |
| Direct download / enterprise sideload | **EV or OV code-signing cert** (DigiCert/Sectigo) | OV triggers SmartScreen reputation building; EV gets immediate SmartScreen trust. Strongly prefer a cert stored in an **HSM / Azure Key Vault** — never a `.pfx` in the repo. |
| Internal dev/test only | self-signed | Users must trust the cert manually; **never** for release. |

### 5.3 Sign command (Key Vault, recommended for CI)

Use `AzureSignTool` (so the private key never leaves the HSM):

```powershell
dotnet tool install --global AzureSignTool
AzureSignTool sign `
  -kvu "https://forge-signing.vault.azure.net" `
  -kvc "forge-codesign-cert" `
  -kvm `                                  # managed identity / OIDC, no secret on disk
  -tr "http://timestamp.digicert.com" -td sha256 -fd sha256 `
  out\Forge_0.1.0.0.msixbundle
```

Classic `signtool` equivalent (cert already in the machine store):

```powershell
signtool sign /fd SHA256 /a /tr http://timestamp.digicert.com /td SHA256 `
  out\Forge_0.1.0.0.msixbundle
```

**Always timestamp** (`/tr`): otherwise the signature expires when the cert does
and installed apps fail validation.

### 5.4 Verify

```powershell
signtool verify /pa /v out\Forge_0.1.0.0.msixbundle
```

---

## 6. Auto-update (PS-9)

PS-9 requirement, restated for Windows: *signed update mechanism; **the embedded
server drains connections before restart**.* Two layers: the MSIX delivery
mechanism, and the in-app graceful-restart coordination.

### 6.1 Delivery mechanism

| Channel | Update mechanism |
|---|---|
| Microsoft Store | Store handles update detection + differential download + install automatically. |
| Direct / sideload | **App Installer** (`.appinstaller` XML) — Windows polls the URL, downloads only changed blocks (MSIX block-map diff), installs on next launch. This is the "MSIX/Squirrel-class" auto-update PS-9 names, done the Windows-native way. |

`Forge.appinstaller` (hosted at a stable HTTPS URL, e.g. `https://dl.terrane.app/win/Forge.appinstaller`):

```xml
<?xml version="1.0" encoding="utf-8"?>
<AppInstaller Uri="https://dl.terrane.app/win/Forge.appinstaller"
              Version="0.1.0.0"
              xmlns="http://schemas.microsoft.com/appx/appinstaller/2021">
  <MainBundle Name="Terrane.Forge" Version="0.1.0.0"
              Publisher="CN=Terrane, O=Terrane, C=..."
              Uri="https://dl.terrane.app/win/Forge_0.1.0.0.msixbundle" />
  <UpdateSettings>
    <OnLaunch HoursBetweenUpdateChecks="8" UpdateBlocksActivation="false"
              ShowPrompt="true" />
    <AutomaticBackgroundTask />
    <ForceUpdateFromAnyVersion>false</ForceUpdateFromAnyVersion>
  </UpdateSettings>
</AppInstaller>
```

The first install is a single click on this `.appinstaller`; every later release
just updates the `Version` + `Uri` and re-uploads the bundle. `UpdateBlocksActivation=false`
+ `ShowPrompt=true` means the user keeps working and is offered the update — the
shell then coordinates the drain (§6.2).

### 6.2 Graceful restart + embedded-server drain (PS-9)

MSIX update **installs** while the old version may still be running; the new code
takes effect on **next launch**. The shell must therefore, before it exits to let
the update apply:

1. **Stop accepting new connections** on the embedded server (if running —
   `03-PLATFORM-SERVICES.md` §embedded-server). Route the core through the
   existing facade: `sync.stop` / server-shutdown command (no shell-side socket
   logic — thin-shell rule).
2. **Drain in-flight requests**: wait (bounded, e.g. 30 s) for active sync/HTTP
   exchanges to complete; new peers see "server updating, retry shortly".
3. **Flush + checkpoint storage**: SQLite WAL checkpoint via a core command so no
   data is in-flight (the core owns all writes — CR-A1).
4. **Persist UI/session state** so reopening restores the same workspace/page.
5. **Exit**; App Installer applies the pending update; relaunch.

Sketch of the C# update coordinator (calls only the core facade + WinAppSDK update API):

```csharp
// Forge.Shell/Services/UpdateCoordinator.cs  (orchestration only — no business logic)
async Task ApplyPendingUpdateAsync() {
    var pm = new PackageManager();
    // WinAppSDK: check the AppInstaller source for an available update.
    var avail = await Package.Current.CheckUpdateAvailabilityAsync();
    if (avail.Availability != PackageUpdateAvailability.Available) return;

    // PS-9 drain sequence — all state changes go through the core facade.
    await _core.HandleAsync("""{ "name":"sync.stop", ... }""");        // 1. stop intake
    await _server.DrainAsync(TimeSpan.FromSeconds(30));                 // 2. bounded drain
    await _core.HandleAsync("""{ "name":"workspace.checkpoint", ... }"""); // 3. flush WAL
    _session.Persist();                                                // 4. session state
    // 5. trigger the deferred-registration update + relaunch
    await pm.AddPackageByAppInstallerFileAsync(
        new Uri("https://dl.terrane.app/win/Forge.appinstaller"),
        AddPackageByAppInstallerOptions.ForceTargetAppShutdown, null);
}
```

> `ForceTargetAppShutdown` only fires **after** the drain completes, so no
> connection is severed mid-exchange. If the drain times out, surface a
> "finish syncing then update" prompt rather than killing live peers.

### 6.3 Version stamping (single source of truth)

The package `Version` (`a.b.c.0`) is derived in CI from the git tag (`v0.1.0` →
`0.1.0.0`) and written into **both** `Package.appxmanifest/Identity/@Version` and
`Forge.appinstaller/@Version` by `tools/win-package.mjs`, so the three never drift.
The Rust core version stays `forge/Cargo.toml`'s `workspace.package.version`
(`0.1.0`); CI asserts the major.minor match.

---

## 7. Installer story & distribution channels

| Channel | Artifact | Install UX | Update |
|---|---|---|---|
| **Microsoft Store** | `.msixupload` (StoreUpload mode) | Store listing, one click | Store-managed |
| **Direct download** | `Forge.appinstaller` + signed `.msixbundle` | Click `.appinstaller` → App Installer UI → Install | `.appinstaller` auto-update (§6.1) |
| **Enterprise / MDM** | signed `.msixbundle` | Intune/SCCM deploy | MDM-managed |
| **CI / smoke** | unsigned `.msixbundle` | dev-mode sideload (`Add-AppxPackage`) | n/a |

There is **no legacy MSI/EXE installer**; MSIX is the single packaging format
(PS-14). Users on Win10 22H2+ already have App Installer; we link the SDK redist if
absent.

---

## 8. The conformance gate in packaging

PS-4 + CR-12 + UI-14 make conformance **release-blocking**. A package is not
publishable unless, on the actual built artifacts:

1. **Engine conformance (CR-12):** the covered vectors in
	   `forge/fixtures/conformance-engines/*.json` (format in
	   `forge/spec/conformance-vector-format.md`) run on QuickJS-native **inside the
	   Windows dll** and produce byte-identical `RunRecord` fingerprints. The broader
	   `forge/fixtures/conformance/*.json` files remain runtime/host-API seed vectors
	   unless promoted into the engine-agnostic CR-12 harness. Exercised by the
	   `forge-runtime` conformance tests on the Windows runner.
2. **Renderer conformance (UI-14):** the golden trees under
   `forge/crates/ui/tests/golden/` (e.g. `roundtrip_*`, `diff_*`, `unknown_*`) are
   replayed through the **C# WinUI renderer** by the `Forge.Conformance` runner
   (defined in `02-WINUI-RENDERER.md`) and must match. This catches renderer
   divergence — the same bar as CR-12. The `unknown_*` fixtures assert UI-6
   fallback (no crash on unknown component types).
3. **Data fixtures (DL/09):** the workspace export round-trip
   (`forge/spec/workspace-export-format.md`) opens identically — same workspace
   file opens on every shipped platform (PRD 06 §9).
4. **Platform smoke:** the built MSIX installs in CI and launches the demo
   workspace headlessly (the WinUI equivalent of `forge demo` — `05-MILESTONES.md`
   W2 acceptance).

If any gate fails, the workflow fails before the artifact-upload/signing step. This
is enforced structurally in `windows.yml` (§9): the `package` job `needs:` the
`conformance` job.

---

## 9. CI: the Windows GitHub Actions job (`windows.yml`)

Mirrors the existing repo patterns (`.github/workflows/ci.yml` Windows job;
`release.yml` macOS DMG job → artifact upload + GitHub Release) but targets the new
`forge/` Rust workspace and the `windows/` shell — **not** the legacy v0.4 implementation
paths. Runner: `windows-2022` (ships VS 2022 + ARM64 toolset + Win11 SDK).

### 9.1 Job graph

```
core-test (x64)  ┐
build-core ──────┤ (matrix: x64, arm64)
                 ├─► conformance ──► build-app (matrix) ──► package ──► (release: sign + upload)
```

`package` depends on `conformance` so the gate in §8 is structural.

### 9.2 `.github/workflows/windows.yml`

```yaml
name: Windows

on:
  push:
    tags: ["v*"]
  pull_request:
    paths: ["forge/**", "windows/**", ".github/workflows/windows.yml"]
  workflow_dispatch:
    inputs:
      tag:
        description: "Release tag, e.g. v0.1.0"
        required: false

permissions:
  contents: write   # to attach artifacts to a GitHub Release on tag

env:
  RUST_TOOLCHAIN: "1.96.0"
  CONFIGURATION: Release

jobs:
  # ---------- 1. Rust core: test once + build the cdylib per arch ----------
  core:
    name: forge-core (${{ matrix.arch }})
    runs-on: windows-2022
    timeout-minutes: 40
    strategy:
      fail-fast: false
      matrix:
        include:
          - arch: x64
            rust_target: x86_64-pc-windows-msvc
            rid: win-x64
          - arch: arm64
            rust_target: aarch64-pc-windows-msvc
            rid: win-arm64
    steps:
      - uses: actions/checkout@v4

      - name: Install Rust ${{ env.RUST_TOOLCHAIN }} + target
        shell: pwsh
        run: |
          rustup toolchain install $env:RUST_TOOLCHAIN --profile minimal
          rustup target add ${{ matrix.rust_target }} --toolchain $env:RUST_TOOLCHAIN
          rustup component add clippy --toolchain $env:RUST_TOOLCHAIN

      - name: Cache cargo
        uses: actions/cache@v4
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            forge/target
          key: cargo-${{ matrix.rust_target }}-${{ hashFiles('forge/Cargo.lock') }}

      # rquickjs (QuickJS C) + rusqlite bundled (SQLite C) build under MSVC/clang.
      - name: Set up MSVC dev shell
        uses: ilammy/msvc-dev-cmd@v1
        with:
          arch: ${{ matrix.arch == 'arm64' && 'amd64_arm64' || 'amd64' }}

      # cargo test runs natively only on x64 (arm64 binaries can't execute on the
      # x64 runner); arm64 we only cross-build the cdylib. Engine/renderer
      # conformance therefore runs on the x64 build in the `conformance` job.
      - name: cargo test (x64 only)
        if: matrix.arch == 'x64'
        shell: pwsh
        run: cargo test --release --manifest-path forge/Cargo.toml --workspace

      - name: forge demo smoke (x64 only)
        if: matrix.arch == 'x64'
        shell: pwsh
        run: cargo run --release --manifest-path forge/Cargo.toml -p forge-cli -- demo

      - name: Build forge_ffi.dll
        shell: pwsh
        env:
          # Help the C cross-build pick the right clang target on arm64.
          CC_aarch64_pc_windows_msvc: clang-cl
          CFLAGS_aarch64_pc_windows_msvc: --target=arm64-pc-windows-msvc
        run: |
          cargo build --release --manifest-path forge/Cargo.toml `
            -p forge-ffi --target ${{ matrix.rust_target }}

      - name: Upload dll
        uses: actions/upload-artifact@v4
        with:
          name: forge_ffi-${{ matrix.rid }}
          path: forge/target/${{ matrix.rust_target }}/release/forge_ffi.dll

  # ---------- 2. Conformance gate (PS-4 / CR-12 / UI-14) ----------
  conformance:
    name: Conformance kit (engine + renderer)
    runs-on: windows-2022
    needs: core
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { name: forge_ffi-win-x64, path: windows/Forge.Shell/runtimes/win-x64/native }
      - uses: actions/setup-dotnet@v4
        with: { dotnet-version: "8.0.x" }

      # Renderer conformance: replay forge/crates/ui/tests/golden/* through the
      # C# WinUI renderer (UI-14) and assert visual match + UI-6 fallback.
      - name: Renderer conformance (UI-14)
        shell: pwsh
        run: |
          dotnet run -c Release --project windows/Forge.Conformance -- `
            --golden forge/crates/ui/tests/golden `
            --enforce
	      # Covered-vector engine conformance (CR-12) already ran inside the Rust
	      # runtime tests on the x64 core job; this job re-asserts the renderer half.

  # ---------- 3. Build the WinUI app per arch ----------
  build-app:
    name: Forge.Shell (${{ matrix.arch }})
    runs-on: windows-2022
    needs: conformance
    timeout-minutes: 40
    strategy:
      fail-fast: false
      matrix:
        arch: [x64, arm64]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-dotnet@v4
        with: { dotnet-version: "8.0.x" }
      - uses: actions/setup-node@v4
        with: { node-version: 24 }
      - uses: actions/download-artifact@v4
        with:
          name: forge_ffi-win-${{ matrix.arch }}
          path: windows/Forge.Shell/runtimes/win-${{ matrix.arch }}/native

      - name: Stamp version from tag
        if: startsWith(github.ref, 'refs/tags/v')
        shell: pwsh
        run: node tools/win-package.mjs --stamp-version "${{ github.ref_name }}"

      - name: Build packaged MSIX
        shell: pwsh
        run: |
          msbuild windows\Forge.sln /restore `
            /p:Configuration=$env:CONFIGURATION /p:Platform=${{ matrix.arch }} `
            /p:AppxPackageDir=$pwd\out\${{ matrix.arch }}\ `
            /p:UapAppxPackageBuildMode=SideloadOnly `
            /p:GenerateAppxPackageOnBuild=true

      - name: Upload per-arch MSIX
        uses: actions/upload-artifact@v4
        with:
          name: msix-${{ matrix.arch }}
          path: out/${{ matrix.arch }}/**/*.msix

  # ---------- 4. Bundle + (on tag) sign + smoke + release ----------
  package:
    name: MSIX bundle + sign + release
    runs-on: windows-2022
    needs: build-app
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v4
      - uses: actions/download-artifact@v4
        with: { pattern: "msix-*", path: out, merge-multiple: true }

      - name: Bundle x64 + arm64 into one .msixbundle
        shell: pwsh
        run: |
          $sdk = "${env:ProgramFiles(x86)}\Windows Kits\10\bin\10.0.26100.0\x64"
          & "$sdk\makeappx.exe" bundle /d out /p out\Forge.msixbundle /o

      # Sideload smoke: install the unsigned bundle in dev mode and launch headless.
      - name: Install + smoke (x64 runner)
        shell: pwsh
        run: |
          Add-AppxPackage -Path out\Forge.msixbundle -AllowUnsigned
          node tools/win-package.mjs --smoke-launch   # WinUI 'forge demo' equiv (W2)

      - name: Sign (release only)
        if: startsWith(github.ref, 'refs/tags/v')
        shell: pwsh
        env:
          AZURE_CLIENT_ID:     ${{ secrets.SIGNING_CLIENT_ID }}
          AZURE_TENANT_ID:     ${{ secrets.SIGNING_TENANT_ID }}
          AZURE_CLIENT_SECRET: ${{ secrets.SIGNING_CLIENT_SECRET }}
        run: |
          dotnet tool install --global AzureSignTool
          AzureSignTool sign -kvu "${{ secrets.SIGNING_KV_URL }}" `
            -kvc "${{ secrets.SIGNING_CERT_NAME }}" `
            -kvi $env:AZURE_CLIENT_ID -kvt $env:AZURE_TENANT_ID -kvs $env:AZURE_CLIENT_SECRET `
            -tr http://timestamp.digicert.com -td sha256 -fd sha256 `
            out\Forge.msixbundle
          signtool verify /pa /v out\Forge.msixbundle

      - name: Generate Forge.appinstaller (release only)
        if: startsWith(github.ref, 'refs/tags/v')
        shell: pwsh
        run: node tools/win-package.mjs --appinstaller "${{ github.ref_name }}" --out out

      - name: Upload artifacts
        uses: actions/upload-artifact@v4
        with:
          name: windows-msix
          path: |
            out/Forge.msixbundle
            out/Forge.appinstaller

      - name: Attach to GitHub Release (release only)
        if: startsWith(github.ref, 'refs/tags/v')
        env:
          GH_TOKEN: ${{ github.token }}
          RELEASE_TAG: ${{ inputs.tag || github.ref_name }}
        shell: pwsh
        run: |
          gh release view "$env:RELEASE_TAG" 2>$null `
            || gh release create "$env:RELEASE_TAG" --title "$env:RELEASE_TAG" --notes "Forge $env:RELEASE_TAG"
          gh release upload "$env:RELEASE_TAG" out\Forge.msixbundle out\Forge.appinstaller --clobber
```

### 9.3 Notes on the workflow

- **Dual-arch matrix** is explicit in both `core` and `build-app` (`x64`, `arm64`),
  matching PS-14. arm64 is **built every run** but **tested on x64** (the runner is
  x64; arm64 binaries can't execute there). Native arm64 test execution is deferred
  to a self-hosted/Windows-arm64 runner — tracked as a risk in `05-MILESTONES.md`.
	- **Conformance is a hard `needs:` gate** — `build-app` cannot start until both
	  covered CR-12 engine vectors and renderer golden trees pass (§8).
- **Signing only on tags** (release), using **OIDC/Key Vault secrets** — no `.pfx`
  in the repo. PR builds produce unsigned, dev-sideloaded bundles for smoke.
- **`forge demo` smoke** reuses the exact spine acceptance the headless CLI already
  guarantees, so CI fails on the same conditions as local dev.
- Reuses the repo conventions you already have: `actions/checkout@v4`,
  `actions/setup-node@v4` (`node-version: 24`), `actions/upload-artifact@v4`, and
  the `gh release create/upload --clobber` pattern from `release.yml`.

---

## 10. Exact command reference (copy-paste)

For a developer on a Windows machine reproducing CI locally.

```powershell
# 0. one-time toolchain
rustup toolchain install 1.96.0
rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc --toolchain 1.96.0

# 1. core: test + spine smoke (must be green before packaging)
cargo test --release --manifest-path forge/Cargo.toml --workspace
cargo run  --release --manifest-path forge/Cargo.toml -p forge-cli -- demo   # exit 0 required

# 2. build the native dll, both arches
cargo build --release --manifest-path forge/Cargo.toml -p forge-ffi --target x86_64-pc-windows-msvc
cargo build --release --manifest-path forge/Cargo.toml -p forge-ffi --target aarch64-pc-windows-msvc

# 3. stage dlls
Copy-Item forge\target\x86_64-pc-windows-msvc\release\forge_ffi.dll  windows\Forge.Shell\runtimes\win-x64\native\  -Force
Copy-Item forge\target\aarch64-pc-windows-msvc\release\forge_ffi.dll windows\Forge.Shell\runtimes\win-arm64\native\ -Force

# 4. renderer conformance gate (UI-14)
dotnet run -c Release --project windows\Forge.Conformance -- --golden forge\crates\ui\tests\golden --enforce

# 5. build packaged MSIX per arch
msbuild windows\Forge.sln /restore /p:Configuration=Release /p:Platform=x64   /p:AppxPackageDir=out\x64\   /p:UapAppxPackageBuildMode=SideloadOnly /p:GenerateAppxPackageOnBuild=true
msbuild windows\Forge.sln /restore /p:Configuration=Release /p:Platform=arm64 /p:AppxPackageDir=out\arm64\ /p:UapAppxPackageBuildMode=SideloadOnly /p:GenerateAppxPackageOnBuild=true

# 6. bundle
makeappx bundle /d out /p out\Forge.msixbundle /o

# 7. sign + verify (release)
AzureSignTool sign -kvu https://forge-signing.vault.azure.net -kvc forge-codesign-cert -kvm `
  -tr http://timestamp.digicert.com -td sha256 -fd sha256 out\Forge.msixbundle
signtool verify /pa /v out\Forge.msixbundle

# 8. local install + smoke
Add-AppxPackage -Path out\Forge.msixbundle   # add -AllowUnsigned for the unsigned dev bundle
```

---

## 11. Acceptance checks for this document

A reviewer/implementer confirms W5 (`05-MILESTONES.md`) is met when **all** hold:

1. **Dual-arch build:** `cargo build -p forge-ffi` succeeds for **both**
   `x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`; both `forge_ffi.dll`
   files exist (§3.3).
2. **Core green:** `cargo test --workspace` passes and `forge demo` exits `0` on the
   Windows runner (§3.5).
3. **Conformance gate:** the `conformance` job passes — covered CR-12 engine vectors
	   (`forge/fixtures/conformance-engines/*.json`) byte-identical on the Windows dll **and**
   all golden trees in `forge/crates/ui/tests/golden/` (including `unknown_*` UI-6
   fallback) match through the C# renderer (§8). `build-app` does not run unless
   this passes.
4. **MSIX bundle:** `makeappx bundle` produces `Forge.msixbundle` containing **both**
   the x64 and arm64 packages (§4.5); `Get-AppxPackage` after install reports the
   matching architecture on each machine.
5. **Signed + verified:** on a tag build, `signtool verify /pa` returns success and
   the cert subject equals `Package.appxmanifest/Identity/@Publisher` (§5).
6. **Install + launch:** `Add-AppxPackage out\Forge.msixbundle` installs on Win11
   and Win10 22H2+, x64 and arm64, and the app launches into the demo workspace
   (smoke step, §9.2 / §8.4).
7. **Auto-update path:** a `Version`-bumped `Forge.appinstaller` triggers an update
   on next launch; before relaunch the shell stops intake, drains the embedded
   server within the bound, and checkpoints storage (PS-9, §6.2) — verified by
   pointing a peer at the server during an update and observing no severed
   in-flight exchange.
8. **CI wired:** `.github/workflows/windows.yml` runs on PRs touching `forge/**`
   or `windows/**` and on `v*` tags; the `package` job `needs: conformance`
   (structural gate); release artifacts (`Forge.msixbundle`, `Forge.appinstaller`)
   attach to the GitHub Release (§9).
9. **Version single-source:** `tools/win-package.mjs --stamp-version vX.Y.Z` sets
   the manifest, `.appinstaller`, and asserts major.minor against
   `forge/Cargo.toml` (§6.3).

> **Tauri fallback note (PS-15):** if WinUI 3 packaging/maturity blows the budget at
> the M3 gate, the fallback (Tauri 2 reusing the web renderer) swaps §3–§4 for
> Tauri's bundler (`cargo tauri build` → MSI/NSIS or MSIX via the same `signtool`
> path) while §5 (signing), §6 (App Installer auto-update + drain), §8 (conformance
> gate), and §9 (CI matrix) carry over largely unchanged. See `05-MILESTONES.md`
> for the decision criteria.
