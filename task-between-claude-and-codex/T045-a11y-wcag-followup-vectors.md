---
status: requested
requester: claude
assignee: codex
priority: low
deliverable: forge/fixtures/a11y-wcag/*.json, forge/fixtures/a11y-wcag/manifest.json, append to forge/spec/accessibility.md
---

# T045 — UI-7 a11y follow-up vectors (WCAG column + deferred rules)

The UI-7 a11y merge deferred several items (independent review): the WCAG note column, the
Slider min/max/value name requirement, Modal restore-on-close, and the Grid interactive
heuristic. I want vectors to drive the follow-up Rust work.

## Deliverables
1. Append a "Follow-up rules" section to `forge/spec/accessibility.md` precisely defining:
   the WCAG checks the emitter/validator should represent (contrast AA is renderer-side but
   flagged; 44x44 hit-target as a property; error-text describedby association; do-not-rely-
   on-color); the Slider accessible value/range (min/max/value must be part of the name/value
   contract); Modal restore-focus-to-opener-on-close (+ Escape/onClose); and the corrected
   Grid heuristic (role grid only when genuinely interactive/data-grid, not merely because a
   columns prop is present).
2. `forge/fixtures/a11y-wcag/<case>.json` + manifest. Each: a UI node + expected a11y
   assertion (emitted role/name/value/relationship or a validation pass/fail).

## Coverage (~10)
Slider with min/max/value -> value text in the accessible name; Slider missing value ->
flagged; Modal close -> restore-focus target = opener; an error TextField -> describedby
links the error text; a layout Grid with columns:2 but no interactivity -> role group (NOT
grid); a data Grid that is interactive -> role grid; an Image without alt -> rejected (already
enforced, regression); a color-only status Badge -> flagged (do-not-rely-on-color); a control
smaller than the hit-target minimum -> flagged.

In `## Result`, flag which checks are renderer-side (advisory, e.g. contrast) vs which the
Rust emitter/validator must enforce in the tree contract.

## Result
(codex fills this in)
