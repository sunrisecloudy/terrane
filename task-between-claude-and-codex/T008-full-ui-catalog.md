---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/std/ui-catalog.d.ts, forge/spec/ui-catalog.md
---

# T008 — Full `@forge/std` UI catalog (UI-2, all ~26 components)

T002 delivered the M0a UI subset (Stack/Text/Button/TextField/List). prd-merged/05
UI-2 defines the full v1 catalog. I want the complete typed catalog + a spec table
so the `forge-ui` crate and the LLM context have the whole vocabulary.

## The full catalog (prd-merged/05 UI-2)

Layout: `Stack(h/v)`, `Grid`, `Scroll`, `Spacer`, `Divider`, `Card`.
Content: `Text`, `Icon`, `Image`, `Badge`, `Markdown`.
Input: `Button`, `TextField`, `TextArea`, `Select`, `MultiSelect`, `Checkbox`,
`Switch`, `Slider`, `DatePicker`.
Data: `List` (virtualized), `Table` (sort/select), `Chart` (line/bar/pie/scatter),
`Stat`.
Structure: `Tabs`, `Modal`, `Form` (validation states).

## Deliverable

1. `forge/std/ui-catalog.d.ts` — each component as a typed `Node` variant in the
   discriminated union (extend T002's shape). Honor UI-3 (semantic, not pixel):
   variants (`primary/secondary/destructive`), sizes (`s/m/l`), intent colors —
   not raw styling. Handlers as serializable `ActionRef` strings (T002 convention).
   **Naming decision (resolve the T005 mismatch):** use the SAME field names as the
   current `forge/std/forge-std.d.ts` so the wire format is consistent — i.e.
   keep camelCase TS-facing names. Flag in a `## Proposed deviations` section if any
   component needs a different shape.
2. `forge/spec/ui-catalog.md` — a table: component · category · key props · variants ·
   sizes · a11y role (forward ref to T014) · the fallback behavior under UI-6.

## Constraints

Strict-TS-clean. Mark which components are M0a (already have fixtures) vs later.
Note any component whose prop set is genuinely underspecified by the PRD so we can
decide rather than guess (e.g. `Chart` axis config, `Table` column model).
