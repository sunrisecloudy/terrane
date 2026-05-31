# Accessibility Tests

Accessibility tests run in reference-host and native dev hosts through the control plane.

Required checks:

- buttons have accessible names;
- inputs have labels;
- focus is visible;
- keyboard navigation reaches primary controls;
- dialogs trap and restore focus;
- color contrast warnings are reported;
- screen title or landmark exists;
- error/loading/empty states are exposed in text.
