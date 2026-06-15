# Slice Review: Windows Forge Core Bridge

## Slice Goal

Resolve the Windows native path for Phase 2 by keeping the live WebView2 host in `native/windows/` and replacing its legacy Zig `core.step` bridge with a Forge FFI-backed bridge. The top-level `windows/` .NET wrapper is already Forge-backed and remains as a non-host helper surface.

## Diff Reviewed

Working diff before commit for the Windows host cutover.

## Files Changed

- `native/windows/CMakeLists.txt`
- `native/windows/README.md`
- `native/windows/src/ForgeCoreBridge.cpp`
- `native/windows/src/ForgeCoreBridge.h`
- `native/windows/src/DevControlPlane.cpp`
- `native/windows/src/WebBridge.h`
- `native/windows/src/ZigCoreBridge.cpp` (deleted by rename)
- `native/windows/src/ZigCoreBridge.h` (deleted by rename)
- `tools/check-repo.mjs`
- `tools/reference-host/test/native-core-timeout-source.test.js`
- `tools/reference-host/test/native-runtime-resource-mapping.test.js`
- `tools/reference-host/test/windows-dev-control-source.test.js`
- `tools/reference-host/test/windows-native-build.test.js`

## Commands Run

- `node --test --no-warnings tools/reference-host/test/windows-native-build.test.js`
- `node --test --no-warnings tools/reference-host/test/native-core-timeout-source.test.js`
- `node --test --no-warnings tools/reference-host/test/native-runtime-resource-mapping.test.js`
- `node --test --no-warnings tools/reference-host/test/windows-dev-control-source.test.js`
- `node --no-warnings tools/check-repo.mjs`

## Independent Review Findings

- No blocker found in source/static coverage.
- The Windows host preserves the existing async timeout shape and app-id mismatch denial.
- `core.step` now builds a Forge `CoreCommand` envelope with `name = "legacy.core_step"`, `actor = windows-host`, and `workspace_id = windows-native`.
- The bridge loads `forge_ffi.dll` from `TERRANE_FORGE_FFI_DLL`, executable-adjacent packaging, and repo-local Forge build outputs.
- Dev-control replay now uses `ForgeCoreBridge` instead of the old Zig bridge.

## Resolution Status

- Addressed in this slice.
- Native Windows compile/smoke is unavailable on this macOS host; source-contract tests were updated so Windows CI/build hosts verify Cargo-built `forge_ffi.dll` staging.

## Follow-Up Tasks

- Release packaging still has separate legacy Windows artifact references and is intentionally left for the dedicated packaging/CI slice.
