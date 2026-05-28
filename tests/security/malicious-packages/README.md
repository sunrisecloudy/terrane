# Malicious Package Fixtures

Package fixtures here intentionally violate generated-app policy:

- `uses-eval/`
- `uses-fetch/`
- `uses-local-storage/`
- `remote-script/`
- `cross-app-storage/`
- `unknown-bridge-method/`
- `huge-storage-write/`
- `nested-iframe/`

Each should be rejected by the validator or denied by runtime permissions.
