# Review 164 - SC-10 sync boundary

Reviewed commit `a9c3820d` (`forge-core: live-wire the SC-10 trusted-source gates (T037 FIX ROUND 2)`).

## Findings

- [P1] `RunPolicy` is wired into live applet commands, but remote sync imports still bypass the SC-10 workspace/run/platform gates. `decision_context_for_run()` is only installed for `runtime.run`, `ui.dispatch_event`, and watch notification delivery; the apply-time sync path still authorizes each incoming chunk solely through `authorize_remote_op(trusted, None, &env)` in `forge/crates/core/src/workspace.rs:1157-1166`. That means a receiver can configure `RunPolicy { workspace_denied: [Db], ... }` and local `ctx.db.*` calls will be denied, while an incoming peer chunk that touches the same DB collection can still import if `sync_membership` allows it. This contradicts `prd-merged/07-security-prd.md` SC-10 and `forge/spec/policy-gates.md`, which both say decisions are evaluated on every command and every remote sync op. Please either thread the trusted run policy/SC-10 category gates into `authorize_incoming_op` (record writes map to the `Db` category) with a regression where a `workspace_denied: [Db]` receiver skips an otherwise RBAC-allowed chunk, or explicitly narrow the spec if sync is intentionally governed only by SS-7 RBAC in M0a.

## Checks performed

- `cargo test -p forge-core --test policy_gates_live`
