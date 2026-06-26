# Review 081 - signing digest / expiry follow-up

Reviewed commit: `cc97f3cd forge-signing: app signing/trust (SC-15) — per-file digest + RFC3339 expiry (reviews 079 #1/#3)`.

## Findings

1. **P2 - The new RFC3339 parser still accepts malformed timestamps as valid instants.** `check_policy` now depends on `parse_rfc3339` to reject malformed `manifest.signedAt` / `publisher_trust.valid_until`, but the parser only checks `day` is `1..=31` and never verifies that the UTC offset consumes the entire string (`forge/crates/signing/src/trust.rs:194`, `forge/crates/signing/src/trust.rs:200`, `forge/crates/signing/src/trust.rs:268`, `forge/crates/signing/src/trust.rs:271`, `forge/crates/signing/src/trust.rs:309`). As a result, non-RFC3339 values like `2026-02-31T00:00:00Z` or `2026-06-13T00:00:00Zjunk` are normalized instead of rejected, so malformed signed/trust timestamps can still pass policy in expiry checks. Tighten the parser by validating day-of-month including leap years and requiring `idx + 1 == bytes.len()` for `Z`/`z` or `idx + 6 == bytes.len()` for numeric offsets; add regressions beside the existing malformed timestamp tests (`forge/crates/signing/src/trust.rs:500`, `forge/crates/signing/src/trust.rs:520`).

## Verification

- `git show --check --format=short cc97f3cd` passed.
- `git diff --check 987fca0b..cc97f3cd` passed.
- `cargo test -p forge-signing` passed.
- `cargo clippy -p forge-signing --all-targets -- -D warnings` passed.
- `cargo check -p forge-signing --target wasm32-unknown-unknown --locked` passed.
- `cargo clippy -p forge-core --all-targets -- -D warnings` passed.
