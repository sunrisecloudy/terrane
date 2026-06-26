# PRD 06 — Platform Shells (thin native apps over one core)

**Status:** Merged draft v1 · **Depends on:** 01–05 · **Order (decisions D1, D5):** CLI harness → renderer zero → macOS → web → Linux headless server → Windows → iOS → Android
**Sources:** F-06 (shell requirements, desktop/web/iOS/Android specifics) + P-03 (platform matrix, storage/keychain per platform, iOS risk controls) + decisions D1, D4, D5

All shells are thin: native UX chrome + renderer (PRD 05) + platform services, over the same core. **Shell code contains no business logic** — no shell may mutate storage, CRDT, permissions, schema, or runtime state except through commands (CR-A1); enforced in review and by the binding surface.

## 1. Shared shell requirements

- **PS-1** Bindings generated, never hand-written: UniFFI (Swift, Kotlin, C#), wasm-bindgen (web), C-ABI adapter where generated bindings aren't mature.
- **PS-2** Common surface: onboarding (no account required — D8), workspace switcher, applet launcher/manager (install, permissions, suspend, uninstall), editor + LLM panel + review UI, schema designer, data browser, settings (home-server picker, model manager, budgets, storage limits), diagnostics/debug screen, opt-in crash reporting.
- **PS-3** Platform services each shell provides to the core: secrets store (Keychain / Credential Manager+DPAPI / Keystore / Secret Service / WebCrypto-backed with documented browser caveats), file pickers returning **handles** (never raw paths to applets), OS permission prompts mapped to capabilities, notifications, deep links (`forge://`), share-sheet → LLM panel.
- **PS-4** Per-platform conformance gates before a shell ships: engine conformance (CR-12), renderer kit (UI-14), data fixtures (DL/09), platform smoke of the demo workspace.

## 2. Shell zero: CLI harness (M0)

- **PS-5** A Rust CLI speaking the full Command/Event/Stream contract: create workspace, install applet from disk, run pipeline stages, simulate UI events, assert golden trees, drive in-process client↔server sync, replay deterministic runs. It is the SDK's ancestor and the proof that the contract has no UI-shaped holes. Renderer zero (UI-13) attaches to it.

## 3. macOS (M1 — first real shell)

- **PS-6** Swift/SwiftUI shell (AppKit where needed), **JavaScriptCore engine (D4)**, native SQLite, SwiftUI renderer. Menu bar: New/Open/Export/Run/Stop/Time Travel/Sync/AI Generate (P-03).
- **PS-7** Embedded server mode (SS-15..18): settings toggle, menubar status item, port/relay config, backup schedule, pairing-QR display.
- **PS-8** Local model manager (LM-3) + LM Studio detection; `tsgo` sidecar supervision (CR-15): auto-restart, version-pinned.
- **PS-9** Auto-update: Sparkle, signed + notarized; embedded server drains connections before restart. Target: macOS 14+ (arm64 + x86_64).

## 4. Web (M3; renderer-zero lineage)

- **PS-10** Full client: core in WASM; SQLite-WASM on **OPFS** (worker-hosted) with IndexedDB-VFS fallback (flagged in diagnostics); QuickJS-WASM applet realms in workers; type-check via lazy-loaded `tsc` worker (CR-15); COOP/COEP for SharedArrayBuffer; installable PWA, offline after first load.
- **PS-11** Worker architecture (P-03): UI thread ↔ core worker ↔ storage worker / runtime worker / sync connection. Storage quotas and eviction surfaced in settings; persistent-storage failure triggers a "download backup" reminder; streaming export/import for large workspaces.
- **PS-12** Browser matrix: latest Chrome/Edge/Safari/Firefox. Web is also the no-install collaborator path and read-only share viewer (SS-14). Budget: ≤ 6 MB gzipped core initial load, TTI < 3 s mid-tier hardware.

## 5. Linux headless (v1, near-free)

- **PS-13** Same `server` crate as a CLI/daemon (SS-19): self-host sync, backups, marketplace mirror, LLM gateway. No Linux GUI in v1 (D5); revisit GTK4/gtk-rs post-GA.

## 6. Windows (v1.x fast-follow — M6)

- **PS-14** C#/WinUI 3 shell, QuickJS engine, Rust DLL via UniFFI-C#/C-ABI; Credential Manager/DPAPI secrets; MSIX-class signed installer/updates; firewall prompt handling for server mode. Targets: Windows 11 & 10 22H2+ (x64 + arm64).
- **PS-15** Decision gate: if WinUI estimate exceeds budget at M3 exit, fall back to Tauri 2 reusing the web renderer (F risk table).

## 7. iOS / iPadOS (v1.x — after Windows decision, M6)

- **PS-16** Swift/SwiftUI shell; **JavaScriptCore engine held to the covered CR-12 engine vectors as it lands (D4)**, converting the biggest engine risk into an explicit conformance gate alongside renderer + platform-services work.
- **PS-17** App Store 2.5.2 posture (F-06 + P-03 risk controls, PRD 07 §9): JSC execution path; applets are user-created, **source-visible and editable**; execution is user-requested; no hidden background code execution; no dynamic native code, no JIT dependency claims beyond JSC itself, public APIs only; **no public marketplace inside the iOS app at launch**; review-safety mode documented; TestFlight long-beta + PWA contingency (PS-10 already gives full iOS-Safari functionality).
- **PS-18** Mobile constraints: BGTaskScheduler best-effort sync with honest staleness UI; push via cloud relay for invites/mentions; on-device local models deferred (route local tier to home server); iPad split view first-class (P-03). Target: iOS 17+.

## 8. Android (v1.x — after iOS)

- **PS-19** Kotlin/Compose shell; QuickJS engine; UniFFI-Kotlin/JNI; app-private SQLite + Storage Access Framework handles; WorkManager sync; foreground service only for user-visible long-running server/sync; optional on-device small model on ≥ 8 GB devices behind a flag. Distribution: Play (Data Safety in CI) + direct APK. Target: Android 10+ (API 29), arm64.

## 9. Acceptance

- M0 exit: harness + renderer zero run the demo applet with the full loop green on macOS/Linux/WASM CI targets.
- One demo workspace exercised end-to-end on macOS (M1), then identically on web (M3), per conformance kits; same workspace file opens on every shipped platform.
- Cold start to interactive workspace: < 2 s desktop, < 3 s web, < 2.5 s mobile.
- iOS submitted with 2.5.2 rationale doc; approval is the M6 gate.

## 10. Open questions

1. Tauri fallback trigger criteria (proposal: > 2 engineer-quarters estimated for WinUI at M3 exit).
2. iPad as distinct layout target at iOS launch or after.
3. Linux GUI demand check post-GA.
