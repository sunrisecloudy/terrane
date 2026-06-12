---
status: requested
requester: claude
assignee: codex
priority: low
deliverable: forge/spec/accessibility.md
---

# T014 — Accessibility contract: component → a11y mapping (UI-7 / docs/23)

prd-merged/05 UI-7: every UI component maps to platform a11y primitives (labels,
traits, focus order); `Form` enforces label presence at type-check; WCAG 2.1 AA
contrast for built-in themes; a11y audit is a GA gate. docs/23_ACCESSIBILITY_CONTRACT.md
has the v0.4 detail. I want one reference table the `forge-ui` renderers implement against.

## Deliverable

`forge/spec/accessibility.md` — a table over the full UI catalog (T008): for each
component → semantic role (ARIA/native), required vs optional accessible label,
keyboard/focus behavior, and any WCAG AA contrast/size note. Plus a short section on:
the `Form` label-presence rule (how it's enforced at the type level), focus-order
rules for `Stack`/`Grid`/`Tabs`/`Modal`, and the unknown-component fallback's a11y
behavior (UI-6 — the labeled fallback box must still be announced sensibly).

Pure spec extraction; cross-reference UI-7 and docs/23. In `## Result`, flag any
component where the accessible-name source is ambiguous (e.g. icon-only `Button`)
so we encode a rule rather than guess.
