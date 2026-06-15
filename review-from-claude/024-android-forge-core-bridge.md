# Slice review: Android Forge core bridge

## Slice goal

Cut the Android host `core.step` path from the legacy Zig JNI bridge to the
Forge FFI CoreCommand ABI, while preserving the host-derived app id permission
check and Android ABI packaging through generated `jniLibs`.

Per user instruction on 2026-06-15, this slice was implemented and reviewed
independently by Codex instead of requesting Claude Code review.

## Commit or working diff reviewed

Working diff before commit for the Android Forge FFI bridge slice.

## Files changed

- `native/android/README.md`
- `native/android/app/build.gradle.kts`
- `native/android/app/src/main/cpp/CMakeLists.txt`
- `native/android/app/src/main/cpp/forge_core_jni.cpp`
- `native/android/app/src/main/cpp/zig_core_jni.cpp` deleted
- `native/android/app/src/main/java/com/terrane/platform/ForgeCoreBridge.kt`
- `native/android/app/src/main/java/com/terrane/platform/ZigCoreBridge.kt` deleted
- `native/android/app/src/main/java/com/terrane/platform/AndroidDevControlPlane.kt`
- `native/android/app/src/main/java/com/terrane/platform/NativeBridge.kt`
- `tools/check-repo.mjs`
- `tools/reference-host/test/android-native-build.test.js`

## Commands/tests run

- `node --test --no-warnings tools/reference-host/test/android-native-build.test.js`
  - Gradle assembly test skipped with "gradle is not available".
- `node --test --no-warnings tools/reference-host/test/runtime-capabilities-contract.test.js`
- `cargo test -p forge-ffi --locked`
- `node --no-warnings tools/check-repo.mjs`

## Review findings

- No blocker found in the Kotlin bridge: `ForgeCoreBridge.step` keeps the
  caller-provided `app` mismatch rejection and routes a `legacy.core_step`
  CoreCommand through the native Forge handle.
- No blocker found in JNI: `forge_core_jni` loads `libforge_ffi.so`, resolves the
  `forge_core_*` symbols, opens an in-memory Android workspace, and frees Forge
  strings through `forge_string_free`.
- No blocker found in Gradle wiring: Android now generates ABI-specific
  `libforge_ffi.so` outputs under `generated/terrane-forge-ffi/jniLibs` and
  packages them as `jniLibs`.
- Non-blocking note: the actual Android Gradle build was not run because `gradle`
  is unavailable in this environment. Static Android source tests and
  `tools/check-repo.mjs` passed.
- Non-blocking note: Android Rust cross-builds require the appropriate Rust
  Android targets and NDK linker configuration in the build environment.

## Resolution status

- All findings resolved or explicitly documented.
- `tools/check-repo.mjs` reports Android native core status as `core=forge-ffi`.
- No Android-local `ZigCoreBridge`, `zig_core_jni`, `libzig_core`, or
  `core_step_json` references remain in the intended slice.

## Follow-up tasks

- Run the Android Gradle assembly on a machine with Gradle, Android SDK/NDK, and
  Rust Android targets installed.
- Continue Phase 2 with Windows host resolution/cutover.
