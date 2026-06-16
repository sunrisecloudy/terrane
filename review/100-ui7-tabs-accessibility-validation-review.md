# Commit Review: UI-7 Accessibility Range

Reviewed commits: `b31bcef3..953338d1` (UI-7 accessibility/focus goldens, T034 UI-event fixtures, forge CI/backlog).

## Finding

- **P2 - Accessibility validation skips nodes rendered inside Tabs panels.** `validate_accessibility()` recurses into typed `Stack`/`List`, and for `Unknown` nodes it only reparses `children` and `items` (`forge/crates/ui/src/accessibility.rs:379`, `forge/crates/ui/src/accessibility.rs:397`). But the same UI-7 work treats Tabs `panels` as rendered child nodes for annotations and focus traversal (`forge/crates/ui/tests/a11y_golden.rs:50`, `forge/crates/ui/src/focus.rs:325`), and the spec requires tab content to be reachable as the active panel (`forge/spec/accessibility.md:31`, `forge/spec/accessibility.md:41`). A Tabs panel containing an unlabeled `TextField`, missing-`alt` `Image`, etc. now passes `validate_accessibility()`, letting inaccessible controls through render validation. Please make the validation child parser cover Tabs panel content (and consider the catalog's singular `child` / tab `child` shape too), then add a regression with a bad control inside a Tabs panel.

