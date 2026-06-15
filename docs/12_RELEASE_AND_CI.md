# Release and CI Plan

## 1. Build outputs

The first version should produce:

- Forge FFI libraries for host/native targets.
- Runtime web static assets.
- Example webapp packages.
- Public contract JSON for downstream private products.
- Native host app builds.
- Server executable.

## 2. Development commands

Suggested commands:

```text
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p forge-cli -- demo
node tools/validate-webapp-package/main.js webapps/examples/notes-lite
node tools/package-examples/main.js
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
```

Platform commands will vary by host OS.

## 3. CI stages

### Stage 1: fast validation

- Validate schemas.
- Validate example manifests.
- Run package validator against examples.
- Run malicious fixture rejection tests.
- Run Forge workspace tests and clippy.
- Run runtime JS unit tests.
- Generate and validate the public contract export.
- Verify public contract provenance and recorded file hashes.

### Stage 2: build

- Build Forge FFI library artifacts.
- Build Forge server.
- Build runtime package.
- Build native shells where CI OS supports them.

### Stage 3: smoke

- Launch runtime in browser mock.
- Run example smoke tests.
- Run server API tests.
- Run platform smoke tests if runner supports UI.

## 4. Platform CI matrix

```text
ubuntu-24.04:
  Forge workspace tests
  Forge server tests
  runtime tests
  package validator
  Linux native release artifact package on ubuntu-24.04
  Docker-backed Linux shell build, release production-guard audit, and WebKitGTK smoke on ubuntu-24.04

macos-latest:
  Forge FFI macOS artifact build
  macOS shell build
  iOS simulator smoke

windows-2022:
  Forge FFI Windows build
  Windows shell build on windows-2022
  Windows native release artifact package on windows-2022
  WebView2 smoke with the pinned WebView2 SDK package

android emulator job:
  Android debug build
  emulator launch smoke
```

## 5. Release artifacts

The release artifact packager is:

```text
node --no-warnings tools/package-release.mjs --out artifacts --build-forge-ffi --build-server --build-native-macos
```

It produces deterministic archives for the build-free runtime and example packages, builds the host-target Forge FFI library plus `forge_ffi.h` when `--build-forge-ffi` is passed, builds the host-native Forge server executable for the current CI runner when `--build-server` is passed, builds a macOS `.app` host bundle plus a user-downloadable `.dmg` with runtime/example/database resources and `libforge_ffi.dylib` when `--build-native-macos` is passed on macOS, builds a Linux host app directory with runtime/example/database resources and `libforge_ffi.so` when `--build-native-linux` is passed on Linux, builds a Windows host app directory with runtime/example/database resources and `forge_ffi.dll` when `--build-native-windows` is passed on Windows, and writes a manifest that records hashes plus the target-specific directories populated by platform CI jobs.

```text
artifacts/
  forge-ffi/
    macos/macos-arm64/forge_ffi.h
    macos/macos-arm64/libforge_ffi.dylib
    macos/macos-arm64/libforge_ffi.a
  server/
    linux-x86_64/terrane-server
  runtime-web.zip
  example-webapps.zip
  public-contract.json
  release-manifest.json
  native-apps/
    macos/macos-arm64/terrane.app/
      Contents/Resources/runtime/
      Contents/Resources/webapps/examples/
      Contents/Resources/db/sqlite/
      Contents/Frameworks/libforge_ffi.dylib
    macos/macos-arm64/Terrane-macos-arm64.dmg
    linux/linux-x86_64/TerraneHost/
      terrane-host
      libforge_ffi.so
      resources/runtime/
      resources/webapps/examples/
      resources/db/sqlite/
    windows/windows-x86_64/TerraneHost/
      TerraneHost.exe
      forge_ffi.dll
      resources/runtime/
      resources/webapps/examples/
      resources/db/sqlite/
```

The macOS native app artifact path is `native-apps/macos/macos-arm64/terrane.app` on Apple Silicon CI runners. The user-downloadable macOS release asset is `native-apps/macos/macos-arm64/Terrane-macos-arm64.dmg`.

The Linux native app artifact path is `native-apps/linux/linux-x86_64/TerraneHost` on the `ubuntu-24.04` release runner.

The Windows native app artifact path is `native-apps/windows/windows-x86_64/TerraneHost` on the `windows-2022` release runner.

The dedicated Forge FFI artifact job runs:

```text
node --no-warnings tools/package-release.mjs --out artifacts --build-forge-ffi
```

The dedicated Linux server artifact job runs:

```text
node --no-warnings tools/package-release.mjs --out artifacts --build-server
```

The dedicated macOS native artifact job runs on `macos-latest`:

```text
node --no-warnings tools/package-release.mjs --out artifacts --build-native-macos
```

The `Release` workflow runs the same macOS packaging command for pushed `v*`
tags or manual dispatch, uploads the DMG as a workflow artifact, and attaches
`Terrane-*.dmg` plus `release-manifest.json` to the matching GitHub Release.

The dedicated Linux native artifact job runs on `ubuntu-24.04` after installing GTK4, WebKitGTK, JSON-GLib, SQLite, libsoup, Meson, Ninja, and pkg-config:

```text
node --no-warnings tools/package-release.mjs --out artifacts --build-native-linux
```

The dedicated Windows native artifact job runs on `windows-2022` after installing the pinned WebView2 SDK package:

```text
node --no-warnings tools/package-release.mjs --out artifacts --build-native-windows
```

The Windows native smoke job also builds the release host and verifies that production/release builds reject `--control-plane-port`, `--allow-runtime-mismatch`, and `--allow-unsigned-dev` while writing a `native.production_guard` audit record.

The Linux native smoke job runs through Docker so the WebKitGTK, GTK, SQLite, Meson, Ninja, Cargo toolchain, and SQLite CLI audit probe are all supplied by the checked-in container definition. It also builds the packaged Linux native artifact and launches it from outside the repo root without `TERRANE_FORGE_FFI_SO`, proving runtime resources, example apps, SQLite migrations, and `libforge_ffi.so` resolve relative to the executable:

```text
node --no-warnings tools/run-linux-native-docker.mjs
```

The default static packaging command without build flags still creates placeholder target directories for downstream jobs.

`public-contract.json` is the machine-readable contract consumed by downstream private products such as Terrane Premium. It lists the public docs, schemas, fixtures, tools, generated-app boundary, bridge methods, syncable record kinds, and non-syncable local record kinds that define generated-app-visible behavior. Downstream products should pin this artifact or the matching release manifest hash before adapting private APIs.

## 6. Versioning

Use independent but compatible versions:

- Platform runtime version: `0.1.0`.
- Zig core ABI version: `0.1.0`.
- Webapp runtime target version: `0.1.0`.

Generated apps declare `runtimeVersion` in manifest. Compatibility rule lives in docs/04 §8.

## 6.1 Runtime self-update

The WebView runtime is shipped *inside* each native host binary. There is no over-the-air runtime update in v0.4. Updates happen by:

| Host | Update mechanism |
|---|---|
| iOS App Store | New host binary via App Store update; the runtime is bundled in the IPA |
| iOS TestFlight / Developer-ID | Same as App Store but via the TestFlight / sideload flow |
| macOS App Store / Developer ID | Same as iOS via App Store / Sparkle |
| Android Play Store | Host APK update via Play |
| Windows | MSIX bundle update |
| Linux | Distro package or AppImage update |
| Server | Operator-driven Zig binary deployment |
| Reference host (dev) | `git pull` + restart |

When the runtime version changes, every installed generated app is re-evaluated against the new runtime semver rule (docs/04 §8). Apps that fail the rule are kept in storage but cannot mount until the user installs a compatible app version. The host shows a one-time banner listing incompatible apps after a runtime upgrade.

Rollback: if a runtime upgrade is itself broken, the user must reinstall the previous host binary. There is no in-product runtime rollback. The platform DB is preserved across host upgrades and downgrades; `db/sqlite/*.sql` migrations are append-only and skipped if already applied.

A future revision may add an OTA runtime patch channel for sideloaded builds. Until then, runtime updates and host updates are the same event.

## 7. Release checklist

- All examples pass validation.
- All examples run in browser mock.
- All platform shells tested at least manually.
- Bridge contract fixtures pass.
- Public contract export exists and its hash is recorded in `release-manifest.json`.
- Security fixtures rejected.
- Docs updated.
- Acceptance checklist completed.


## Control-plane CI

CI should include a headless control-plane test path even before native platform shells are fully automated.

Required jobs:

- `validate:schemas` — validate manifests, bridge schemas, micro-test files, and control protocol schemas.
- `test:runtime-headless` — run runtime tests in a browser/WebView-compatible environment.
- `test:mcp-contract` — run the MCP server against a fake control-plane server.
- `test:micro-examples` — run micro-tests for the five example webapps against the reference host.
- `test:desktop-smoke` — launch at least one desktop host and run all example app smoke tests.

Optional/manual jobs:

- iOS simulator smoke.
- Android emulator smoke.
- Release packaging/signing smoke.
- Store-specific submission dry runs.

The reference host is required so Codex can test the MCP server without needing every native toolchain installed.

## v0.3 CI additions

CI must add stages for:

1. schema validation for all examples and fixtures;
2. package validator mutation tests;
3. reference-host install/sign/mount tests;
4. snapshot/replay tests;
5. rollback/migration tests;
6. accessibility audit smoke tests;
7. network policy tests;
8. Codex MCP tool-contract tests.

Release builds must reject dev-only `none-dev` signatures and must package bundled examples as signed installed packages or sign them at first launch with a release-trusted bundled-app key.

## v0.4 database CI gates

Add CI jobs for:

- JSON parsing for all schema/fixture/test files.
- SQLite migration execution using in-memory SQLite.
- Required table/index assertions.
- Postgres schema static parity checks, with optional live apply when `POSTGRES_TEST_URL` is configured.
- DB fixture schema validation.
- App install transaction test.
- Storage CRUD test.
- Rollback test.
- Migration dry-run/apply test.
- Backup export/import round-trip test.

No release artifact should be accepted if the SQLite schema fails to apply or the app install transaction is broken.
