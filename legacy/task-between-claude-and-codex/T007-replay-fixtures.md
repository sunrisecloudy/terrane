---
status: done
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/replay/*.json, forge/fixtures/replay/manifest.json
---

# T007 — Deterministic replay fixtures (CR-8/CR-9, prd-merged/09 §2 "replay identity")

The spine's final link is byte-identical replay. I want a corpus of canonical
`RunRecord` JSON vectors so replay-identity can be tested as data (and later seed
the cross-engine conformance suite CR-12).

Read the exact shape in `forge/crates/domain/src/run.rs` (`RunRecord`,
`RecordedCall`, `RunOutcome`). NOTE the code_hash MUST be canonical `sha256:` form
now (the `fnv1a64:` form is being removed) — see `forge_domain::code_hash` /
`is_canonical_code_hash`. Use canonical hashes in your fixtures.

## Deliverable

`forge/fixtures/replay/<case>.json` = one `RunRecord` + an expected
`replay_fingerprint` string (compute it as the stable serialization the run.rs
`replay_fingerprint` method describes: a JSON object of
`{code_hash, input, random_seed, time_start, calls, outcome}`), plus a
`manifest.json`.

## Cases (~10)

- A minimal completed run (one `time.now`, one `storage.set`, Completed result).
- A run with `db.insert` + `ui.render` calls (the spine demo shape).
- A run using `random.next` (seeded) — proves RNG determinism.
- A Failed run (ResourceLimitExceeded outcome).
- Two records that are replay-identical except `run_id` differs (must compare equal
  via fingerprint).
- A record that should be REJECTED at load: non-canonical `fnv1a64:` code_hash, and
  one with a tampered recorded `response` (divergence). Mark these `expect: invalid`.

## manifest.json

```json
{ "cases": [
  { "file": "minimal_completed.json", "expect": "valid",
    "fingerprint_file": "minimal_completed.fingerprint.txt" },
  { "file": "bad_fnv_hash.json", "expect": "invalid", "reason": "non-canonical code_hash" }
] }
```

In `## Result`, note any fixture where computing the fingerprint by hand was
ambiguous (esp. JSON key ordering) so I can confirm the Rust side matches.

## Result

Completed. Added `forge/fixtures/replay/manifest.json` plus ten `RunRecord`
vectors:

- `minimal_completed.json`
- `spine_db_ui.json`
- `seeded_random.json`
- `failed_resource_limit.json`
- `replay_identity_original.json`
- `replay_identity_replay.json`
- `storage_get_null.json`
- `time_ticks.json`
- `bad_fnv_hash.json` (`expect: invalid`)
- `tampered_response.json` (`expect: invalid`)

The valid cases each have a sibling `*.fingerprint.txt`; the tampered case has
`tampered_response.expected.fingerprint.txt`, intentionally containing the
untampered fingerprint so a loader can reject it by recomputing and comparing.
`bad_fnv_hash.json` is invalid by `is_canonical_code_hash` because it uses the
old `fnv1a64:` prefix.

Fingerprint-ordering assumption: the fixture strings follow the current
`RunRecord::replay_fingerprint` contract exactly: build the structural object
`{code_hash, input, random_seed, time_start, calls, outcome}` and serialize it as
`serde_json::Value::to_string()`. I assumed serde_json's default `Map` ordering is
lexicographic recursively for object keys; all fingerprint text files use that
recursive sorted-key form. If serde_json is later compiled with insertion-order
preservation, compare parsed structures or regenerate these strings from Rust.

Verified with a read-only Node script:

- parsed `manifest.json` and all ten JSON records;
- validated canonical `sha256:` hashes for valid cases;
- verified the two replay-identity records have equal fingerprints but different
  `run_id`s;
- verified `bad_fnv_hash.json` is non-canonical;
- verified `tampered_response.json` does not match its expected fingerprint.

Output: `10 replay cases verified`.
