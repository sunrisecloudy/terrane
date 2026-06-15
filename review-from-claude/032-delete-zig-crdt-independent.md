# 032 Delete Zig CRDT

## Slice goal

Delete the retired `zig-crdt/` implementation after sync/CRDT exchange was folded into Forge core commands and the Forge FFI bridge.

## Review mode

Independent Codex self-review. Claude Code review was intentionally not requested because the user instructed this run to proceed independently from Claude Code.

## Files changed

- `zig-crdt/README.md`
- `zig-crdt/build.zig`
- `zig-crdt/build.zig.zon`
- `zig-crdt/include/zig_crdt.h`
- `zig-crdt/src/lib.zig`
- `tools/check-repo.mjs`

## Deletion gate

`zig-crdt/` no longer has live host, server, CI, packaging, or check consumers. Forge sync is exposed through `forge_core_handle_command` via `sync.export` / `sync.import`, and `forge-ffi` tests verify sync crosses the C ABI without separate CRDT symbols.

## Zero-reference proof

No non-archived live hits after rewriting the negative macOS package sentinel:

```sh
rg -n --hidden -g '!.git/**' -g '!external-lib/**' -g '!forge/target/**' -g '!target/**' -g '!zig-crdt/**' -g '!docs/**' -g '!review/**' -g '!review-from-claude/**' -g '!task-between-claude-and-codex/**' -g '!task-jun-15/**' "zig-crdt|zig_crdt|libzig_crdt|terrane_zig_crdt_|CZigCrdtBridge" .
```

Archived v0.4 docs still describe the old CRDT package and are intentionally retained.

## Commands and evidence

- `cargo test -p forge-core --test sync --locked`
- `cargo test -p forge-core --test sync_rbac --locked`
- `cargo test -p forge-core --test sync_rbac_enforced --locked`
- `cargo test -p forge-ffi --locked`
- `node --no-warnings tools/check-repo.mjs`
- `git diff --check`

## Findings

- No live code path referenced the standalone CRDT package.
- `tools/check-repo.mjs` kept a negative check for the old macOS package target. The check remains, but it constructs the retired symbol name instead of keeping the retired literal in live grep output.

## Resolution

- Deleted `zig-crdt/`.
- Kept the macOS package guard while removing the retired literal from live search results.
