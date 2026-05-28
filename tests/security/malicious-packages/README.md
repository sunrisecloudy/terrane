# Malicious Package Fixtures

Package fixtures here intentionally violate generated-app policy:

- `uses-eval/`
- `uses-fetch/`
- `uses-local-storage/`
- `remote-script/`
- `remote-css-import/`
- `cross-app-storage/`
- `unknown-bridge-method/`
- `parent-window-access/`
- `huge-storage-write/`
- `nested-iframe/`

Each should be rejected by the validator or denied by runtime permissions.
