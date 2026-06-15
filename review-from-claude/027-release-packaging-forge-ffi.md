# Slice Review: Release Packaging Forge FFI Cutover

## Slice Goal

Replace release packaging and CI dependencies on legacy Zig core artifacts with Forge FFI outputs for surviving native host packages.

## Diff Reviewed

Working diff before commit for release/tooling cutover.

## Files Changed

- `.github/workflows/ci.yml`
- `.github/workflows/release.yml`
- `docs/12_RELEASE_AND_CI.md`
- `tools/README.md`
- `tools/check-repo.mjs`
- `tools/package-release.mjs`
- `tools/reference-host/test/linux-native-build.test.js`
- `tools/reference-host/test/release-packaging.test.js`
- `tools/reference-host/test/windows-native-build.test.js`

## Commands Run

- `node --test --no-warnings tools/reference-host/test/release-packaging.test.js`
- `node --test --no-warnings tools/reference-host/test/windows-native-build.test.js`
- `node --test --no-warnings tools/reference-host/test/linux-native-build.test.js`
- `node --no-warnings tools/check-repo.mjs`
- `git diff --check`

## Independent Review Findings

- No blocker found in source/static coverage.
- `tools/package-release.mjs` now builds host Forge FFI outputs with Cargo and stages:
  - `Contents/Frameworks/libforge_ffi.dylib` for macOS app bundles.
  - `libforge_ffi.so` beside the Linux `terrane-host` executable.
  - `forge_ffi.dll` beside the Windows `TerraneHost.exe` executable.
- Native release and smoke workflow jobs no longer install Zig for the cutover host paths.
- Release docs, package tests, Linux packaged smoke, Windows packaged smoke, and repo checks now expect Forge FFI package artifacts.
- Existing release-path worktree edits for `public-contract.json` release upload/docs and the `terrane.app` macOS bundle name were included because they are already coupled to the same release manifest/package surface verified by this slice.

## Resolution Status

- Addressed in this slice.
- Local macOS DMG creation is unavailable in this environment: a direct `hdiutil create` check fails with `Device not configured`. The release packaging test now skips the DMG build when `hdiutil create` is unavailable; CI macOS runners should continue exercising it.

## Follow-Up Tasks

- Continue with zero-reference scans for live Zig package/build references outside archived docs and deletion-gated legacy paths.
