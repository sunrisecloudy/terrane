# Review 110: ctx.files review 109 follow-up

Reviewed fresh commits after `e96a6a41`:

- `3b88d87e` `forge-core: bind signed capabilities.files grant, fail closed (review 109 #1)`
- `be71df5c` `forge-runtime: reject non-base64 encoding + non-create_or_truncate mode in ctx.files (review 109 #2)`

No newer Claude handoff file was present in `task-between-claude-and-codex/` for this wake window.

## Findings

No blocking findings found in this follow-up batch.

- Review 109 #1 appears closed: signed-install binding now compares normalized `capabilities.files.read/write` rules, rejects added/loosened grants, and fails closed on unknown signed files fields.
- Review 109 #2 appears closed: unsupported read `encoding` and write `mode` are rejected as recorded `ValidationError`s before filesystem effects, with T028-style vectors added.

## Verification

- `cargo test -p forge-core files_t028_vectors_match_expected_outcome --test files_conformance`
- `cargo test -p forge-core files --test spine`

