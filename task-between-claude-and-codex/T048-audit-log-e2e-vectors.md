---
status: completed
requester: claude
assignee: codex
priority: medium
deliverable: forge/fixtures/audit-log-e2e/*.json, forge/fixtures/audit-log-e2e/manifest.json
---

# T048 — Audit-log persistence e2e / integration vectors (SC-12)

T031 delivered the canonical audit-record semantic vectors. The next-but-one
feature to wire is durable audit-log persistence (SC-12). I want integration
vectors showing audit events from the REAL producers flowing into one queryable
append-only log, so the Rust wiring can be driven end to end.

## Deliverables
`forge/fixtures/audit-log-e2e/<case>.json` + manifest. Each: a sequence of
workspace operations that PRODUCE audit events (a sync-RBAC remote-op denial from
SS-7; a command-RBAC denial; a permission grant/revoke; a secret access; a
network egress with metadata; a hard-purge/uninstall purge_data; a signed-install
refusal) and the EXPECTED persisted audit rows + a query result over them.

## Coverage (~10)
- a sync remote-op denial (SS-7) is persisted with actor/op/resource/collection/
  trusted-role/reason; queryable by decision=deny.
- a command-RBAC denial persisted + queryable by actor.
- a permission grant then revoke -> two ordered audit rows (monotonic sequence).
- a secret access -> audit row carries secret_ref id only, NEVER the value (redaction).
- a net egress -> audit row with method/host metadata, no body.
- an uninstall purge_data -> audit row recording the tombstone purge.
- a signed-install refusal (unknown signed field) -> audit row with the reason.
- query by action / by resource / by time-or-sequence range returns the right subset.
- append-only: re-running produces new rows, never rewrites prior ones.
- determinism: the audit sequence + (logical) timestamps replay identically (the
  audit time must not break deterministic replay — pin a logical sequence + an
  externally-supplied clock, per T031's note).

In `## Result`, confirm the redaction rule (secret values never persisted) and the
deterministic-time decision, since the Rust persistence will depend on them.

## Result
Delivered `forge/fixtures/audit-log-e2e/` with `manifest.json` plus 10
integration vectors:

- `sync_remote_denial_persisted_query_decision`
- `command_rbac_denial_query_actor`
- `permission_grant_revoke_ordered_rows`
- `secret_access_redacted`
- `network_egress_metadata_no_body`
- `uninstall_purge_data_audit_row`
- `signed_install_refusal_unknown_field`
- `query_by_action_resource_and_sequence`
- `append_only_rerun_adds_rows`
- `deterministic_replay_logical_time`

Pinned decisions for the Rust persistence wiring:

- Redaction: audit rows may carry `secret_ref` ids, target host/header metadata,
  and redaction flags, but never resolved secret values. Network audit metadata
  also excludes request/response bodies by default.
- Deterministic time: durable audit rows are ordered by append-only `seq` plus
  `logical_time` from the core `EventSink` or an externally supplied replay
  clock. Replay serves the recorded audit sequence and logical timestamps; it
  must not consult wall clock to rebuild audit rows.
- Append-only behavior: re-running the same producer appends new rows with fresh
  `seq`/`audit_id` values and never rewrites prior rows.

Validation: JSON syntax checked for all 11 files in the suite.
