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

### Enforcement on run admission (the `run_logs` cap is a PRE-FLIGHT gate)

A run record (`runs.record_json`) is part of the `run_logs` category (§1), so the
`run_logs_cap` is **enforced**, not merely reported. But it is enforced as a **pre-flight
admission gate** — *before* a run starts — **not** as a post-execution save gate. The
reason is correctness: every `ctx.db` write an applet makes commits to SQLite
**immediately as the applet runs** (`apply_mutation_crdt`, its own transaction), so the
applet's record writes are durable the instant it executes. A run record is **mandatory**
(CR-9: every execution persists its resulting writes), so gating that record AFTER the
applet ran — rejecting it because `run_logs` is now over cap — would leave the applet's
already-committed writes with **no run record to replay from**: durable, unreplayable side
effects. Reject-not-delete forbids that torn state.

So every run-persistence path — `runtime.run`, `ui.dispatch_event`, and the `db.watch`
callback re-entry — calls `Store::admit_run_or_reject` **before** any applet side effect.
It reads the **committed** `quota_usage` and the trusted `QuotaPolicy` and **REFUSES to
start the run** with the typed `ResourceLimitExceeded` + the compaction/cleanup/export
suggestion when the `run_logs` category has **no headroom** (committed `run_logs` usage
`>= run_logs_cap`). Because nothing has run yet, a rejection leaves **NO** new records,
**NO** UI state, and **NO** callback writes — no torn, unreplayable state
(reject-not-delete). The semantics: a workspace whose run-log budget is exhausted refuses
to START new runs until logs are compacted/exported (reject, never delete). Without any
gate the cap was **report-only**: a tightened cap appeared in `quota.status` while later
runs kept appending run records beyond it.

Once a run is **ADMITTED**, its run record **ALWAYS** persists (`Store::save_run_tx`) —
the mandatory CR-9 record is never dropped after the applet committed writes. The
admission gate may let the mandatory record push `run_logs` up to **one record past the
cap**; that bounded overshoot is acceptable because the record is mandatory and the NEXT
run is then rejected pre-flight (so usage stays bounded at one record over).

The overshoot is bounded to **exactly one** record even when an admitted run drives a
**downstream** run (a `db.watch` callback re-entered by a notification). Two rules keep it
at one:

1. **PRODUCER record before downstream admission.** A producer command that delivers
   live-query notifications (`runtime.run`, `ui.dispatch_event`, and a triggering
   mutation) assigns and **saves its OWN run record BEFORE** any watcher callback is
   admitted. So a downstream callback's pre-flight admission reads the **already-counted**
   producer record, not the stale pre-producer usage — it cannot be admitted *alongside*
   the producer record off the same headroom. (`ui.dispatch_event` was the gap: it
   delivered notifications, admitting/saving a watcher callback run, *before* assigning its
   dispatch run record, so a near-cap dispatch-with-watch could land **two** records past
   the cap. It now saves the dispatch record first, mirroring `runtime.run`.)

2. **A downstream callback admission denial is a SKIPPED delivery, not a producer
   failure.** When a watcher callback re-entered during notification delivery cannot be
   admitted over the `run_logs` cap, its delivery is **skipped** — the callback never runs
   (no `ctx.db` write, no callback run record) — and the producer command (whose own
   durable effects + run record already committed) stays **successful**. Propagating the
   callback's `ResourceLimitExceeded` out of the producer would report a *failed* run/write
   whose triggering side effects in fact landed. The skip is a **recorded decision** (a
   `db.watch.callback_rejected` envelope on the delivered batch, alongside a
   `db.watch.callback_rejected` event) so replay deterministically reproduces the identical
   skipped-callback outcome — the admission gate is a pure function of committed state. The
   recorded decision is kept SEPARATE from the `db.watch.notification` replay stream, which
   stays a pure notification sequence. Only the run-log **admission** denial becomes a skip;
   any other callback error still propagates.

The workspace TOTAL is deliberately **not** part of the admission gate: a run that FAILS
because its `ctx.db` write was rejected at the records boundary (above) committed **no**
durable write (its records transaction rolled back), so it has no unreplayable side
effect — and its *failed* run record is the auditable record of that very rejection, which
must survive **even when the workspace is at `workspace_limit`**. Admitting that recording
on a full workspace is exactly the reject-not-delete contract; gating it on the total
would drop the audit trail of the rejection. The `run_logs` cap is the dedicated DL-22
backstop that bounds run records, and as a pre-flight admission gate it bounds them
without ever stranding a committed write.

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
