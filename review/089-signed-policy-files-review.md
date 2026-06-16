# Review 089 - commits 7ef5a279, 4994ef7b, 333863c2

## Findings

1. [P2] Validate signed `capabilities.net[]`, not only `networkPolicy.allow[]`.

   `reject_unknown_signed_policy_fields` now fail-closes unknown `capabilities`
   namespaces, `storage`/`db` action keys, `networkPolicy.allow[]` rule keys, and
   `resourceBudget` keys (`forge/crates/core/src/workspace.rs:2418-2445`), which
   covers the new budget regression. But the signed policy hash explicitly covers
   `{resourceBudget, networkPolicy, capabilities}` (`forge/fixtures/signing/README.md:20`),
   and the new signed net fixture carries a policy-bearing `capabilities.net[]`
   array (`forge/fixtures/signing/bind_net_rule.json:22-33`). That array is
   allowed as a namespace but never schema-checked; the later net bind only reads
   `networkPolicy.allow[]` (`forge/crates/core/src/workspace.rs:2493-2519`).

   A package can therefore put a future/tighter net constraint under
   `capabilities.net[]` while keeping today's `networkPolicy.allow[]` shape, and
   this core would still install it as `Signed` without enforcing or refusing that
   signed capability field. Please either reject `capabilities.net` until it is a
   fully understood shape, or validate each `capabilities.net[]` entry with the
   same known-key set as `networkPolicy.allow[]` and add a regression with an
   unsupported key in `capabilities.net[]`.

## Notes

- `4994ef7b` commits the T028 `ctx.files` spec and 12 fixture vectors from the
  prior Codex handoff; I did not find a new actionable issue there.
- `333863c2` closes review 087's remaining T026 count mismatch; no new issue.
