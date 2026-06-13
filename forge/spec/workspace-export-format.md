# Workspace Export Format

Source of record: prd-merged/02 DL-24. v1 exports are a single SQLite workspace file. Legacy docs/29 described a JSON backup format; that document is superseded for forge/ except as a checklist for deterministic ordering and excluded local state.

## Container

The canonical export artifact is the SQLite database file itself, with an export_format_version stored in meta. Import validates the schema version, opens the database read-only first, then copies or migrates into the target workspace file inside a transaction.

## Current forge-storage Tables Included

| Table | Included | Deterministic ordering for tests | Notes |
|---|---|---|---|
| meta | yes | key | format version, workspace ids, schema registry pointers |
| kv | yes | namespace, key | ctx.storage data; tombstones retained |
| oplog | yes | lamport, op_id | append-only rebuild source |
| crdt_chunks | yes | doc_id, created_at, chunk_id | append-only CRDT op chunks |
| crdt_snapshots | yes | doc_id, created_at, snapshot_id | snapshot accelerator, not sole source of truth |
| records | yes | collection, id | projection; import must match byte-identical projection after rebuild |
| run_logs | policy-dependent | run_id, seq | include only when export policy allows logs |
| runs | policy-dependent | created_at, run_id | deterministic replay records; may be excluded for privacy policy |

## Required Future Sections Not Yet Persisted

DL-24 also names applet sources, applet manifests/signatures, schema registry CRDT document, index definitions, RBAC config, permissions, and marketplace provenance. The current M0a storage schema does not yet persist dedicated rows for those sections. Until those tables land, export descriptors should mark them as missing_required_for_ga.

## Excluded Data

Secrets are never exported as plaintext. Local-only settings, local window state, transient locks, process ids, in-flight network responses, and provider credentials are excluded. Secret references may be exported only as redacted refs if policy allows.

## Versioning

meta.export_format_version is the open-format version. meta.forge_storage_schema_version is the physical schema version. Importers may read older versions only through explicit migration code; they must not silently reinterpret unknown tables.

## Re-import Invariant

A re-import must reproduce a byte-identical records projection after replay/rebuild from oplog and CRDT chunks. Snapshot bytes may be regenerated, but records.data, collection/id membership, live kv values, tombstones, and run records included by policy must compare equal under the deterministic ordering above.
