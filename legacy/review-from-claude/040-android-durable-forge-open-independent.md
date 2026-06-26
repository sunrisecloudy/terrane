# Android Durable Forge Open - Independent Review

## Slice goal

Fix the Android native host persistence blocker by opening Forge through
file-backed `forge_core_open(path, workspace_id)` instead of
`forge_core_open_in_memory`.

## Files changed

- `native/android/app/src/main/java/com/terrane/platform/ForgeCoreBridge.kt`
- `native/android/app/src/main/java/com/terrane/platform/NativeBridge.kt`
- `native/android/app/src/main/java/com/terrane/platform/AndroidDevControlPlane.kt`
- `native/android/app/src/main/cpp/forge_core_jni.cpp`
- `tools/reference-host/test/android-native-build.test.js`

## Resolution

- `ForgeCoreBridge` now receives Android `Context` and derives an app-private
  durable database path with `context.getDatabasePath("forge-workspace.sqlite")`.
- JNI now accepts the database path for availability and command handling,
  resolves `forge_core_open`, and opens workspace `android-native` against that
  file.
- Runtime and debug-control call sites now construct `ForgeCoreBridge(context)`.
- The reference-host Android source assertion was updated to require the new
  context-backed construction.

## Verification

- `rg "ForgeCoreBridge\\(|nativeIsAvailable\\(|nativeHandleCommand\\(|forge_core_open_in_memory|open_in_memory|ForgeCoreOpenInMemory" native/android/app/src/main`: only the new context-backed calls and JNI signatures remain; no in-memory open remains.
- `git diff --check` for the Android slice: passed.
- `node --test --no-warnings tools/reference-host/test/android-native-build.test.js`: 12 passed, 0 failed, including debug APK assembly with JNI libraries.
