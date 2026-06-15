# macOS And iOS Durable Forge Open - Independent Review

## Slice goal

Fix the native host persistence blocker for the Apple hosts by opening Forge
through file-backed `forge_core_open(path, workspace_id)` instead of
`forge_core_open_in_memory`.

## Files changed

- `native/macos/Sources/CForgeCoreBridge/CForgeCoreBridge.c`
- `native/macos/Sources/CForgeCoreBridge/include/CForgeCoreBridge.h`
- `native/macos/Sources/TerraneHostMac/ForgeCoreBridge.swift`
- `native/ios/Sources/CForgeCoreBridge/CForgeCoreBridge.c`
- `native/ios/Sources/CForgeCoreBridge/include/CForgeCoreBridge.h`
- `native/ios/Sources/TerraneHostIOS/ForgeCoreBridge.swift`

## Resolution

- The C wrappers now resolve `forge_core_open` and pass a database path plus
  workspace id to Forge.
- The Swift bridges create `Application Support/Terrane` and use
  `forge-workspace.sqlite`, keeping Forge workspace persistence separate from
  the host platform `platform.sqlite` database.
- The old `terrane_forge_core_open_in_memory` wrapper entrypoint was removed
  from the Apple host wrappers.

## Verification

- `swift build` in `native/macos`: passed. Note: the macOS tree already had
  unrelated dirty local host files, so this confirms the current working tree
  builds but is not a pristine-HEAD proof.
- `swift build` in `native/ios`: the C wrapper compiled, then the package failed
  on the existing default-host SDK issue `no such module 'UIKit'`.
- `rg "terrane_forge_core_open_in_memory|forge_core_open_in_memory" native/macos native/ios -g '!**/.build/**'`: no matches.
- `git diff --check` for the changed Apple host files: passed.

## Follow-up

- Add a restart-persistence smoke once the host test harness can run without
  relying on unrelated dirty macOS files or a default macOS SwiftPM build for the
  iOS package.
