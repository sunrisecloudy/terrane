# Windows app plan for forge

The complete, implementable plan for the **forge** (Terrane) Windows desktop shell:
a thin C#/WinUI 3 renderer + platform services over the existing, working `forge-core`
Rust workspace at `~/projects/terrane/forge/` inside WSL. No business logic lives in
the shell — every state change goes through the core's `Command`/`Event`/`Stream`
facade (`WorkspaceCore::handle`), exposed across a stable C-ABI seam (`forge_ffi.dll`),
and the UI-tree + patch protocol is rendered natively in WinUI 3. Targets Windows 11
and Windows 10 22H2+, x64 + arm64, shipped as a signed MSIX (PRD 06 PS-14, roadmap M6).

## Executive summary (5 lines)

1. **The brain works; we build the body and skin.** The M0a spine (TS→SWC→QuickJS→ctx→SQLite→UI tree→deterministic replay) already runs headlessly via `forge demo`; Windows reuses it unchanged.
2. **One new Rust crate, `crates/ffi`** — a thin `cdylib` exposing 7 C symbols (JSON in, JSON out, `catch_unwind`, never panics across the boundary). It is the only mutating path the shell can reach.
3. **A WinUI 3 renderer** maps the ~26-component UI catalog + 5-op patch stream to a live `FrameworkElement` tree (shadow model + visual tree by index path), virtualizes with `ItemsRepeater`, and passes the shared golden corpus (UI-14).
4. **Platform services** (Credential Manager/DPAPI secrets, file-picker handles, toasts, `forge://` deep links, firewall for embedded server, `tsgo` sidecar) are injected effects — never shell-owned logic.
5. **Packaging** is a signed, x64+arm64 `.msixbundle` with `.appinstaller` auto-update and a `windows-latest` CI matrix where conformance is a structural, release-blocking gate.

## Index / table of contents

| # | Document | Covers |
|---|---|---|
| 00 | [00-OVERVIEW.md](./00-OVERVIEW.md) | Architecture, the thin-shell rule made enforceable, mapping to the headless spine, v1 scope (PS-14/PS-15), OS targets & SLOs, conformance gates, prerequisites |
| 01 | [01-BUILD-AND-FFI.md](./01-BUILD-AND-FFI.md) | Building `forge-core` as a Windows DLL, the C build deps, the binding decision (C-ABI vs UniFFI-C#), the 7-symbol C ABI, `Forge.Interop` P/Invoke, threading, `System.Text.Json` DTOs, the "hello core" walkthrough |
| 02 | [02-WINUI-RENDERER.md](./02-WINUI-RENDERER.md) | UI-tree → XAML factory (full catalog), the patch applier, event routing, UI-6 fallback, theming, `ItemsRepeater` virtualization (100k rows), a11y, the renderer conformance kit (UI-14) |
| 03 | [03-PLATFORM-SERVICES.md](./03-PLATFORM-SERVICES.md) | Secrets (Credential Manager + DPAPI), file pickers as handles, notifications, `forge://` deep links, embedded-server firewall, the `tsgo` type-check sidecar, `ctx.net` fetch; the capability map and M0a-vs-later matrix |
| 04 | [04-PACKAGING-CI.md](./04-PACKAGING-CI.md) | MSIX packaging (per-arch bundle), code signing, `.appinstaller` auto-update + server drain, distribution channels, the conformance gate in packaging, the `windows.yml` GitHub Actions job |
| 05 | [05-MILESTONES.md](./05-MILESTONES.md) | The ordered W0→W5 milestones, each with deliverables and one mechanical acceptance gate; the PS-15 Tauri-fallback decision gate at W1; the requirement map and risks |

## Prerequisites checklist

Install and verify on the Windows dev machine **before W0** (full table in 00-OVERVIEW §9):

- [ ] **WSL Ubuntu 24.04 checkout** lives in the Linux filesystem, not under `C:\` or `/mnt/c`.
      Recommended path: `~/projects/terrane`. From Windows, open it at
      `\\wsl$\Ubuntu-24.04\home\<linux-username>\projects\terrane`. Run build/test commands
      inside WSL; use the `\\wsl$` path for Windows editors and file browsing.
- [ ] **Visual Studio 2022** (17.10+) with workloads: *Desktop development with C++*, *.NET desktop development*, *Windows App SDK / WinUI 3*. Components: MSVC v143 (x64/x86 **and** ARM64) build tools, C++ Clang tools (clang 18), Windows 11 SDK 10.0.22621+.
- [ ] **Rust 1.96.0** (matches `forge/rust-toolchain.toml`): `rustup show` reports stable 1.96.0.
- [ ] **Rust targets** added: `rustup target add x86_64-pc-windows-msvc aarch64-pc-windows-msvc`.
- [ ] **.NET SDK 8.0.x** (LTS): `dotnet --version` reports 8.0.x.
- [ ] **Windows App SDK 1.6.x** (NuGet `Microsoft.WindowsAppSDK`).
- [ ] **LLVM/clang 18+** discoverable (`LIBCLANG_PATH` set if `bindgen` can't find it) — needed for `rquickjs` (QuickJS C) and `rusqlite` bundled SQLite C builds.
- [ ] **Node 24** for the small packaging/manifest helper scripts (`tools/win-package.mjs`).
- [ ] **Native-deps smoke** (retires the highest build risk): from a Developer PowerShell, `cargo build -p forge-storage -p forge-runtime --target x86_64-pc-windows-msvc` succeeds.

## Start here → W0

Begin with **[00-OVERVIEW.md](./00-OVERVIEW.md)** to absorb the architecture and the thin-shell
rule, then go straight to **[05-MILESTONES.md](./05-MILESTONES.md) → W0** ("Build `forge_ffi.dll`
on Windows + 'hello core' over the boundary"). W0 is the entry point: it proves the Rust core
compiles to a Windows native lib and a C# process can drive one real `CoreCommand` end to end.
Use **[01-BUILD-AND-FFI.md](./01-BUILD-AND-FFI.md)** as the detailed reference while implementing
W0, then proceed W1→W5, consulting docs 02 (renderer), 03 (platform services), and 04 (packaging/CI)
as each milestone calls for them. Every milestone ends in a single mechanical gate that must exit 0
before moving on.
