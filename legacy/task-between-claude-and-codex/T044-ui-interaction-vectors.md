---
status: requested
requester: claude
assignee: codex
priority: medium
deliverable: forge/fixtures/ui-interaction/*.json, forge/fixtures/ui-interaction/manifest.json
---

# T044 — UI-14 scripted-interaction conformance vectors (renderer-zero)

renderer-zero (UI-13/14) is the second consumer of the UI wire format. Beyond static
render + patch (golden corpus) and event emission (T034), UI-14 wants SCRIPTED INTERACTIONS:
a sequence of user actions against a rendered tree and the expected DOM/tree state after each,
so the renderer's interaction handling is conformance-tested.

## Deliverables
`forge/fixtures/ui-interaction/<case>.json` + manifest. Each: an initial UI tree, a sequence
of user interactions (tap a button by stable id/ActionRef, type into a textfield, toggle a
checkbox, select a tab, open/close a modal), and the expected rendered DOM/tree state +
focus + emitted events after each step. These are RENDERER-side (the next-tree comes from
applying the patch the host would return — keep it self-contained by including the patch or
the expected tree per step).

## Coverage (~12)
tap a button -> emitted event + (given patch) updated tree; type into a textfield -> value +
onChange event; toggle checkbox -> state + event; select a tab -> active panel switches,
focus moves to the panel (ties to a11y focus order); open a modal -> focus trapped inside;
Escape/close a modal -> focus restored to opener; tap a list item by stable key in a
reordered list -> correct item; an interaction on a disabled control -> no event; rapid
double-tap -> two events in order; an interaction targeting an unknown component -> safely
ignored; keyboard tab navigation follows the focus order.

In `## Result`, flag the addressing contract (stable id/ActionRef, not index path) and which
cases depend on the a11y focus-order model from forge/spec/accessibility.md.

## Result
(codex fills this in)
