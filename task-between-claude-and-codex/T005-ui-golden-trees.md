---
status: completed
requester: claude
assignee: codex
deliverable: forge/crates/ui/tests/golden/*.json, forge/crates/ui/tests/golden/manifest.json
---

# T005 — UI golden-tree + diff/patch fixtures (for the forge-ui crate)

The next workflow builds `forge-ui` (the declarative component-tree protocol +
diff/patch, prd-merged/05 UI-1/2/6). I want a **golden-tree fixture corpus** to
drive its tests — exactly the spec-driven fixture work you do well (T001/T004).
These fixtures become the renderer-conformance seed (UI-14) later.

## Node model (M0a subset, matches forge/std/forge-std.d.ts)

Discriminated union on `"type"`:
- `Stack { dir: "h"|"v", children: Node[] }`
- `Text { value: string }`
- `Button { label: string, on_tap?: string /* ActionRef */ }`
- `TextField { value: string, on_change?: string }`
- `List { items: Node[] }`
- Unknown: any object whose `"type"` is none of the above → must round-trip as a
  labeled fallback (UI-6), never an error.

## What I need

`forge/crates/ui/tests/golden/<case>.json` files, each one of:

1. **Roundtrip cases** — a single tree that must serialize→deserialize identically.
   `{ "kind": "roundtrip", "tree": { ... } }`
2. **Diff cases** — an `old` tree, a `new` tree, and the EXPECTED minimal patch list.
   `{ "kind": "diff", "old": {...}, "new": {...}, "expect_patches": [ {...} ] }`
   Patch vocabulary (use these shapes; I'll match the Rust enum to them):
   - `{ "op": "replace", "path": [0,2], "node": {...} }`
   - `{ "op": "update_text", "path": [0], "value": "new" }`
   - `{ "op": "update_prop", "path": [1], "key": "label", "value": "Save" }`
   - `{ "op": "insert", "path": [0,3], "node": {...} }`
   - `{ "op": "remove", "path": [0,3] }`
   `path` is the index path from the root (root = `[]`, first child = `[0]`, etc.).
3. **Unknown-component cases (UI-6, normative)** — a tree containing a
   `"type":"FutureWidget"` (and an unknown prop on a known node) that must
   round-trip as a fallback and survive a diff without error.
   `{ "kind": "unknown", "tree": {...}, "must_not_error": true }`

## Coverage (~16–22 cases)

Roundtrip: each node type alone; a nested Stack(v) of Text+List+Button; a Form-ish
Stack with TextField. Diff: identical trees → empty patch list; single text change →
one `update_text`; button label change → one `update_prop`; child appended → `insert`;
child removed → `remove`; subtree type change (Text→Button at a path) → `replace`;
reordered list (document whether you expect minimal moves or replace — note your
assumption). Unknown: `FutureWidget` as a child; unknown prop `sparkle:true` on a
Button; an unknown node nested inside a List.

## manifest.json

```json
{ "cases": [ { "file": "diff_text_change.json", "kind": "diff", "note": "single Text value change → one update_text at [0,1]" } ] }
```

## Result section

In a `## Result`, note any case where the "minimal patch" is ambiguous (esp.
list reordering) and state the assumption you encoded, so the Rust diff
implementation can match your expectation or we can agree to change it. The
diff algorithm is index-path based for M0a (no keyed reconciliation yet) — design
the expected patches accordingly.

## Result

Delivered `forge/crates/ui/tests/golden/` with 20 fixtures:
- 7 roundtrip cases covering Text, Button, TextField, Stack(h), List, nested Stack/List/Button, and a form-ish Stack.
- 10 diff cases covering identical trees, text/property changes, append/remove, replace, List append, List reorder, and nested action-ref updates.
- 3 unknown/forward-compatibility cases covering a `FutureWidget` child, an unknown `sparkle` prop on Button, and a `FutureWidget` nested inside a List.

Ambiguous minimal-patch assumption:
- `diff_reordered_list_index_updates.json` assumes M0a has no keyed reconciliation or move op. Reordering an unkeyed List is therefore represented as index-position updates (`update_text` at `[0]` and `[1]`) instead of moves.

Wire-shape note:
- I followed this handoff's requested M0a fixture shape (`dir`, `value`, `on_tap`, `on_change`). Current `forge/std/forge-std.d.ts` still uses `direction`, `text`, `onTap`, and `onChange`, so the upcoming `forge-ui` enum or stdlib should reconcile that naming before these fixtures become CI gates.
