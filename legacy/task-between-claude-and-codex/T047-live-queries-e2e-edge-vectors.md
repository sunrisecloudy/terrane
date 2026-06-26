---
status: completed
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
Delivered `forge/fixtures/live-queries-e2e/` with `manifest.json` plus 12 edge
vectors:

- `rollback_discards_dirty_set_no_notify`
- `different_filters_targeted_notifications`
- `unwatch_during_pending_batch_suppresses_delivery`
- `watch_registered_after_mutation_has_no_history`
- `transact_three_records_two_collections_coalesces`
- `filtered_enter_then_leave_same_transaction_no_notify`
- `delete_watched_record_result_excludes_deleted`
- `schema_change_on_watched_collection_defined_behavior`
- `monotonic_versions_and_shared_transaction_version`
- `no_op_patch_still_dirties_watched_row`
- `replay_session_notifications_byte_identical`
- `reentrant_callback_mutation_queued_next_turn`

Pinned decisions for the Rust wiring:

- Re-entrancy: notification delivery is non-reentrant. If a watch callback
  mutates, queue that mutation as the next event-loop turn after the current
  delivery batch; it receives a later watch version and never recursively flushes
  inside the same batch.
- No-op mutation: a committed `patch`/`update` dirties its target id even when
  values are identical, because DL-16 defines dirtying by write operation rather
  than by implementation-dependent deep equality. Delivery still follows normal
  filter semantics.
- Schema change on watched collection: v1 schema changes are additive-only.
  Additive changes do not emit `db.watch.notification` by themselves and keep
  watches active; destructive collection drops are rejected as
  `SchemaCompatibilityError` before watch invalidation.

Validation: JSON syntax checked for all 13 files in the suite.
