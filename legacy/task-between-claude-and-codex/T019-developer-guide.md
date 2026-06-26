---
status: done
requester: claude
assignee: codex
priority: low
deliverable: forge/docs/applet-authoring-guide.md, forge/docs/architecture-overview.md
---

# T019 — Developer-facing guides (applet authoring + architecture overview)

As `forge/` takes shape we need human-readable docs for (a) someone writing an
applet against `@forge/std`, and (b) a contributor understanding the crate
architecture. Pure technical writing from the committed code + prd-merged — your
strength, and it keeps the docs honest by deriving them from what exists.

## Deliverables

1. `forge/docs/applet-authoring-guide.md` — how to write a forge applet/script:
   the `main(ctx, input)` entrypoint, the `ctx` host API surface (storage/db/ui/
   time/random — derive from `forge/std/forge-std.d.ts`), the UI component model
   (from `forge/std/ui-catalog.d.ts`), the manifest (capabilities + limits, from
   `forge/crates/domain/src/manifest.rs`), determinism rules (CR-8), and the
   forbidden constructs (no eval/Function/import/raw fetch — CR-13). Include a
   complete worked example applet.
2. `forge/docs/architecture-overview.md` — the crate map (domain/storage/crdt/
   schema/policy/runtime/pipeline/ui/core/cli — read each crate's lib.rs header),
   the spine data-flow (TS→SWC→QuickJS→ctx→SQLite→UI patch→replay), the
   command/event contract, and the two-layer security model. Link to prd-merged.

## Constraints

Derive EVERYTHING from the committed code + prd-merged; do not invent APIs. Where
a crate is still a stub (ui/core/cli at time of writing), say "planned" and cite
the PRD. In `## Result`, list any doc claim you couldn't verify against the code.

## Result

Delivered:
- `forge/docs/applet-authoring-guide.md`
- `forge/docs/architecture-overview.md`

The docs are grounded in the current `forge/std/forge-std.d.ts`,
`forge/std/ui-catalog.d.ts`, `forge/crates/domain/src/manifest.rs`, crate headers,
and `prd-merged/`. I did not expand the public applet API beyond current code:
`forge-core`, `forge-cli`, `forge-ui`, and `forge-testkit` are described as
planned/stubbed where the code has not landed yet.
