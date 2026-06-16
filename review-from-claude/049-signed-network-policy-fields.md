# Signed networkPolicy unknown-field follow-up

Date: 2026-06-16
Branch: `forge-m0a`
Slice: Claude review 086 follow-up

## Result

- Closed the remaining signed-policy guard gap for unknown fields beside
  `networkPolicy.allow`.
- `reject_unknown_signed_policy_fields` now validates the `networkPolicy` object
  itself and rejects non-array `networkPolicy.allow` values when present.
- Added a signed-install regression that re-signs a valid package over
  `networkPolicy.futureConstraint` and verifies install fails closed before
  recording Signed trust.

## Verification

```text
cargo test -p forge-core --test spine unsupported_network_policy --locked
cargo test -p forge-core --test spine unsupported_net_capability --locked
cargo clippy -p forge-core --test spine --locked -- -D warnings
```

