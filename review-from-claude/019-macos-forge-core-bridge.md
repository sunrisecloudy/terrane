# 019 macOS Forge core bridge self-review

Independent Codex review note. This slice intentionally does not depend on Claude Code review.

## Scope

- Replaced the macOS `core.step` host bridge from the legacy Zig core dylib shim to the Forge FFI dylib shim.
- Added the `CForgeCoreBridge` SwiftPM C target and `ForgeCoreBridge.swift`.
- Routed macOS `core.step` through the Forge `legacy.core_step` compatibility command while preserving channel-derived app id enforcement and timeout behavior.
- Left `ZigCrdtBridge` in place; CRDT host cutover remains a later slice.

## Verification

- `cargo test -p forge-ffi --locked`
- `node --test --no-warnings tools/reference-host/test/native-core-timeout-source.test.js`
- `node --no-warnings tools/check-repo.mjs`
- `node --test --no-warnings tools/reference-host/test/macos-native-build.test.js`

## Notes

- The first macOS SwiftPM smoke was blocked by sandbox access to Swift/Clang module caches, then passed when rerun with approved escalation.
- Remote-owned macOS and reference-host files were edited because this slice is the host cutover boundary required by `task-jun-15/04-IMPLEMENTATION-PROMPT.md`.
