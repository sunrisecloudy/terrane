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
(`crdt_write::mutation::write_collection_bucket_tx`): after the incremental chunk
payload is exported and **before** it is persisted,
`enforce_records_write_tx(tx, collection, chunk_bytes)` reads usage + policy off the
same transaction and rejects an over-quota write. Returning the error rolls the
**whole** transaction back, so the chunk, the oplog row, and the projection are never
written — and every existing record stays byte-for-byte intact (reject-not-delete).
The chunk payload size is the write's durable growth, so it is what is charged.

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
