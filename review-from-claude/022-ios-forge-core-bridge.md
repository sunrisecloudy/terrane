# Slice review: iOS Forge core bridge

## Slice goal

Cut the iOS host `core.step` path from the legacy Zig core bridge to the Forge
FFI CoreCommand ABI, while preserving the host-derived app id permission check
and simulator/dev dylib fallback behavior.

Per user instruction on 2026-06-15, this slice was implemented and reviewed
independently by Codex instead of requesting Claude Code review.

## Commit or working diff reviewed

Working diff before commit for the iOS Forge FFI bridge slice.

## Files changed

- `native/ios/Package.swift`
- `native/ios/README.md`
- `native/ios/Sources/CForgeCoreBridge/CForgeCoreBridge.c`
- `native/ios/Sources/CForgeCoreBridge/include/CForgeCoreBridge.h`
- `native/ios/Sources/TerraneHostIOS/ForgeCoreBridge.swift`
- `native/ios/Sources/TerraneHostIOS/IOSDevControlPlane.swift`
- `native/ios/Sources/TerraneHostIOS/WebBridge.swift`
- `native/ios/Sources/CZigCoreBridge/CZigCoreBridge.c` deleted
- `native/ios/Sources/CZigCoreBridge/include/CZigCoreBridge.h` deleted
- `native/ios/Sources/TerraneHostIOS/ZigCoreBridge.swift` deleted
- `forge/crates/runtime/Cargo.toml`
- `forge/crates/runtime/src/lib.rs`
- `forge/crates/runtime/src/unsupported_runner.rs`
- `tools/check-repo.mjs`
- `tools/reference-host/test/ios-native-build.test.js`

## Commands/tests run

- `cargo build -p forge-ffi --locked --target aarch64-apple-ios-sim`
- `cargo test -p forge-runtime --locked`
- `cargo test -p forge-ffi --locked`
- `node --no-warnings tools/check-repo.mjs`
- `node --test --no-warnings tools/reference-host/test/ios-native-build.test.js`
- `node --test --no-warnings tools/reference-host/test/runtime-capabilities-contract.test.js`

## Review findings

- No blocker found in the Swift bridge: `core.step` now sends a
  `legacy.core_step` CoreCommand envelope through `forge_core_handle_command`
  and keeps the channel-derived app id enforcement before dispatch.
- No blocker found in the C shim: linked symbols are tried first, then explicit
  and repo/bundle `libforge_ffi.dylib` paths; allocations and returned strings
  are released through the FFI-owned free function.
- No blocker found in source gates: `tools/check-repo.mjs` now validates the iOS
  Forge FFI bridge without changing the remaining Zig-backed Linux, Android, or
  Windows status labels.
- Non-blocking note: `rquickjs-sys` does not ship pre-generated iOS simulator
  bindings. The slice gates `rquickjs` off iOS and returns
  `CoreError::PlatformUnavailable` for runtime JS execution on iOS until the
  planned CR-12/JSC backend lands. This still allows the iOS host to load
  `forge-ffi` for non-JS CoreCommand surfaces such as `legacy.core_step` and
  sync commands.

## Resolution status

- All findings resolved or explicitly documented.
- The iOS simulator `forge-ffi` target build is green after the runtime target
  gate.
- No legacy iOS `ZigCoreBridge` / `CZigCoreBridge` files remain in the intended
  staged slice.

## Follow-up tasks

- Continue Phase 2 host cutover with the remaining Zig-backed native paths:
  Linux, Android, and Windows.
- Replace the iOS runtime JS fallback with the planned CR-12/JSC or equivalent
  JavaScript backend when that milestone is in scope.
