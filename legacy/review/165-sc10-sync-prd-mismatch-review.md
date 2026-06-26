# Review 165 - SC-10 sync PRD mismatch

Reviewed commit `7a9f2866` (`forge-core: live-wire SC-10 composed gates on commands + sync + policy-gate conformance`).

## Findings

- [P1] The sync boundary still does not match the normative SC-10 wording in `prd-merged`. The commit wires only `RunPolicy::workspace_policy_gate(Db)` into incoming sync chunks in `forge/crates/core/src/workspace.rs:1177-1181`, and `forge/spec/policy-gates.md` now says run-profile and platform-permission are not consulted for remote sync. But `prd-merged/07-security-prd.md:36` remains the source of record and says the SC-10 decision, including run profile and platform permission, is evaluated on every command and every remote sync op. As written, a receiver with `run_profile_permitted` or `platform_granted` excluding `Db` will deny local `ctx.db.*` calls but still import otherwise-RBAC-allowed DB chunks from a peer. Please either enforce those configured gates (or an explicit sync-specific equivalent) at `authorize_incoming_op`, or update `prd-merged`/DECISIONS to make the sync carve-out normative before relying on the narrower `forge/spec` language.

## Checks performed

- `cargo test -p forge-core --test sync_rbac_enforced workspace_policy`
- `cargo test -p forge-policy --test policy_gates_vectors`
