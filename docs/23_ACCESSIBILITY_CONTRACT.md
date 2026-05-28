# Accessibility Contract

## 1. Purpose

AI-generated UI must be testable and usable. Accessibility is not optional because the platform will generate many apps and bad defaults will multiply.

## 2. Required generated-app rules

Generated apps must:

- use semantic HTML where possible;
- provide labels for all inputs;
- provide accessible names for all buttons and controls;
- keep focus visible;
- support keyboard navigation for interactive controls;
- avoid pointer-only interactions;
- use dialogs that trap focus and restore focus on close;
- expose status changes through visible text and, when available, ARIA live regions;
- pass contrast thresholds for normal and dark themes;
- avoid text inside images unless redundant text exists.

## 3. Runtime support

The runtime should provide:

- focus manager;
- dialog primitive;
- toast/status primitive;
- accessibility tree snapshot in dev mode;
- automated audit tool in the control plane.

## 4. Required test signals

Micro-tests should verify:

```text
runtime.accessibility_snapshot
runtime.run_accessibility_audit
runtime.assert_visible
runtime.press_key
runtime.assert_text
```

Use `schemas/accessibility-report.schema.json` for audit reports.

## 5. Install gate

The app installer may accept apps with non-critical accessibility warnings in dev mode, but production/bundled release builds should fail on:

- unlabeled inputs;
- buttons without accessible names;
- missing document title/screen title;
- keyboard traps;
- severe color contrast failures.

## 6. Prompt requirement

AI generation prompts must explicitly require accessible, semantic HTML and keyboard-friendly controls. Codex repair prompts must include accessibility failures as first-class bugs.
