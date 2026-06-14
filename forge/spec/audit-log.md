# Durable append-only audit log (SC-12)

> T031 spec. The normative shape + decisions are
> `forge/fixtures/audit-log-e2e/manifest.json`; the behavioral contract is the 10
> case vectors in `forge/fixtures/audit-log-e2e/`. The persistence + query +
> redaction implementation is `forge-storage` (`crates/storage/src/audit.rs`, the
> `audit_log` table in `crates/storage/src/store.rs`).

prd-merged/07-security-prd.md **SC-12**: *"Audit (union of F + P lists): permission
grants/denials, role changes, secret access attempts, network calls (metadata),
filesystem access, AI provider calls + context manifests, marketplace installs,
sync peer changes, hard-purge events, runtime crashes/limit violations,
membership/admin events. Retention configurable; redaction default; secrets never
in logs; per-applet access view for the user (UI-21)."*

Today every security-relevant authorization decision is **emitted** by a real
producer (sync-RBAC `authorize_remote_op`, the command-RBAC denial path, secrets,
`ctx.net` egress, signed-install refusal, applet-lifecycle purge). SC-12 requires
those decisions to be **persisted** as an append-only, queryable log so a user — or
an auditor — can answer "who did what, and was it allowed?" after the fact. This
spec locks the canonical record shape, the append-only + deterministic-ordering
invariants, the query contract, and the redaction rule.

## 1. Canonical record shape

Each audit row carries (manifest `row_shape`):

| field           | type             | meaning |
|-----------------|------------------|---------|
| `audit_id`      | string           | Stable row id minted from `seq` as `audit-{seq:06}`. |
| `seq`           | u64              | Workspace-local **monotonic** append sequence; the deterministic ordering key. |
| `logical_time`  | u64              | Logical timestamp supplied by the caller (EventSink logical clock or an externally supplied replay clock). NEVER a wall-clock read on the persisted path. |
| `producer`      | string           | Subsystem that emitted the row: `sync-rbac`, `command-rbac`, `permission-manager`, `secrets`, `net`, `lifecycle`, `signing`. |
| `action`        | string           | Canonical action, e.g. `sync.record.insert`, `command.runtime.run`, `permission.grant`/`permission.revoke`, `secret.use`, `network.egress`, `applet.uninstalled`, `package.install.refused`. |
| `decision`      | `allow` \| `deny`| The authorization outcome. |
| `actor_id`      | string           | Authenticated actor responsible for the decision. |
| `resource_type` | enum             | `record`, `schema`, `command`, `capability`, `secret`, `network`, `applet`, `package`, or `audit_log`. |
| `resource_id`   | string?          | Stable resource id when present (collection name, command name, capability key, secret_ref, host origin, applet/package id). Nullable. |
| `collection`    | string?          | Record collection when present. Nullable. |
| `reason`        | string           | Human-readable decisive check (the same reason the producer's decision carries). |
| `metadata`      | JSON object      | **Redacted** structured context — never a secret value, request body, or response body (§4). |

`audit_id` is a pure function of `seq` (`audit_id_for_seq`), so the two never
diverge and the id is stable across replay.

## 2. Append-only semantics

The log is **append-only**. There is no UPDATE and no DELETE path in code: the
storage surface exposes only `append_audit_tx` / `append_audit` (write) and
`query_audit` (read). Re-running the same producer operation **appends a new row**
with a fresh `seq`/`audit_id`; it never mutates a prior row. The
`append_only_rerun_adds_rows` vector pins this: a second identical command-RBAC
denial lands `audit-000081` and leaves `audit-000080` byte-for-byte unchanged.

The append happens inside the **caller's transaction** (`append_audit_tx` takes the
open `rusqlite::Transaction`). A decision and its audit row therefore commit — or
roll back — coherently: a denied op whose surrounding transaction rolls back leaves
no orphan audit row, and a committed decision always lands its row. Two appends in
one transaction take **consecutive** seqs (the permission grant+revoke vector lands
`20` then `21`).

## 3. Deterministic ordering + time

`seq` is the single ordering key. It is minted from a **persisted workspace-local
counter** (a KV entry under `__forge/meta`, read-bump-write inside the append
transaction), not from SQLite's `ROWID`, so:

- a caller can **pin the starting sequence** (`set_audit_seq(next_seq)`), exactly as
  each fixture pins `next_seq`; and
- the counter rolls back with the rows on a transaction rollback, so a committed run
  is gap-free.

`logical_time` is **supplied by the caller** — from the runtime EventSink logical
clock during a live run, or from an externally supplied replay clock during replay.
The persistence path NEVER calls a wall clock. This is the determinism contract: a
recorded run, replayed with the same clock, reproduces **byte-identical** audit rows
(the `deterministic_replay_logical_time` vector: `seq` and `logical_time` replay from
the record; the wall clock is not consulted). A wall-clock value may appear in
`metadata` only when a producer explicitly supplies and records it (e.g. a signed
package's `signed_at`); it is data the producer carried, never a clock the audit
path read.

## 4. Redaction (secrets and bodies never persist)

`redact_metadata` is applied to `metadata` on **every** append, before the row is
written, regardless of producer (`forge/spec/secrets.md` SC-13; SC-12 "secrets never
in logs"):

- **Secret values** — any secret-value key (`secret_value`, `value`,
  `resolved_secret`, `secret`) is dropped and a `value_redacted: true` marker is
  stamped. A secret audit row keeps only the `secret_ref` **id** (a `secret_ref` is
  not secret material; the resolved value never persists). The `secret_access_redacted`
  vector asserts `Bearer abc123` / `abc123` never appear in the row.
- **Request / response bodies** — `request_body` / `response_body` (and a bare `body`,
  or a `body` nested under `request`/`response`) are dropped and the matching
  `request_body_redacted` / `response_body_redacted` marker is stamped. A network
  egress row keeps method/scheme/host/path/status but never the bodies. The
  `network_egress_metadata_no_body` vector asserts the request/response payloads
  (`Ada`, `ada@example.com`, `lead-1`) never appear.

Redaction is a pure value→value transform, so it is unit-testable in isolation and is
the **single** chokepoint guaranteeing no producer can persist sensitive material —
even by accident (a producer that mistakenly hands the resolved secret in metadata
still cannot leak it, because the value is stripped on the way to disk).

The walk is **fully recursive through both objects AND arrays** (review 148): a
secret value or body is dropped wherever it appears — under a key
(`{"request": {"body": …}}`) or inside an array element
(`{"attempts": [{"secret_value": …}]}`). Arrays are not a redaction blind spot;
every element is recursed into, so a producer cannot smuggle sensitive material past
redaction by nesting it in a list.

## 5. Query contract

`query_audit(filter)` returns rows **ordered by `seq` ascending** (the deterministic
ordering key). The filter (`AuditQuery`) AND-combines any set fields; an all-`None`
filter returns every row. Supported predicates:

- exact `actor_id`, `action`, `decision`, `resource_type`, `resource_id`, `collection`;
- inclusive `seq` range (`seq_gte` / `seq_lte`);
- inclusive `logical_time` range (`logical_time_gte` / `logical_time_lte`).

A filter that matches nothing returns an **empty** `Vec` (not an error) — the
empty-result path is part of the contract. The `query_by_action_resource_and_sequence`
vector exercises by-action, by-resource, and by-sequence-range; the per-producer
vectors each round-trip an append and a query by the decisive field
(`decision=deny`, `actor_id`, `action`, `resource_type`, `resource_id`).

## 6. Producers (live wiring)

The log must be persisted by the **real** producers, not a disconnected library. Each
manifest case names the producer whose live path must land a row:

| producer            | action(s)                                  | resource_type |
|---------------------|--------------------------------------------|---------------|
| `sync-rbac`         | `sync.record.*` / `sync.schema.change`     | record/schema |
| `command-rbac`      | `command.<name>`                           | command       |
| `permission-manager`| `permission.grant` / `permission.revoke`   | capability    |
| `secrets`           | `secret.use`                               | secret        |
| `net`               | `network.egress`                           | network       |
| `lifecycle`         | `applet.uninstalled`                       | applet        |
| `signing`           | `package.install.refused`                  | package       |

The acceptance bar (T031) is that a **real** sync-RBAC / command-RBAC denial lands a
persisted, queryable row through the live decision path — proving the log is wired to
the producers, not merely tested in isolation.

**Live wiring (landed).** Two production decision paths persist through this log today:

- **sync-RBAC** — `WorkspaceCore::sync_with` authorizes every incoming op against the
  receiver's trusted membership (`authorize_incoming_op`). Each decision (allow AND
  deny — including the malformed-chunk and unknown-peer fail-closed denials) is
  collected during authorization and appended to the **receiver's** `audit_log` in one
  `Store::transact` after `forge_sync` releases the store borrow. The durable row
  shares the EventSink logical clock with the transient `sync.permission_denied` /
  `sync.authorized` event, so both replay deterministically.
- **command-RBAC** — `WorkspaceCore::handle` persists a `command-rbac` deny row on the
  `Err(PermissionDenied)` branch of the CR-A3 gate, before returning the response. The
  append never fails open: an audit-persistence error never turns a deny into an allow.

`crates/core/tests/audit_log_live.rs` proves the bar end-to-end: a real viewer sync
write and a real auditor `runtime.run` each leave a row queryable by
`decision=deny` / `actor_id`, and re-running a denial **appends** (never rewrites)
history. The remaining producers (`secrets`, `net`, `lifecycle`, `signing`,
`permission-manager`) share the same `append_audit_tx` seam and are exercised by the
data-driven harness below; their live call sites land as those subsystems route
through the audit seam.

**Vector harness (landed).** `crates/core/tests/audit_log_e2e.rs` loads
`fixtures/audit-log-e2e/manifest.json` and genuinely asserts every one of the 10 case
vectors — pinning each `next_seq`/`logical_time`, appending through the real storage
substrate (feeding the secret/network producers the RAW secret value / request+response
bodies so redaction is actually exercised), and asserting the appended rows, the query
`result_audit_ids`, the `must_not_contain` redaction guards, the append-only
`existing_rows_unchanged` invariant, and the deterministic replay. It is guarded by
`ran == manifest.count` (10), so a dropped or unhandled vector fails the suite.
