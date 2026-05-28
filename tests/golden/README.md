# Golden Regression Corpus

These fixtures are intentionally small generated apps/packages that the runtime must never break. They should run on fake-host first and then on every native target.

Golden cases:

- `minimal-counter` — no storage, no network, simple DOM/action behavior.
- `storage-form` — form validation plus storage read/write.
- `network-policy` — network.request through an allowlisted origin.
- `file-dialog` — open/save dialog bridge behavior.
- `core-step` — app calling Zig core and rendering returned actions.
- `large-table` — virtual-list/table behavior under resource budgets.

Codex should add or update golden fixtures whenever a new runtime primitive is added.
