# PRD 02 — Data Layer (Loro CRDT + dynamic-schema relational store on SQLite)

**Status:** Merged draft v1 · **Depends on:** 01 · **Depended on by:** 03, 04, 05
**Sources:** F-02 (CRDT mapping, projection, query DSL, compaction) + P-05 (physical schema, record envelope, compatibility rules, limits, export) + P-06 (document model, time travel, tombstones)

## 1. Purpose

A local-first data engine where (a) the source of truth is CRDT documents so any device or collaborator can write offline and merge conflict-free; (b) applications see a **relational model with a dynamically updatable schema** stored on SQLite used as a KV/oplog substrate; (c) **old clients can always read new data** and never destroy fields they don't understand; (d) the workspace is a portable, inspectable, single-file SQLite database with a public format spec.

## 2. Concepts

- **Workspace** — unit of sync, membership, and export. Contains collections, applets, schema registry, settings. Exports as one SQLite file (DL-22).
- **Collection** — named set of records (≈ logical table).
- **Record** — CRDT map of `field_id → value`, stored in a forward-compatible envelope.
- **Schema registry** — itself a CRDT document; schema changes sync exactly like data changes.
- **Projection** — derived, queryable SQLite representation of CRDT state; rebuildable at any time.
- **Run/audit records** — append-only execution and audit data (PRD 01 CR-9, PRD 07).

## 3. CRDT engine

- **DL-1** Engine: **Loro** (Rust-native; map/list/movable-list/text/tree; shallow snapshots). Abstracted behind a thin `CrdtDoc` trait; fallback (Automerge 3) evaluated at M0 exit.
- **DL-2** Document granularity (merged F + P-06 — never one giant doc):
  `workspace_manifest_doc · schema_registry_doc · collection_doc:<id> (sharded) · applet docs: manifest + src/<file> + tests/<file> · file_tree_doc · app_state_doc:<applet>:<scope> (opt-in) · chat_doc:<thread> · settings_doc(workspace-scoped)`.
  Collections shard at 2,000 records or 4 MB snapshot; shard index lives in the registry. Local-only device settings, secrets, and caches are never synced.
- **DL-3** Field type in the registry selects merge semantics: scalar → LWW register; text → collaborative text; list → movable list; counter → counter. The schema author (often the LLM) chooses merge behavior by choosing field type.

## 4. SQLite physical layout (P-05 substrate + F projection)

The physical schema changes rarely; logical schemas are data, not DDL.

```sql
meta(key PK, value, updated_at)
kv(namespace, key, value, content_type, logical_version, updated_at, tombstone, PK(namespace,key))
oplog(op_id PK, actor_id, workspace_id, lamport, hlc, kind, payload, schema_ref, created_at,
      redaction_class DEFAULT 'normal')
crdt_snapshots(doc_id, snapshot_id, format, payload, frontier, created_at, PK(doc_id,snapshot_id))
crdt_chunks(doc_id, chunk_id, format, payload, start_frontier, end_frontier, created_at, PK(doc_id,chunk_id))
schema_defs(schema_id PK, name, version, definition, compatibility, updated_at, tombstone)
tombstones(entity_id PK, entity_kind, deleted_by, deleted_at, purge_after, reason)
attachments(attachment_id PK, content_hash, media_type, byte_len, storage_class, payload, external_ref, created_at)
run_logs(run_id, seq, level, event_type, payload, created_at, PK(run_id,seq))
audit_log(audit_id PK, actor_id, action, resource, decision, payload, created_at)
-- Projection (derived, rebuildable):
records(collection, id, data TEXT /* JSON, queried via JSON1 json_extract */, updated_at, PK(collection,id))
```

(SQLite has no native `JSONB` column type; the projection stores canonical JSON `TEXT` and relies on the JSON1 functions for extraction and expression indexes. A Postgres `JSONB` projection is a server-scale option later, not part of the workspace file format.)

- **DL-4** Writes: mutation → CRDT op → append `crdt_chunks` + `oplog` → apply to `records` projection — **one SQLite transaction** (WAL). Remote updates follow the identical path; projection is always consistent with merged CRDT state.
- **DL-5** Dynamic indexes: registry-declared `indexed` fields create expression indexes (`json_extract(data,'$.<field_id>')` partial per collection); full-text fields register into FTS5 shadow tables. Index lifecycle `proposed → building → active → stale → rebuilding → deprecated → removed`; builds are online, idempotent, resumable, interruptible (P-05). Planner warns on full-scan fallback.
- **DL-6** Projection rebuild (`forge db rebuild`) reconstructs `records` and all indexes purely from CRDT docs; must complete with zero diffs in CI soak. This is the corruption-recovery and upgrade escape hatch.

## 5. Record envelope & forward compatibility (normative; merged F-02 §5 + P-05 rules)

Every record round-trips an envelope: `envelope_version, entity_id, collection, schema_id, schema_version, field_ids{...}, unknown_fields{...}, extensions{...}, crdt{doc_id, frontier}, created/updated (logical), deleted, purge_policy`.

- **DL-7** Every field has a **stable `field_id`** never reused (per-actor id ranges: actor-id ⊕ counter), plus a display name; renames touch only the name.
- **DL-8** Schema changes are **additive-only** in v1: add collection/field, widen type, add index, deprecate (hide). Destructive ops are not exposed; "delete" = deprecate + retain.
- **DL-9** **Unknown-field preservation:** clients persist and round-trip unrecognized `field_id`s and schema features; read-modify-write never strips them; UI renders them via the fallback component (PRD 05 UI-6). Unknown features surface as capability warnings, not errors.
- **DL-10** **Unknown-collection tolerance:** clients sync, store, and raw-query collections they have no applet for.
- **DL-11** Registry versions are CRDT vectors, not linear numbers — two offline users adding different fields merge to the union by construction.
- **DL-12** Defaults & validation (`default`, `required-for-write`, regex/range) live in the registry, enforced at the `db` host API; **new constraints default to warning mode before enforcement mode** (P-05 rule 8); old data stays readable (validate-on-write).
- **DL-13** Logical migrations are oplog operations, never destructive SQLite DDL. Lens transforms (breaking changes) deferred to v2; registry reserves `lenses[]` now.
- **DL-14** Older clients may open newer workspaces in **limited mode** when minimum features are unsupported (capability negotiation, P-05 rule 5).

## 6. Query & mutation API (applet-facing)

- **DL-15** Typed query DSL in `@forge/std`, compiled to SQL over the projection: `db.from('expenses').where(f => f.amount.gt(100)).orderBy('date','desc').limit(50)` — filter/sort/limit/offset/aggregates (count,sum,avg,min,max)/group/text-search/joins on declared reference fields. A SQL-like string form (`query.execute`) with the same validated subset serves the data browser and SDK.
- **DL-16** Live queries: `db.watch(query, cb)` via SQLite update hooks + dirty sets; p95 notify < 30 ms local.
- **DL-17** Mutations: `insert/update/patch/delete`; `transact([...])` groups into one CRDT commit (atomic locally, merged as a unit). Raw SQL never exposed to applets; SDK escape hatch is dev-machine-only.
- **DL-18** Per-applet scope: grants name collections (row filters v1.x). Per-applet KV (`ctx.storage`) maps to `kv` namespaces.

## 7. History, tombstones, storage management

- **DL-19** Compaction: fold `crdt_chunks` into shallow snapshots at 1 MB / 1,000 entries per doc, preserving history for peers ≤ 30 days behind; older peers full-state resync. Never compact away data an active peer still needs unless workspace policy allows peer reset (P-06).
- **DL-20** **File-level time travel** (v1, P-06): per-doc history view; restore creates a new version, never destructive rollback; per-record change feed (who/when/what) retained 90 days (configurable) powers undo and audit.
- **DL-21** Deletion = tombstone by default (sync-correct). **Hard-purge** only for records with an explicit purge class (sensitive/protected); produces a redacted purge marker sufficient for sync safety; auditable.
- **DL-22** Quotas (user-configurable): defaults 1 GB/workspace local, 100 MB per applet's collections; caps for attachments, run logs, retained chunks/snapshots, cache. Approaching limits → suggest compaction/cleanup/export; never silent deletion. Attachments deduplicated by content hash.

## 8. Durability, export, encryption

- **DL-23** WAL, `synchronous=NORMAL`, idle checkpoint; kill-during-write torture (1,000 cycles) → zero corruption, zero acked-data loss.
- **DL-24** Export/import: the workspace SQLite file (metadata, applet sources, schemas, records, CRDT snapshots/chunks, oplog, index defs, RBAC config; run logs per export policy). Re-import reproduces a byte-identical projection. Public/open format spec published at GA; compatibility fixtures versioned forever (PRD 09).
- **DL-25** At-rest: OS-level encryption assumed; optional SQLCipher for embedded-server data dir; **project-level encryption keys supported** (key material outside the file or user-supplied; explicit lost-key policy); encrypted export option. Workspace-level use of these keys is governed by the explicit **server-visibility mode** (SS-14): server-readable (default, enables server-side features) vs encrypted (server sees ciphertext; those features disabled/degraded).

## 9. Acceptance

- Fuzzed concurrent editing (8 peers, partitions, 1M ops) → byte-identical convergence; projection rebuild diff = 0.
- v1.0 client opens a simulated v3.0 workspace (new fields/collections/reserved lenses) with zero errors and zero data loss on round-trip write; old-client limited mode works on min-feature mismatch.
- 100k-record collection: indexed query p95 < 10 ms desktop, < 50 ms web (OPFS).
- Export A → import B on a different platform: semantically identical; fixture suite (incl. future-unknown-fields, hard-purge, old-snapshot fixtures) green every release.

## 10. Open questions

1. Cross-workspace reference fields (v1: workspace-internal only).
2. Blob sync ceiling (proposal 25 MB; larger = link-only).
3. SQL string subset vs DSL parity scope at v1.
