# Release and CI Plan

## 1. Build outputs

The first version should produce:

- Zig core libraries for all platform targets.
- Runtime web static assets.
- Example webapp packages.
- Native host app builds.
- Server executable.

## 2. Development commands

Suggested commands:

```text
zig build -Dtarget=native test
zig build -Dtarget=native run-server
node tools/validate-webapp-package/main.js webapps/examples/notes-lite
node tools/package-examples/main.js
```

Platform commands will vary by host OS.

## 3. CI stages

### Stage 1: fast validation

- Validate schemas.
- Validate example manifests.
- Run package validator against examples.
- Run malicious fixture rejection tests.
- Run Zig core tests.
- Run runtime JS unit tests.

### Stage 2: build

- Build Zig core native target.
- Cross-build Zig libraries where feasible.
- Build server.
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
  Zig core tests
  server tests
  runtime tests
  package validator
  Linux shell build and WebKitGTK smoke on ubuntu-24.04

macos-latest:
  Zig core macOS/iOS build
  macOS shell build
  iOS simulator smoke

windows-2022:
  Zig core Windows build
  Windows shell build on windows-2022
  WebView2 smoke with the pinned WebView2 SDK package

android emulator job:
  Android debug build
  emulator launch smoke
```

## 5. Release artifacts

The static release artifact packager is:

```text
node --no-warnings tools/package-release.mjs --out artifacts
```

It produces deterministic archives for the build-free runtime and example packages, plus a manifest that records hashes and the target-specific directories populated by platform CI jobs.

```text
artifacts/
  zig-core/
    ios/
    macos/
    android/
    windows/
    linux/
  runtime-web.zip
  example-webapps.zip
  release-manifest.json
  server/
  native-apps/
```

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
| Fake host (dev) | `git pull` + restart |

When the runtime version changes, every installed generated app is re-evaluated against the new runtime semver rule (docs/04 §8). Apps that fail the rule are kept in storage but cannot mount until the user installs a compatible app version. The host shows a one-time banner listing incompatible apps after a runtime upgrade.

Rollback: if a runtime upgrade is itself broken, the user must reinstall the previous host binary. There is no in-product runtime rollback. The platform DB is preserved across host upgrades and downgrades; `db/sqlite/*.sql` migrations are append-only and skipped if already applied.

A future revision may add an OTA runtime patch channel for sideloaded builds. Until then, runtime updates and host updates are the same event.

## 7. Release checklist

- All examples pass validation.
- All examples run in browser mock.
- All platform shells tested at least manually.
- Bridge contract fixtures pass.
- Security fixtures rejected.
- Docs updated.
- Acceptance checklist completed.


## Control-plane CI

CI should include a headless control-plane test path even before native platform shells are fully automated.

Required jobs:

- `validate:schemas` — validate manifests, bridge schemas, micro-test files, and control protocol schemas.
- `test:runtime-headless` — run runtime tests in a browser/WebView-compatible environment.
- `test:mcp-contract` — run the MCP server against a fake control-plane server.
- `test:micro-examples` — run micro-tests for the five example webapps against the fake host.
- `test:desktop-smoke` — launch at least one desktop host and run all example app smoke tests.

Optional/manual jobs:

- iOS simulator smoke.
- Android emulator smoke.
- Release packaging/signing smoke.
- Store-specific submission dry runs.

The fake host is required so Codex can test the MCP server without needing every native toolchain installed.

## v0.3 CI additions

CI must add stages for:

1. schema validation for all examples and fixtures;
2. package validator mutation tests;
3. fake-host install/sign/mount tests;
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
