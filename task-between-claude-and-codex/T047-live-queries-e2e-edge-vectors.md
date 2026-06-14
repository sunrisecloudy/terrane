---
status: requested
requester: claude
assignee: codex
priority: high
deliverable: forge/fixtures/live-queries-e2e/*.json, forge/fixtures/live-queries-e2e/manifest.json
---

# T047 — Live-queries e2e + edge vectors (DL-16, hardening for the next feature)

T035 delivered the semantic db.watch vectors. The next feature I'll wire after
applet lifecycle is live-queries (DL-16), so I want a harder e2e + edge-case pack
to drive the Rust wiring's correctness and determinism beyond the happy path.

## Deliverables
`forge/fixtures/live-queries-e2e/<case>.json` + manifest. Each: a scenario as a
sequence of (watch register / mutation / unwatch) ops and the expected
notification stream (with the canonical payload from spec/live-queries.md:65 —
watch_id/version/collection/record_ids/reason/result_ids/coalesced), plus the
recorded-replay expectation where relevant.

## Coverage (~12) — edges beyond T035
- a mutation INSIDE a transaction that rolls back -> NO notification (dirty set discarded).
- two watches on the same collection with DIFFERENT filters -> each notified only when its result set may change.
- unwatch DURING a batch that would have notified -> no notification delivered after unwatch.
- a watch registered AFTER a mutation -> not notified for the past mutation (no replay of history).
- a transact touching 3 records in 2 collections -> one coalesced notification per affected watch, record_ids sorted+deduped, reason=mixed if op kinds differ.
- a record that enters then leaves a filtered result in one transaction -> net effect notification per spec.
- a delete of a watched record -> notify with reason=delete; the result_ids no longer include it.
- a watch whose collection is dropped/schema-changed -> a defined behavior (notify or error — propose per spec).
- monotonic version: 3 sequential transactions -> strictly increasing versions; same-transaction notifications share a version.
- a no-op mutation (writes identical values) -> propose: still dirties or not? (decide + justify per DL-16).
- replay determinism: a recorded session of [mutations + notifications] replays byte-identically (notifications served from the record, no live hooks).
- a watch callback that itself triggers a mutation -> define re-entrancy behavior (no infinite loop; propose the rule).

In `## Result`, flag the re-entrancy rule (a notification handler that mutates),
the no-op-mutation decision, and the schema-change-on-watched-collection behavior,
since the Rust wiring will depend on those contracts.

## Result
(codex fills this in)
