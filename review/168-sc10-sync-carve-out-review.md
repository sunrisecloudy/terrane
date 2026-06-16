# Review: 1addd4ec SC-10 sync carve-out

## Findings

- No actionable findings. The commit makes the SC-10 remote-sync carve-out normative in `prd-merged/07-security-prd.md` / `prd-merged/DECISIONS.md`, keeps the current sync boundary limited to sync-applicable gates, and adds coverage for db-denied migration chunks plus unrelated-category allows.

## Checks

- `cargo test -p forge-core --test sync_rbac_enforced workspace_policy --offline`
- `cargo test -p forge-core --test sync_rbac_enforced migration --offline`
