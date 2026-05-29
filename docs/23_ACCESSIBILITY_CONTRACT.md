# Accessibility Contract

## 1. Purpose

AI-generated UI must be testable and usable. Accessibility is part of the platform contract because this runtime will host many generated apps, and poor defaults would be copied into every repair loop.

Accessibility has two roles:

- user quality: generated apps need semantic, keyboard-friendly interfaces;
- agent quality: Codex needs stable selectors, names, roles, and snapshots to inspect and repair apps.

## 2. Generated-App Requirements

Generated apps must:

- use semantic HTML landmarks, headings, labels, lists, tables, and buttons where possible;
- include a non-empty document `<title>`;
- include a visible screen title, normally one `<h1>`;
- include a `<main>` landmark for the primary app surface;
- provide accessible names for every button, link, input, select, textarea, and custom control;
- provide explicit labels for inputs;
- keep focus visible;
- support keyboard navigation for interactive controls;
- avoid pointer-only interactions;
- use dialogs that trap focus and restore focus on close;
- expose status changes through visible text and, when useful, ARIA live regions;
- pass contrast thresholds for normal and dark themes;
- avoid text inside images unless equivalent text exists.

Every interactive element must also have a stable `data-testid`. `data-testid` is not the accessible name; it is the automation handle.

## 3. Automated v0.4 Gate

The v0.4 automated gate is intentionally static and deterministic so it can run in the fake host, server control plane, and CI without a browser dependency. It must produce `schemas/accessibility-report.schema.json`.

Required checks:

| Check id | Failure condition | Severity |
|---|---|---|
| `document_title` | Missing or empty document title | fail |
| `main_landmark` | No `<main>` landmark | fail |
| `screen_title` | No level-1 heading | fail |
| `no_unlabeled_controls` | Any interactive control lacks an accessible name | fail |

The fake host and server must expose the same control tools:

```text
runtime.accessibility_snapshot
runtime.run_accessibility_audit
runtime.assert_accessibility
```

`runtime.accessibility_snapshot` returns at least:

- `appId`;
- document title;
- landmarks with role/name;
- headings with level/name;
- controls with role/name/test id when available.

`runtime.run_accessibility_audit` returns a report with `appId`, `checkedAt`, `status`, and `checks`. `runtime.assert_accessibility` must support at least the rule `no_unlabeled_controls`.

## 4. Browser/Manual Checks

Some requirements cannot be trusted from static HTML alone. Before a release can claim broader accessibility coverage, a browser-backed or manual pass must verify:

- keyboard tab order and focus visibility;
- dialog focus trap and restoration;
- status announcements and live regions;
- responsive text reflow;
- color contrast;
- behavior after runtime errors and permission denials.

Unchecked browser/manual items are unknown, not passed. Do not mark them complete in `docs/10_ACCEPTANCE_CHECKLIST.md` unless the relevant target and test path were actually run.

## 5. Install And Repair Behavior

Dev installs may accept warnings, but they must preserve the accessibility report in install/test output so Codex can repair it.

Bundled and production release builds must fail installation when any required v0.4 automated check returns `fail`.

Codex repair loops must treat accessibility failures as first-class bugs:

1. run package validation;
2. run static policy audit;
3. run `runtime.run_accessibility_audit`;
4. run app smoke tests and affected microtests;
5. patch the smallest app files needed;
6. rerun the failed checks before enabling the app version.

## 6. Test Fixtures

Accessibility fixtures live under `tests/accessibility/` and are executable microtests. Every bundled app must pass:

- `runtime.run_accessibility_audit`;
- `runtime.accessibility_snapshot`;
- `runtime.assert_accessibility` with `rule = "no_unlabeled_controls"`.

The primary regression test is:

```text
node --test --no-warnings tools/fake-platform-host/test/accessibility.test.js
```

The full fake-host test suite also runs accessibility checks through package smoke, microtests, and repair-loop coverage.

## 7. Prompt Requirement

AI generation prompts must explicitly require accessible, semantic HTML, keyboard-friendly controls, and stable `data-testid` selectors. Repair prompts must include accessibility failures with the check id, selector when available, and the expected semantic fix.
