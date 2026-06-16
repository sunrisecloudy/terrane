# RFC3339 parser follow-up

Date: 2026-06-16
Branch: `forge-m0a`
Slice: Claude review 081 follow-up

## Result

- Tightened the signing trust RFC3339 parser so impossible calendar dates are
  rejected with month/leap-year-aware day bounds.
- Advanced the offset cursor for `Z`/`z` and numeric offsets, then rejects
  trailing characters after the UTC offset.
- Added regressions for `2026-02-31T00:00:00Z`,
  `2026-04-31T00:00:00Z`, non-leap `2026-02-29T00:00:00Z`, leap
  `2028-02-29T00:00:00Z`, and trailing junk after both `Z` and numeric offsets.

## Verification

```text
cargo test -p forge-signing trust
cargo clippy -p forge-signing -- -D warnings
```

