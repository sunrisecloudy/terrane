# Windows Durable Forge Open - Independent Review

## Slice goal

Fix the surviving native Windows host persistence blocker by opening Forge
through file-backed `forge_core_open(path, workspace_id)` instead of
`forge_core_open_in_memory`.

## Files changed

- `native/windows/src/ForgeCoreBridge.h`
- `native/windows/src/ForgeCoreBridge.cpp`
- `native/windows/src/WebBridge.cpp`
- `native/windows/src/DevControlPlane.cpp`
- `tools/reference-host/test/windows-native-build.test.js`

## Resolution

- `ForgeCoreBridge` now receives the host platform database path and derives a
  sibling `forge-workspace.sqlite` for Forge state.
- The native Windows bridge now resolves `forge_core_open` and passes the
  durable Forge database path plus workspace id `windows-native`.
- `WebBridge` and dev-control replay now pass their existing platform database
  path into `ForgeCoreBridge`.
- The Windows reference-host test now asserts that the native host uses durable
  `forge_core_open` and does not contain `forge_core_open_in_memory`.

## Verification

- `rg "ForgeCoreBridge\\(|ForgeCoreOpenInMemory|forge_core_open_in_memory|openInMemory|open_in_memory" native/windows/src windows/src`: no native Windows host in-memory open remains. Top-level `windows/` still exposes the intentional `.NET` test/helper `OpenInMemory` API.
- `git diff --check` for the Windows slice: passed.
- `node --test --no-warnings tools/reference-host/test/windows-native-build.test.js`: 3 passed, 0 failed, 4 skipped on macOS because Windows native smoke requires a Windows host.

## Follow-up

- Run the Windows CMake/WebView2 native smoke on a real Windows host.
