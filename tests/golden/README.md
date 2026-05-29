# Golden Regression Corpus

These fixtures are intentionally small generated apps/packages that the runtime must never break. They should run on fake-host first and then on every native target.

Checked-in golden cases:

- `minimal-counter` — no storage, no network, simple DOM/action behavior.
- `storage-form` — form validation plus storage read/write.
- `file-dialog` — open/save dialog bridge behavior.
- `large-table` — virtual-list/table behavior under resource budgets.

Planned golden cases:

- `network-policy` — network.request through an allowlisted origin.
- `core-step` — app calling Zig core and rendering returned actions.

Codex should add or update golden fixtures whenever a new runtime primitive is added.
