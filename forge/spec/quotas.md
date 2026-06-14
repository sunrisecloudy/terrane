# Workspace quotas (DL-22) — deterministic accounting + reject-not-delete enforcement + attachment dedup

> Spec. The behavioral contract is `forge/fixtures/quotas/` (the manifest + case
> vectors, driven by `forge-storage/tests/quota_fixtures.rs`) plus the unit tests
> in `forge-storage` (`src/quota.rs`).

prd-merged/02-data-layer-prd.md **DL-22**: *"Quotas (user-configurable): defaults
1 GB/workspace local, 100 MB per applet collections; caps for attachments, run
logs, retained chunks/snapshots, cache. Approaching limits → suggest
compaction/cleanup/export; never silent deletion. Attachments deduplicated by
content hash."*

Quotas are a **measure + decide** layer over the substrate that already exists.
Nothing about a quota mutates user data: accounting reads the persisted tables, and
enforcement either lets a write through or **rejects** it. An over-quota write is
blocked at the write boundary; it never deletes, evicts, or compacts to make room —
that choice always stays with the user.

## 1. Size accounting is a PURE function of persisted state

`Store::quota_usage()` returns a `QuotaUsage`:

```jsonc
{
  "workspace_total_bytes": 1234,          // every accounted slice, summed
  "per_applet": [                          // collection storage by owning applet
    { "applet": "tasks", "collections_bytes": 800 }
  ],
  "per_category": [                        // the independently-capped categories
    { "category": "attachments",     "bytes": 0 },
    { "category": "run_logs",        "bytes": 0 },
    { "category": "retained_chunks", "bytes": 434 },
    { "category": "snapshots",       "bytes": 0 },
    { "category": "cache",           "bytes": 0 }
  ]
}
```

Every number is summed directly from bytes already on disk:

| Accounted slice | Source | SQL |
| --- | --- | --- |
| per-applet collections | `records` | `SUM(length(data))` grouped by collection, folded into the owning applet |
| `attachments` | `attachments` | `SUM(byte_len)` over the **deduplicated** rows (once per content hash) |
| `run_logs` | `run_logs` + `runs` | `SUM(length(payload))` + `SUM(length(record_json))` |
| `retained_chunks` | `crdt_chunks` | `SUM(length(payload))` |
| `snapshots` | `crdt_snapshots` | `SUM(length(payload) + length(frontier))` |
| `cache` | `oplog` + `audit_log` | `SUM(length(payload))` + `SUM(length(metadata))` |

The **applet** that owns a collection is the collection-name prefix before the first
`/` (`tasks/inbox → tasks`), or the whole name when it has none (`notes → notes`).

There is **NO wall clock and NO request input** in the accounting path: two reads of
an unchanged store are byte-equal, and a replay of the same writes reproduces the
exact same report (the SC-12 / audit-log determinism lesson, applied to size).

## 2. The policy is TRUSTED config (const default + persisted override)

`QuotaPolicy` carries the workspace limit, the per-applet limit, the per-category
caps, and the approaching-limit threshold. The DL-22 defaults
(`QuotaPolicy::DEFAULT`):

| Limit | Default |
| --- | --- |
| `workspace_limit` | 1 GiB |
| `per_applet_limit` | 100 MiB |
| `attachments_cap` | 512 MiB |
| `run_logs_cap` | 256 MiB |
| `retained_chunks_cap` | 256 MiB |
| `snapshots_cap` | 256 MiB |
| `cache_cap` | 128 MiB |
| `approaching_threshold` | 0.8 (80%) |

Quotas are **user-configurable**: `Store::set_quota_policy` persists an override in
the **local-only** KV namespace `__local/quota` (so it never travels to a peer or an
exported bundle — it is per-install config). The policy is read from that durable
state, **never from the request payload being checked** — a write can never widen
its own quota. An override that fails validation (a zero limit, or a threshold
outside `(0, 1]`) is **rejected** rather than silently disabling enforcement.

## 3. Enforcement = reject, never delete

`Store::check_quota(category, applet, write_bytes)` is a **pure** function of the
current usage, the trusted policy, and the incoming write size. It returns a
`QuotaDecision`:

- **`Ok`** — the projected post-write totals sit below the approaching threshold of
  every relevant limit. The write proceeds.
- **`ApproachingLimit { scope, projected, limit }`** — the write still fits, but the
  projected total reaches the threshold (default ≥ 80%) of some limit. The write is
  **allowed** (non-blocking); the surface surfaces a warning suggesting
  *compaction / cleanup / export*. This is distinct from a hard rejection.
- **`OverQuota { scope, projected, limit }`** — the projected total would exceed a
  limit. The write is **REJECTED**: `over_quota_error()` is a typed
  `ResourceLimitExceeded` naming the scope and suggesting the remedies, and it ends
  with *"no data was deleted"*. No data is deleted or evicted to make room.

The relevant limits for a write are the **workspace** limit, the **per-applet** limit
(when the write is attributed to an applet — a records write), and the touched
**category** cap. The **tightest** limit decides; a hard breach of any limit
dominates a soft warning on another.

### Live wiring (the real write path)

The check runs on the **real** DL-4 records write path, inside the same SQLite
transaction that appends the chunk/oplog/projection
(`crdt_write::mutation::write_collection_bucket_tx`). The chunk, the oplog row, and the
projection are **staged first**, then `enforce_records_write_tx(tx, collection)`
recomputes `quota_usage` off the **same** transaction — now reflecting every staged
slice — and compares the **real post-write totals** against the limits. This charges
**exactly the slices the report counts** (the per-applet `records.data`, the
`retained_chunks`, and the `cache`/oplog), so an *accepted* write can never leave the
workspace over the limit it was checked against (it would otherwise pass on the chunk
bytes alone and then commit additional accounted records + oplog bytes — review 176
P1). Returning the over-quota error rolls the **whole** transaction back, so the chunk,
the oplog row, and the projection are never written — and every existing record stays
byte-for-byte intact (reject-not-delete).

> The DL-6 projection **rebuild** and the migration rewrite do **not** go through this
> boundary: they reconstruct already-accepted history, so an existing (possibly
> over-budget) workspace can always be rebuilt. Enforcement gates *new* writes only.

## 4. Attachments are deduplicated by content hash

`Store::put_attachment(bytes)` stores an attachment **once per content hash**
(`sha256:…`, `forge_domain::content_hash`):

- The **first** put of given bytes writes one `attachments` row (`stored_new = true`,
  `refcount = 1`). It is enforced against the attachments cap + the workspace limit
  first; over quota ⇒ rejected, nothing stored, nothing deleted.
- A **subsequent** put of **identical** bytes stores nothing new — it only bumps the
  refcount (`stored_new = false`). Identical bytes occupy **one** blob and are
  accounted **once** by `quota_usage`, no matter how many records reference them. A
  dedup hit adds no storage, so it is allowed even at quota (it can never push usage
  up).

The whole lookup → enforce → insert/refcount path runs in **one `BEGIN IMMEDIATE`
transaction** (review 176 P2): it takes the writer lock **before** the dedup lookup, so
two file-backed handles cannot both observe the same pre-write headroom and then both
insert distinct blobs that together exceed the cap, and two identical first puts cannot
race into a primary-key error (the second blocks, then dedups against the committed
row). Like the records path, the new-blob branch **stages** the insert and then
enforces against the **real post-insert** attachments + workspace totals.

## 5. Determinism + the lessons this encodes

- **NEVER silent deletion** — an over-quota write is *rejected* with a typed error
  suggesting compaction/cleanup/export; existing data is byte-intact (§3, proven by
  `over_quota_records_write_rejects_on_the_real_path_without_deleting` and the
  `rejected_write_leaves_data_intact` fixture).
- **Determinism** — accounting + the quota decision are pure functions of persisted
  state + the write size, with no wall clock in the replayable path (§1).
- **Dedup** — one stored blob per content hash, refcounted (§4).
- **Approaching-limit** — a non-blocking ≥ 80% warning, distinct from the hard
  rejection (§3).
- **Config is TRUSTED state** — a const default + a persisted local-only override,
  not the request payload (§2).
- **Live-wiring** — the check is enforced on the real DL-4 write path, not a
  disconnected library (§3).

## 6. The forge-core command/host boundary (the LIVE surface)

The storage layer (§1–§4) is the substrate; `forge-core` is where DL-22 is **live
on the real command/host path** an applet and a shell actually use. The behavioral
contract here is `forge/fixtures/quotas-core/` (driven by
`forge-core/tests/quota_core_conformance.rs`), which exercises only the public
[`WorkspaceCore`] surfaces.

### Enforcement on the live `ctx.db` write path

`runtime.run` runs the applet against a `StorageHostBridge` whose `ctx.db.insert /
update / patch / transact` go through the same DL-4 `apply_mutation_crdt` write path
that enforces the quota (§3). So an **over-quota `ctx.db` write is REJECTED at the
host call**: the bridge returns the typed `ResourceLimitExceeded`, the applet's `main`
rejects, and the run is recorded as **failed** with that error as its result
(`run_ok = false`, `result.error.kind = "ResourceLimitExceeded"`, detail ending in
*"no data was deleted"*). The over-quota records write rolled its own transaction back,
so **the prior records and the records usage are byte-for-byte intact and the rejected
record never landed** — reject-not-delete, proven LIVE by the
`over_quota_db_write_rejected_data_intact` vector.

### Enforcement on run-record persistence (the `run_logs` cap)

A run record (`runs.record_json`) is part of the `run_logs` category (§1), so the
`run_logs_cap` is **enforced on run persistence**, not merely reported. Every
run-persistence path — `runtime.run`, `ui.dispatch_event`, and the `db.watch` callback
re-entry — routes its save through `Store::save_run_with_quota_tx`, which STAGES the
run record inside the caller's transaction and then enforces the `run_logs` cap against
the **real** post-write usage (the same stage+recompute discipline as the records-write
gate). Once `run_logs_cap` (which a privileged `quota.set` can tighten) sits below the
next run record's bytes, the save is **REJECTED** with the typed `ResourceLimitExceeded`
+ the compaction/cleanup/export suggestion; the whole transaction rolls back, so the run
record (and any same-txn audit rows) never land and the `run_logs` usage never exceeds
the cap (reject-not-delete). Without this the cap was **report-only**: a tightened cap
appeared in `quota.status` while later runs kept appending run records beyond it.

The workspace TOTAL is deliberately **not** gated on run persistence: a run that FAILED
because its `ctx.db` write was rejected at the records boundary is still recorded as
*failed* (above), and that auditable record of the rejection must survive even when the
workspace is at `workspace_limit` — gating it on the total would drop the audit trail of
the very rejection. The `run_logs` cap is the dedicated DL-22 backstop that bounds run
records.

### The approaching-limit warning (event + field)

A `ctx.db` write that **fits** but pushes a budget at/above the approaching threshold
(default ≥ 80%) is **allowed** and surfaces a non-blocking warning, distinct from the
hard rejection. After the write commits, the bridge computes the post-write status
(`Store::records_write_quota_status`, a pure read with `write_bytes = 0`) and records a
`QuotaWarning { collection, scope, projected, limit, suggestion }`. `runtime.run`
surfaces these two ways: a `quota.approaching` **event** per warning, and the
`quota_warnings` **field** on the response. The `suggestion` is the DL-22 remedy
(compaction / cleanup / export) and, like the rejection, **never** a deletion.

### `quota.status` and `quota.set` commands

| Command | Roles | What it does |
| --- | --- | --- |
| `quota.status` | Owner, Maintainer, Editor, Viewer, Auditor | REPORT `{ usage, policy, approaching }` — the deterministic usage vs. the trusted limits + the budgets already ≥ the threshold (each with the remedy suggestion). A read of trusted persisted state; two reads are byte-equal. |
| `quota.set` | **Owner only** | CONFIGURE the trusted policy override. Payload `{ policy: { …optional fields… } }` overlays onto the current effective policy; the merged policy is validated (non-zero limits, threshold in `(0, 1]`) and persisted in the local-only namespace. |

`quota.set` is **privileged, trust-gated** config: enforcement always reads the policy
from this persisted state, never from the write being checked, so a write can never
widen its own quota — and a non-owner `quota.set` is rejected at the command-RBAC gate
(`quota_set_is_owner_only_trusted_state`). The scope a `quota.status` reports is the
whole workspace, read from trusted state, never named by the payload.
