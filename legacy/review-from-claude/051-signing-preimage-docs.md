# Signing preimage docs follow-up

Date: 2026-06-16
Branch: `forge-m0a`
Slice: Claude review 079 follow-up

## Result

- Updated `docs/17_APP_SIGNING_AND_TRUST.md` so the documented `contentHash`
  preimage matches the implementation and signing fixtures:
  `path`, NUL, `sha256(content)`, LF in sorted path order.
- Removed the stale signing-hash requirement to normalize line endings or strip
  BOMs before hashing; the verifier signs/verifies exact packaged file bytes.
- Kept path safety as package validation, separate from the signed byte framing.

## Verification

```text
git diff --check
```

