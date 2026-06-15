# 031 Delete Zig Core

## Slice goal

Delete the retired `zig-core/` implementation after all live consumers were cut over to Forge core and Forge FFI.

## Review mode

Independent Codex self-review. Claude Code review was intentionally not requested because the user instructed this run to proceed independently from Claude Code.

## Files changed

- `zig-core/README.md`
- `zig-core/build.zig`
- `zig-core/build.zig.zon`
- `zig-core/include/zig_core.h`
- `zig-core/src/lib.zig`

## Deletion gate

`server/` was the final functional importer of `zig-core/`; it was deleted in the preceding slice. Native hosts, packaging, CI, reference-host checks, and release docs now target Forge FFI.

## Zero-reference proof

No non-archived live hits before deletion:

```sh
rg -n --hidden -g '!.git/**' -g '!external-lib/**' -g '!forge/target/**' -g '!target/**' -g '!zig-core/**' -g '!docs/**' -g '!review/**' -g '!review-from-claude/**' -g '!task-between-claude-and-codex/**' -g '!task-jun-15/**' "zig-core|zig_core|libzig_core|terrane_zig_core_|core_step_json|ZigCoreBridge|setup-zig|mlugg/setup-zig|ZIG_GLOBAL_CACHE|TERRANE_ZIG" .
```

Archived v0.4 docs still describe the old core ABI and are intentionally retained.

## Commands and evidence

- `cargo test -p forge-core --locked`
- `cargo test -p forge-ffi --locked`
- `node --test --no-warnings tools/reference-host/test/native-core-timeout-source.test.js`
- `node --no-warnings tools/check-repo.mjs`
- `git diff --check`

## Findings

- No live code, host, tool, CI, or packaging path still referenced `zig-core/`.

## Resolution

- Deleted `zig-core/`.
