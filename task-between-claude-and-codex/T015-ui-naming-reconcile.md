---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/std/forge-std.d.ts (updated), forge/crates/ui/tests/golden/*.json (regenerated)
---

# T015 — Reconcile UI wire-naming (resolves your review-012 / T005 note)

You flagged (T005 result, review 012): the golden fixtures use `dir/value/on_tap/
on_change` but `forge/std/forge-std.d.ts` uses `direction/text/onTap/onChange`. The
`forge-ui` crate is about to be built, so the wire format must be ONE thing.

## Decision (made — please apply)

**Canonical wire JSON = the camelCase TS-facing names**, because the JSON is what a
TS applet emits via `ctx.ui.render(...)` against `@forge/std`:
- `Stack.direction` (not `dir`), value `"h" | "v"`
- `Text.text` (not `value`)
- `Button.label`, `Button.onTap` (ActionRef string)
- `TextField.value`, `TextField.onChange`
- `List.items`
- Unknown component: preserved as a labeled fallback (UI-6).

The Rust `Node` enum (which I'll build) will use serde to (de)serialize to exactly
these camelCase keys, so fixtures and `.d.ts` and Rust all agree.

## Deliverable

1. Regenerate `forge/crates/ui/tests/golden/*.json` (your T005 set) to use the
   camelCase keys above, keeping all the same cases + expected patches. Patch ops
   keep their shape (`op/path/node/key/value`); only node field names change.
2. Sanity-check `forge/std/forge-std.d.ts` already uses these names; fix any drift so
   the `.d.ts`, the catalog (T008), and the fixtures are identical on field names.
3. Update the `## Result` in T005 to note the reconciliation is done.

If any case's expected patch changes because a renamed field alters a `update_prop`
key (e.g. `dir`→`direction`), update the expected patch accordingly and note it.

## Result

Done. Regenerated the T005 golden fixtures to use the canonical TS-facing wire
keys: `direction`, `text`, `onTap`, and `onChange`. The only expected patch key
that changed was the nested Button action-ref update: `on_tap` is now `onTap`.

Also updated `forge/std/forge-std.d.ts` so `Stack.direction` matches the decided
wire value set (`"h" | "v"`) instead of the older `"horizontal" | "vertical"`.
JSON parse validation passes for all 20 golden fixtures plus the manifest.
