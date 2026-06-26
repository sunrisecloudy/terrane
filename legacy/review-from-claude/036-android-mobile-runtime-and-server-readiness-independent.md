# Android Mobile Runtime And Server Readiness - Independent Review

## Slice goal

Fix the verification failures found during Phase 4 closure:

- Android native build could not cross-compile Forge FFI through Gradle.
- Android target pulled in `rquickjs-sys`, which has no pre-generated Android
  bindings.
- Full reference-host suite could race the Forge server bridge readiness check
  while concurrent Cargo builds were running.

User direction for this run: work independently from Claude Code. No Claude Code
review was requested for this slice; this file is the local independent review
record.

## Files changed

- `native/android/app/build.gradle.kts`
- `forge/crates/runtime/Cargo.toml`
- `forge/crates/runtime/src/lib.rs`
- `forge/crates/runtime/src/engine.rs`
- `forge/crates/runtime/src/unsupported_runner.rs`
- `tools/reference-host/test/forge-server-bridge-contract.test.js`

## Findings And Resolution

- Android Gradle was invoking `cargo build --target <android-triple>` without
  telling Cargo/cc where the Android NDK compiler and archiver live. The task now
  discovers the SDK/NDK, resolves the LLVM prebuilt bin directory, and sets
  `CC_*`, `AR_*`, and Cargo linker variables for each ABI.
- `forge-runtime` already used structured `PlatformUnavailable` stubs for iOS,
  where `rquickjs-sys` has no bundled bindings. Android has the same binding
  gap, so Android now uses the same unsupported-runner path until the planned
  mobile JS backend lands.
- The Forge server bridge contract test waited only five seconds for
  `cargo run -p forge-server` to bind. Under the full parallel reference-host
  suite, concurrent Cargo builds can consume that window. The readiness wait is
  now deadline-based at 90 seconds, still inside the test timeout.

## Commands run

```sh
node --test --no-warnings tools/reference-host/test/android-native-build.test.js
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p forge-cli -- demo
node --no-warnings tools/check-repo.mjs
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
node --test --no-warnings tools/reference-host/test/forge-server-bridge-contract.test.js
node --test --no-warnings tools/reference-host/test/*.test.js
git diff --check
```

## Verification Results

- Android native build test: 12 passed, 0 failed.
- Forge workspace tests: passed.
- Forge workspace clippy with `-D warnings`: passed.
- Forge demo: printed `REPLAY IDENTICAL: true`.
- `tools/check-repo.mjs`: passed.
- Public contract verification: `ok: true`, `filesChecked: 457`.
- Full reference-host suite: 218 passed, 0 failed, 11 skipped. Skips were
  platform/browser gated.
- `git diff --check`: passed.

## Follow-up

- Android/iOS in-core JS execution intentionally remains `PlatformUnavailable`
  until CR-12/JSC or another mobile JS backend is implemented. Non-JS Forge core
  commands and the FFI bridge build and link on those targets.
