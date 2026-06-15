# Slice Review: Live Docs Forge Cutover

## Slice Goal

Remove live user-facing Zig core/server references from root documentation before deleting legacy source directories.

## Diff Reviewed

Working diff before commit for root documentation cleanup.

## Files Changed

- `README.md`
- `CONTRIBUTION.md`
- `windom-plan.md`

## Commands Run

- `rg -n "zig_core|zig-core|libzig_core|zig_crdt|zig-crdt|libzig_crdt|terrane_zig_core_|terrane_zig_crdt_|core_step_json|build-zig-core|server/src/main\\.zig|zig build|setup-zig|mlugg/setup-zig|ZIG_GLOBAL_CACHE|TERRANE_ZIG|ZigCoreBridge|libzig" README.md CONTRIBUTION.md windom-plan.md`
- `git diff --check README.md CONTRIBUTION.md windom-plan.md`

## Independent Review Findings

- No blocker found.
- Root docs now point contributors and users at the Forge Rust workspace, Forge server, Forge FFI DLL, and `terrane.app` release bundle path.

## Resolution Status

- Addressed in this slice.

## Follow-Up Tasks

- Run the component-specific zero-reference grep before deleting `zig-core/` and `zig-crdt/`.
