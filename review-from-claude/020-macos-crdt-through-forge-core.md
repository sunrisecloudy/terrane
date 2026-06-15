# 020 macOS CRDT through Forge core self-review

Independent Codex review note. This slice intentionally does not depend on Claude Code review.

## Scope

- Removed the macOS `CZigCrdtBridge` SwiftPM target and `ZigCrdtBridge` wrapper.
- Folded the macOS CRDT/sync capability smoke into `ForgeCoreBridge` by issuing `sync.export` through `forge_core_handle_command`.
- Kept the existing `notebook.crdt` capability key for bridge compatibility, but its backing check now comes from Forge core instead of `libzig_crdt.dylib`.
- Updated `tools/check-repo.mjs` to reject `CZigCrdtBridge` on macOS and assert the Forge `sync.export` path.

## Verification

- `cargo test -p forge-ffi --locked`
- `node --test --no-warnings tools/reference-host/test/native-core-timeout-source.test.js`
- `node --no-warnings tools/check-repo.mjs`
- `node --test --no-warnings tools/reference-host/test/macos-native-build.test.js`

## Notes

- The macOS SwiftPM smoke required approved escalation for Swift/Clang module-cache writes outside the workspace.
- Remote-owned macOS and tooling files were edited because this is the CRDT half of the approved macOS Forge cutover.
