# Remaining Phase 3 Gates - Independent Closure Review

## Slice goal

Record the independent post-deletion gate status after the tracked legacy Zig/v0.4
sources were removed, and identify which Phase 3 candidates are still live
rather than safe to delete.

User direction for this run: work independently from Claude Code. No Claude Code
review was requested for this slice; this file is the local independent review
record kept in the existing migration ledger.

## Working diff reviewed

Post-commit state after:

- `961f40a8 legacy: delete v0.4 server`
- `fab8e025 legacy: delete zig-core`
- `6d8c68f1 legacy: delete zig-crdt`
- `817fd8e0 legacy: delete codex plugin devtools`

## Files changed in this slice

- `review-from-claude/034-remaining-phase3-gates-independent.md`

## Commands run

```sh
git status --short
git ls-files db tests schemas webapps native windows linux-plan window-plan artifacts server zig-core zig-crdt codex codex-plugin devtools
git status --ignored --short server zig-core zig-crdt .zig-cache artifacts
find server zig-core zig-crdt .zig-cache artifacts -maxdepth 3 -type f -o -type d
rg -n "zig_core|zig-core|libzig_core|zig_crdt|zig-crdt|terrane_zig_core_|terrane_zig_crdt_|core_step_json|build-zig-core|build-server|server/src/main.zig|zig build" --glob '!forge/target/**' --glob '!server/**' --glob '!zig-core/**' --glob '!zig-crdt/**' --glob '!external-lib/**' --glob '!artifacts/**' .
rm -rf server zig-core zig-crdt .zig-cache artifacts
```

## Independent findings

- `server/`, `zig-core/`, and `zig-crdt/` have no tracked source left in the
  checkout. The remaining filesystem entries under those paths are ignored Zig
  build caches/output and can be removed as generated leftovers.
- `artifacts/` is ignored release/export output. It can be removed as generated
  output, but tools and docs should continue to reference artifact output paths.
- `db/` is still live. `tools/check-repo.mjs`, reference-host storage tests,
  native hosts, and release packaging still consume `db/sqlite` or `db/postgres`.
  Delete only after Forge storage artifacts replace those consumers.
- `schemas/` is still live. Repo checks, reference-host tests, MCP contract
  checks, and public-contract validation still consume root schemas. Delete only
  after those contracts move to Forge-owned schema locations or are retired.
- `tests/` is still live. Root fixtures, microtests, DB tests, mutation tests,
  security packages, performance harnesses, and platform smoke manifests are
  still consumed by repo checks and reference-host tests. Delete only after those
  coverage surfaces move to Forge or are intentionally retired.
- `webapps/` is still live. Runtime-web, release packaging, native resource
  locators, package validation, and reference-host tests still consume
  `webapps/examples`. Delete only after consumers move to `forge/examples` or
  the legacy generated-app packaging surface is retired.
- `native/*` is not retired. The host paths are now Forge-backed, and active
  checks/release packaging still cover them. Delete only after a replacement
  shell is established and current packaging/CI no longer ships them.
- `linux-plan/` and `window-plan/` are tracked planning docs with no current
  hard gate found. They are not required to delete for the Forge cutover, and no
  deletion was made in this slice.
- Untracked PRD source packs (`local_first_util_2/`,
  `local_first_utility_prd_pack/`) remain untouched because the migration prompt
  requires explicit user confirmation before deleting them.

## Resolution status

- Source deletions already completed: `server/`, `zig-core/`, `zig-crdt/`,
  `codex/`, `codex-plugin/`, and `devtools/`.
- Generated cleanup performed: ignored `server/`, `zig-core/`, `zig-crdt/`,
  root `.zig-cache`, and `artifacts/` outputs were removed after confirming they
  were ignored/generated leftovers.
- Remaining Phase 3 directories are blocked by live consumers, so no additional
  source deletion is safe in this independent pass.

## Follow-up tasks

- Repoint or retire root `db/`, `schemas/`, `tests/`, and `webapps/` consumers
  in separate future slices before deleting those directories.
- Keep surviving native hosts until release packaging and native smoke parity
  no longer require them.
- Ask the user before removing untracked PRD source packs.
