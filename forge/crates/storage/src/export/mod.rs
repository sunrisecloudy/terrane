//! Workspace export / import — the DL-24 portable single-file workspace.
//!
//! Normative spec: `prd-merged/02-data-layer-prd.md` DL-24 and
//! `forge/spec/workspace-export-format.md`. A workspace exports as a single
//! **portable SQLite file** containing the *syncable* state — the CRDT
//! chunks/snapshots (the source of truth), the oplog, schema/index definitions
//! that are persisted, the records projection, portable `kv` (applet
//! manifests/programs/schema, NOT secrets or device-local keys), and — per a
//! policy flag — the run records + run logs. Re-importing into a **fresh**
//! `Store` and rebuilding the projection from the imported CRDT chunks (DL-6
//! [`Store::rebuild_projection`](crate::Store::rebuild_projection)) reproduces a
//! **byte-identical records projection**.
//!
//! ## Container & format
//!
//! The export artifact is itself a self-describing SQLite workspace file (the
//! spec's "fresh Store written table-by-table"). [`Store::export_workspace`]
//! writes a brand-new `Store` and copies the syncable tables into it **in a
//! fully deterministic order** (table order fixed below; rows ordered by their
//! primary key / the spec's ordering columns), so two exports of the same
//! workspace are byte-identical. A format-version header lives in the bundle's
//! `meta` table:
//!
//! - `export_format_version` — the open-format version ([`EXPORT_FORMAT_VERSION`]).
//! - `forge_storage_schema_version` — the physical schema version
//!   ([`STORAGE_SCHEMA_VERSION`]).
//!
//! [`Store::import_workspace`] opens the bundle read-only, validates the format
//! version (an unknown version is a clean [`CoreError::StorageError`], never a
//! silent reinterpretation), copies every syncable table into a fresh target
//! `Store`, then calls `rebuild_projection` so the `records` table is
//! reconstructed purely from the imported `crdt_chunks`.
//!
//! ## What is exported (included)
//!
//! | Table          | Included | Ordering (deterministic)            |
//! |----------------|----------|-------------------------------------|
//! | meta           | yes      | key                                  |
//! | kv             | portable | namespace, key (secrets/device excluded) |
//! | oplog          | yes      | lamport, op_id                       |
//! | crdt_chunks    | yes      | doc_id, created_at, chunk_id         |
//! | crdt_snapshots | yes      | doc_id, created_at, snapshot_id      |
//! | records        | yes      | collection, id (re-derived on import)|
//! | runs           | policy   | created_at, run_id                   |
//! | run_logs       | policy   | run_id, seq                          |
//!
//! ## What is NEVER exported (excluded)
//!
//! Local-only / secret state is **never** written to the bundle (DL-24): the
//! `kv` namespaces matching [`is_local_only_namespace`] — secrets, provider
//! credentials, and device-local settings/window state — are filtered out at
//! export, and an exclusion guard test pins this. The reserved `__forge/meta`
//! namespace IS portable: it carries applet manifests/programs/schema pointers
//! and the workspace run counter, which are workspace state rather than secrets.
//! (When secret *references* need to travel they would ride as redacted refs;
//! M0a persists no secret rows, so there is nothing to redact yet — the guard is
//! the forward-compatible enforcement point.)
//!
//! ## Re-import invariant
//!
//! After import + rebuild, the target's `records` projection compares **equal**
//! to the source's under the deterministic `(collection, id)` ordering, live
//! `kv` values + tombstones match, indexes rebuild from canonical records, and
//! (when policy includes them) `runs`/`run_logs` round-trip with their order
//! preserved. This leverages DL-6: records are *derived*, so the CRDT chunks are
//! the portable source and the projection is reconstructed, not trusted.
//!
//! This module is split into directory sub-modules (/simplify #8) — `policy`,
//! `guard`, `version`, `table_copy`, `bundle`, `query`, and `transaction` —
//! re-exported here so the public surface stays byte-stable.
//!
//! [`CoreError::StorageError`]: forge_domain::CoreError::StorageError

mod bundle;
mod guard;
mod policy;
mod query;
mod table_copy;
mod transaction;
mod version;

pub use guard::is_local_only_namespace;
pub use policy::{ExportOptions, RunLogPolicy, EXPORT_FORMAT_VERSION, STORAGE_SCHEMA_VERSION};
pub use query::bundle_meta;

// The export/import orchestrators (`bundle`) and the inspection queries (`query`)
// contribute `impl Store` blocks; their inherent methods need no re-export.

#[cfg(test)]
use bundle::set_post_snapshot_test_hook;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{CreateIndexKind, IndexManager};
    use crate::{Mutation, Store};
    use forge_domain::{AppResult, AppletId, RunId, RunOutcome, RunRecord};
    use rusqlite::params;
    use serde_json::json;

    // --- builders ---------------------------------------------------------

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().expect("object").clone()
    }

    fn insert(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Insert {
            collection: collection.into(),
            id: Some(id.into()),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    fn patch(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Patch {
            collection: collection.into(),
            id: id.into(),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    fn delete(collection: &str, id: &str, at: i64) -> Mutation {
        Mutation::Delete {
            collection: collection.into(),
            id: id.into(),
            logical_at: Some(at),
        }
    }

    /// A workspace built through the real DL-4 CRDT mutation path so that the
    /// projection is CRDT-backed (chunks exist for rebuild on import). Returns
    /// the source store and the index manager whose active index must survive a
    /// round-trip.
    fn source_workspace() -> (Store, IndexManager) {
        let mut s = Store::open_in_memory().unwrap();
        let mut idx = IndexManager::new();
        // Records via the CRDT write path (chunks + oplog + projection).
        s.apply_mutation_crdt(
            &insert(
                "notes",
                "n1",
                json!({"title": "Alpha", "body": "offline first"}),
                1,
            ),
            &idx,
        )
        .unwrap();
        s.apply_mutation_crdt(
            &insert(
                "notes",
                "n2",
                json!({"title": "Beta", "body": "sync later"}),
                2,
            ),
            &idx,
        )
        .unwrap();
        s.apply_mutation_crdt(&patch("notes", "n1", json!({"pinned": true}), 3), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 4), &idx)
            .unwrap();
        // A record that is deleted in CRDT history (must not resurrect on import).
        s.apply_mutation_crdt(&insert("tasks", "t9", json!({"title": "Temp"}), 5), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t9", 6), &idx)
            .unwrap();

        // An active value index over a stable field id (rebuilt on import).
        s.create_index(&mut idx, "notes", "f_title", CreateIndexKind::Value)
            .unwrap();

        // Portable kv: an applet manifest stand-in + ctx.storage namespace + the
        // workspace run counter (all portable workspace state).
        s.kv_set(
            "__forge/meta",
            "applet/notes",
            b"{\"manifest\":true}",
            "application/json",
        )
        .unwrap();
        s.kv_set("applet/notes", "draft", b"hello", "text/plain")
            .unwrap();
        s.next_counter("__forge/meta", "run_counter").unwrap();

        // Local-only / secret kv that must NEVER be exported.
        s.kv_set(
            "secret/weather",
            "api_key",
            b"sk-DO-NOT-EXPORT",
            "text/plain",
        )
        .unwrap();
        s.kv_set("provider/openai", "token", b"tok-secret", "text/plain")
            .unwrap();
        s.kv_set(
            "device/window",
            "geometry",
            b"{\"w\":800}",
            "application/json",
        )
        .unwrap();
        s.kv_set("local/ui", "scroll", b"42", "text/plain").unwrap();

        (s, idx)
    }

    fn sample_run(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: RunId::new(run_id),
            applet_id: AppletId::new("app_notes"),
            code_hash: forge_domain::code_hash("body"),
            input: json!({"x": 1}),
            random_seed: 7,
            time_start: 1000,
            calls: vec![],
            logs: vec!["hello".into()],
            permissions: forge_domain::PermissionSnapshot::default(),
            outcome: RunOutcome::Completed {
                result: AppResult {
                    ok: true,
                    value: json!("ok"),
                },
            },
        }
    }

    /// The full projection keyed by `collection/id` → canonical data, ordered, for
    /// a byte-for-byte source/target comparison (the DL-24 invariant).
    fn projection_snapshot(s: &Store) -> std::collections::BTreeMap<String, String> {
        let mut stmt = s
            .connection()
            .prepare("SELECT collection, id, data FROM records ORDER BY collection, id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        let mut out = std::collections::BTreeMap::new();
        for r in rows {
            let (c, id, data) = r.unwrap();
            out.insert(format!("{c}/{id}"), data);
        }
        out
    }

    /// The complete bundle file as bytes, for byte-identical re-export checks.
    fn read_file(path: &std::path::Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    // --- DL-24 core invariant: export -> import -> equal projection -------

    #[test]
    fn import_reproduces_the_source_projection_byte_for_byte() {
        let (src, idx) = source_workspace();
        let before = projection_snapshot(&src);

        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws_demo"))
            .unwrap();
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();

        // The imported projection — re-derived from the imported chunks — equals
        // the source projection exactly.
        assert_eq!(
            projection_snapshot(&target),
            before,
            "DL-24 byte-identical projection"
        );

        // Query results match (same live rows, deleted record stays gone).
        assert_eq!(target.list_records("notes").unwrap().len(), 2);
        assert_eq!(target.list_records("tasks").unwrap().len(), 1);
        assert!(
            target.get_record("tasks", "t9").unwrap().is_none(),
            "deleted record not resurrected"
        );
        // The patched field survived.
        assert_eq!(
            target.get_record("notes", "n1").unwrap().unwrap().fields["pinned"],
            json!(true)
        );
    }

    #[test]
    fn import_rebuilds_the_active_index_from_canonical_records() {
        // The active value index must serve a query against the imported store,
        // proving indexes were rebuilt from canonical records (not copied raw).
        let (src, idx) = source_workspace();
        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws_demo"))
            .unwrap();
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();

        let q = crate::Query::from_fixture_value(&json!({
            "from": "notes",
            "where": [{"field_id": "f_title", "op": "eq", "value": "Alpha"}]
        }))
        .unwrap();
        let planned = target.query_planned(&q, &idx).unwrap();
        assert!(
            planned.uses_index,
            "the imported store's active index must serve the query"
        );
        assert_eq!(
            planned.index_id.as_deref(),
            Some("idx_records_notes_f_title")
        );
    }

    #[test]
    fn kv_live_values_and_tombstones_round_trip() {
        let (src, idx) = source_workspace();
        // Tombstone a key so the export carries the deletion.
        src.kv_delete("applet/notes", "draft").unwrap();

        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws_demo"))
            .unwrap();
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();

        // Portable kv round-trips; the tombstoned key is hidden but its row exists.
        assert_eq!(
            target
                .kv_get("__forge/meta", "applet/notes")
                .unwrap()
                .as_deref(),
            Some(&b"{\"manifest\":true}"[..])
        );
        assert_eq!(
            target.kv_get("applet/notes", "draft").unwrap(),
            None,
            "tombstone hides the value"
        );
        let tomb: i64 = target
            .connection()
            .query_row(
                "SELECT tombstone FROM kv WHERE namespace='applet/notes' AND key='draft'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tomb, 1, "the tombstone row round-trips");
        // The run counter is portable workspace state.
        assert_eq!(
            target
                .kv_get("__forge/meta", "run_counter")
                .unwrap()
                .as_deref(),
            Some(&b"1"[..])
        );
    }

    // --- DL-24 exclusion guard: secrets / device-local NEVER exported -----

    #[test]
    fn secrets_and_device_local_kv_are_never_exported() {
        let (src, idx) = source_workspace();
        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws_demo"))
            .unwrap();

        // Not in the bundle file at all.
        for (ns, key) in [
            ("secret/weather", "api_key"),
            ("provider/openai", "token"),
            ("device/window", "geometry"),
            ("local/ui", "scroll"),
        ] {
            let present: i64 = bundle
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM kv WHERE namespace=?1 AND key=?2",
                    params![ns, key],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                present, 0,
                "secret/device-local {ns}/{key} must NOT be in the bundle"
            );
        }

        // And not in the imported workspace.
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();
        assert_eq!(target.kv_get("secret/weather", "api_key").unwrap(), None);
        assert_eq!(target.kv_get("provider/openai", "token").unwrap(), None);
        assert_eq!(target.kv_get("device/window", "geometry").unwrap(), None);
        assert_eq!(target.kv_get("local/ui", "scroll").unwrap(), None);
    }

    #[test]
    fn local_only_namespace_policy_is_precise() {
        // Secret / device / local buckets are excluded...
        assert!(is_local_only_namespace("secret/weather"));
        assert!(is_local_only_namespace("secrets"));
        assert!(is_local_only_namespace("provider/openai"));
        assert!(is_local_only_namespace("credentials/aws"));
        assert!(is_local_only_namespace("device/window"));
        assert!(is_local_only_namespace("local/ui"));
        // Exact root buckets (a key stored directly under the bucket, no trailing
        // `/<key>` segment) are excluded too — closes the review 061 P2 gap where
        // `secret`, `provider`, `credentials`, `__device`, `__local` slipped past a
        // prefix-only check and were exported.
        assert!(is_local_only_namespace("secret"));
        assert!(is_local_only_namespace("provider"));
        assert!(is_local_only_namespace("credentials"));
        assert!(is_local_only_namespace("__device"));
        assert!(is_local_only_namespace("__local"));
        assert!(
            is_local_only_namespace("secrets/aws"),
            "secrets/ children stay excluded"
        );
        // ...but portable workspace namespaces are NOT excluded.
        assert!(
            !is_local_only_namespace("__forge/meta"),
            "applet manifests/programs are portable"
        );
        assert!(
            !is_local_only_namespace("applet/notes"),
            "applet ctx.storage is portable"
        );
        assert!(
            !is_local_only_namespace("localized"),
            "prefix must be a bucket boundary, not a substring"
        );
        assert!(
            !is_local_only_namespace("secretive"),
            "a longer name sharing a bucket prefix is portable"
        );
        assert!(
            !is_local_only_namespace("providers"),
            "providers (plural) is not the provider bucket"
        );
    }

    // --- DL-24 deterministic re-export: byte-identical --------------------

    #[test]
    fn re_export_of_the_same_workspace_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let (src, _idx) = source_workspace();

        let a = dir.path().join("a.forgews");
        let b = dir.path().join("b.forgews");
        src.export_workspace(&a, &ExportOptions::new("ws_demo"))
            .unwrap();
        src.export_workspace(&b, &ExportOptions::new("ws_demo"))
            .unwrap();

        // Two exports of the same workspace produce byte-identical bundle files.
        assert_eq!(
            read_file(&a),
            read_file(&b),
            "re-export must be byte-stable"
        );
    }

    #[test]
    fn export_refuses_to_overwrite_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let (src, _idx) = source_workspace();
        let p = dir.path().join("ws.forgews");
        src.export_workspace(&p, &ExportOptions::new("ws_demo"))
            .unwrap();
        let err = src
            .export_workspace(&p, &ExportOptions::new("ws_demo"))
            .unwrap_err();
        assert_eq!(err.code(), "StorageError");
    }

    #[test]
    fn import_round_trips_through_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let (src, idx) = source_workspace();
        let before = projection_snapshot(&src);

        let bundle = dir.path().join("ws.forgews");
        src.export_workspace(&bundle, &ExportOptions::new("ws_demo"))
            .unwrap();

        let target_path = dir.path().join("restored.db");
        let target = Store::import_workspace(&bundle, &target_path, &idx).unwrap();
        assert_eq!(
            projection_snapshot(&target),
            before,
            "file round-trip preserves the projection"
        );
    }

    // --- run-log policy ---------------------------------------------------

    #[test]
    fn run_logs_excluded_by_default_included_on_request() {
        let (src, idx) = source_workspace();
        // Seed a run + run_logs.
        src.save_run(&sample_run("run_1")).unwrap();
        src.connection()
            .execute(
                "INSERT INTO run_logs (run_id, seq, level, event_type, payload, created_at)
                 VALUES ('run_1', 0, 'info', 'log', ?1, 0), ('run_1', 1, 'info', 'log', ?2, 0)",
                params![b"a".as_slice(), b"b".as_slice()],
            )
            .unwrap();

        // Default policy: runs / run_logs are NOT exported.
        let excluded = src
            .export_workspace_in_memory(&ExportOptions::new("ws"))
            .unwrap();
        let target_x = Store::import_workspace_in_memory(&excluded, &idx).unwrap();
        assert!(
            target_x.load_run("run_1").unwrap().is_none(),
            "runs excluded by default"
        );
        let log_count_x: i64 = target_x
            .connection()
            .query_row("SELECT COUNT(*) FROM run_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(log_count_x, 0, "run_logs excluded by default");

        // Include policy: runs + run_logs round-trip, log order preserved.
        let included = src
            .export_workspace_in_memory(&ExportOptions::new("ws").with_run_logs())
            .unwrap();
        let target_i = Store::import_workspace_in_memory(&included, &idx).unwrap();
        assert_eq!(
            target_i.load_run("run_1").unwrap().unwrap().run_id.as_str(),
            "run_1"
        );
        let seqs: Vec<i64> = {
            let mut stmt = target_i
                .connection()
                .prepare("SELECT seq FROM run_logs WHERE run_id='run_1' ORDER BY seq")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(seqs, vec![0, 1], "run_logs preserve seq order");
    }

    // --- version handling -------------------------------------------------

    #[test]
    fn bundle_carries_the_format_version_header() {
        let (src, _idx) = source_workspace();
        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws_demo"))
            .unwrap();
        assert_eq!(
            bundle_meta(&bundle, "export_format_version")
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            bundle_meta(&bundle, "forge_storage_schema_version")
                .unwrap()
                .as_deref(),
            Some("1")
        );
        assert_eq!(
            bundle_meta(&bundle, "workspace_id").unwrap().as_deref(),
            Some("ws_demo")
        );
    }

    #[test]
    fn version_mismatch_is_a_clean_error() {
        let (src, idx) = source_workspace();
        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws"))
            .unwrap();
        // Tamper the header to a future version.
        bundle
            .connection()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'export_format_version'",
                params![b"999".as_slice()],
            )
            .unwrap();
        let err = Store::import_workspace_in_memory(&bundle, &idx)
            .err()
            .expect("import must reject an unsupported version");
        assert_eq!(err.code(), "StorageError");
        assert!(
            format!("{err}").contains("999"),
            "the error names the unsupported version: {err}"
        );
    }

    #[test]
    fn missing_version_header_is_rejected() {
        // A SQLite file that is not a forge bundle (no header row) is refused.
        let idx = IndexManager::new();
        let not_a_bundle = Store::open_in_memory().unwrap();
        let err = Store::import_workspace_in_memory(&not_a_bundle, &idx)
            .err()
            .expect("a file with no header is not a bundle");
        assert_eq!(err.code(), "StorageError");
    }

    #[test]
    fn import_refuses_a_non_fresh_target() {
        let dir = tempfile::tempdir().unwrap();
        let (src, idx) = source_workspace();
        let bundle = dir.path().join("ws.forgews");
        src.export_workspace(&bundle, &ExportOptions::new("ws"))
            .unwrap();
        let target_path = dir.path().join("exists.db");
        // Pre-create the target file.
        Store::open(&target_path).unwrap();
        let err = Store::import_workspace(&bundle, &target_path, &idx)
            .err()
            .expect("import must refuse a pre-existing target");
        assert_eq!(err.code(), "StorageError");
    }

    // --- in-place import persists to the target file (review 062 P1 #1) ---

    #[test]
    fn import_workspace_in_place_persists_to_the_same_file() {
        // A file-backed target must actually PERSIST an in-place import: write the
        // bundle, import into a file-backed store, drop it, reopen the SAME path,
        // and still see the imported records / applet kv / grant table. This is the
        // review 062 P1 #1 regression — the in-memory swap reported success but lost
        // everything on reopen.
        let dir = tempfile::tempdir().unwrap();
        let (src, idx) = source_workspace();
        let before = projection_snapshot(&src);
        // Persist a grant table so a non-record namespace travels too.
        src.kv_set(
            "__forge/meta",
            "db_read_grants",
            b"{\"actor\":[\"notes\"]}",
            "application/json",
        )
        .unwrap();

        let bundle_path = dir.path().join("ws.forgews");
        src.export_workspace(&bundle_path, &ExportOptions::new("ws_demo"))
            .unwrap();

        let target_path = dir.path().join("target.db");
        {
            // Open the bundle as a Store and import into a file-backed target.
            let bundle = Store::open(&bundle_path).unwrap();
            let mut target = Store::open(&target_path).unwrap();
            target.import_workspace_in_place(&bundle, &idx).unwrap();
            // Visible in the same handle that imported.
            assert_eq!(
                projection_snapshot(&target),
                before,
                "import populates the live handle"
            );
        } // Drop both stores: the WAL flushes the committed import to disk.

        // Reopen the SAME file path: the imported state is durably present.
        let reopened = Store::open(&target_path).unwrap();
        assert_eq!(
            projection_snapshot(&reopened),
            before,
            "in-place import must survive a reopen of the same file"
        );
        // Records query as expected (deleted record stays gone).
        assert_eq!(reopened.list_records("notes").unwrap().len(), 2);
        assert_eq!(reopened.list_records("tasks").unwrap().len(), 1);
        assert!(reopened.get_record("tasks", "t9").unwrap().is_none());
        // Portable kv survived the reopen too: applet manifest + grant table.
        assert_eq!(
            reopened
                .kv_get("__forge/meta", "applet/notes")
                .unwrap()
                .as_deref(),
            Some(&b"{\"manifest\":true}"[..])
        );
        assert_eq!(
            reopened
                .kv_get("__forge/meta", "db_read_grants")
                .unwrap()
                .as_deref(),
            Some(&b"{\"actor\":[\"notes\"]}"[..])
        );
    }

    #[test]
    fn import_workspace_in_place_rejects_a_bad_version_before_touching_state() {
        // A version-mismatched bundle is refused before any table is copied, so the
        // (fresh) target is left untouched.
        let (src, idx) = source_workspace();
        let bundle = src
            .export_workspace_in_memory(&ExportOptions::new("ws"))
            .unwrap();
        bundle
            .connection()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'export_format_version'",
                params![b"999".as_slice()],
            )
            .unwrap();
        let mut target = Store::open_in_memory().unwrap();
        let err = target.import_workspace_in_place(&bundle, &idx).unwrap_err();
        assert_eq!(err.code(), "StorageError");
        assert!(
            target.is_empty_target().unwrap(),
            "a rejected import must not populate the target"
        );
    }

    // --- is_empty_target across every importable namespace (062 P1 #2) ----

    #[test]
    fn is_empty_target_true_on_a_fresh_store() {
        let s = Store::open_in_memory().unwrap();
        assert!(
            s.is_empty_target().unwrap(),
            "a brand-new store is a fresh import target"
        );
    }

    #[test]
    fn is_empty_target_false_when_any_importable_table_is_populated() {
        // Each importable physical table independently makes the target non-fresh.
        // records:
        {
            let s = Store::open_in_memory().unwrap();
            s.put_record(&{
                let mut e = crate::RecordEnvelope::new(
                    forge_domain::CollectionId::new("notes"),
                    forge_domain::RecordId::new("n1"),
                    Default::default(),
                    forge_domain::LogicalTimestamp(1),
                );
                e.fields.insert("title".into(), json!("x"));
                e
            })
            .unwrap();
            assert!(
                !s.is_empty_target().unwrap(),
                "a projected record is importable state"
            );
        }
        // crdt_chunks:
        {
            let s = Store::open_in_memory().unwrap();
            s.put_chunk("collection/notes", "c1", "loro", b"x").unwrap();
            assert!(
                !s.is_empty_target().unwrap(),
                "a crdt chunk is importable state"
            );
        }
        // crdt_snapshots:
        {
            let s = Store::open_in_memory().unwrap();
            s.put_snapshot("collection/notes", "s1", "loro", b"x", b"f")
                .unwrap();
            assert!(
                !s.is_empty_target().unwrap(),
                "a crdt snapshot is importable state"
            );
        }
        // oplog:
        {
            let s = Store::open_in_memory().unwrap();
            s.append_op("op1", "a", "ws", 1, "insert", b"p").unwrap();
            assert!(
                !s.is_empty_target().unwrap(),
                "an oplog row is importable state"
            );
        }
        // runs + run_logs:
        {
            let s = Store::open_in_memory().unwrap();
            s.save_run(&sample_run("run_1")).unwrap();
            assert!(
                !s.is_empty_target().unwrap(),
                "a run row is importable state"
            );
        }
        {
            let s = Store::open_in_memory().unwrap();
            s.connection()
                .execute(
                    "INSERT INTO run_logs (run_id, seq, level, event_type, payload, created_at)
                     VALUES ('run_1', 0, 'info', 'log', ?1, 0)",
                    params![b"a".as_slice()],
                )
                .unwrap();
            assert!(
                !s.is_empty_target().unwrap(),
                "a run_log row is importable state"
            );
        }
    }

    #[test]
    fn is_empty_target_false_for_a_grants_only_or_applet_only_workspace() {
        // A workspace whose ONLY content is a portable __forge/meta kv entry (the
        // grants-only / kv-only case the review calls out) is NOT fresh: the grant
        // table / applet manifest would be silently shadowed by an import.
        let grants_only = Store::open_in_memory().unwrap();
        grants_only
            .kv_set("__forge/meta", "db_read_grants", b"{}", "application/json")
            .unwrap();
        assert!(
            !grants_only.is_empty_target().unwrap(),
            "a grants-only workspace must not pass the fresh-target check"
        );

        let applet_only = Store::open_in_memory().unwrap();
        applet_only
            .kv_set("applet/notes", "draft", b"hello", "text/plain")
            .unwrap();
        assert!(
            !applet_only.is_empty_target().unwrap(),
            "an applet ctx.storage namespace is importable workspace state"
        );
    }

    #[test]
    fn is_empty_target_ignores_purely_local_only_kv() {
        // Local-only / secret namespaces never travel in a bundle, so a store whose
        // ONLY content is such a key is still a valid (fresh) import target — counting
        // it would wrongly refuse a genuinely importable workspace.
        let s = Store::open_in_memory().unwrap();
        s.kv_set("secret/weather", "api_key", b"sk-x", "text/plain")
            .unwrap();
        s.kv_set("device/window", "geometry", b"{}", "application/json")
            .unwrap();
        s.kv_set("local/ui", "scroll", b"42", "text/plain").unwrap();
        assert!(
            s.is_empty_target().unwrap(),
            "a store holding only non-exportable local-only kv is still a fresh target"
        );
        // Add ONE portable key and it flips to non-fresh.
        s.kv_set("applet/notes", "draft", b"hi", "text/plain")
            .unwrap();
        assert!(
            !s.is_empty_target().unwrap(),
            "a portable kv row makes it non-fresh"
        );
    }

    #[test]
    fn is_empty_target_counts_a_tombstoned_portable_kv_row() {
        // An exported-then-tombstoned portable key is still importable state: the
        // tombstone row travels in the bundle, so a fresh target must not shadow it.
        let s = Store::open_in_memory().unwrap();
        s.kv_set("applet/notes", "draft", b"hi", "text/plain")
            .unwrap();
        s.kv_delete("applet/notes", "draft").unwrap();
        assert!(
            !s.is_empty_target().unwrap(),
            "a tombstoned portable kv row is still importable state"
        );
    }

    // --- export reflects a single consistent source snapshot (062 P1 #3) --

    #[test]
    fn export_reflects_a_single_consistent_source_snapshot() {
        // While an export reads the source inside ONE read transaction, a concurrent
        // write on a SECOND connection to the same file must NOT bleed into the
        // bundle: the snapshot is pinned at the export's BEGIN, so the bundle is a
        // single consistent point-in-time view (no split-brain). We model the race
        // deterministically: open a second handle, mutate it AFTER the export's read
        // snapshot has begun, and assert the bundle reflects the pre-write state.
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("src.db");
        let writer = Store::open(&src_path).unwrap();
        // Seed one CRDT-backed record so the bundle has chunks + a projection.
        let idx = IndexManager::new();
        {
            let mut w = Store::open(&src_path).unwrap();
            w.apply_mutation_crdt(&insert("notes", "n1", json!({"title": "Alpha"}), 1), &idx)
                .unwrap();
        }
        // A second, independent handle is the "concurrent writer".
        let mut concurrent = Store::open(&src_path).unwrap();

        // Begin the export's source snapshot manually so we can interleave a write
        // against the live file mid-export, then run the copy against that snapshot.
        // A SQLite read transaction pins its snapshot at the FIRST read, so we issue
        // one read to freeze the snapshot — modelling `write_bundle`, whose first
        // `copy_*` SELECT freezes the snapshot before every later copy runs.
        let exporter = Store::open(&src_path).unwrap();
        let snapshot = exporter.connection().unchecked_transaction().unwrap();
        let _pin: i64 = snapshot
            .query_row("SELECT COUNT(*) FROM crdt_chunks", [], |r| r.get(0))
            .unwrap();
        // CONCURRENT WRITE: lands on the file AFTER the snapshot was frozen. WAL lets
        // the writer commit while the snapshot keeps reading its pinned view.
        concurrent
            .apply_mutation_crdt(&insert("notes", "n2", json!({"title": "Beta"}), 2), &idx)
            .unwrap();

        // The pinned snapshot still sees ONLY n1 (the concurrent n2 is invisible).
        let chunk_docs: i64 = snapshot
            .query_row("SELECT COUNT(DISTINCT doc_id) FROM crdt_chunks", [], |r| {
                r.get(0)
            })
            .unwrap();
        let n1_present: i64 = snapshot
            .query_row(
                "SELECT COUNT(*) FROM records WHERE collection='notes' AND id='n1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let n2_present: i64 = snapshot
            .query_row(
                "SELECT COUNT(*) FROM records WHERE collection='notes' AND id='n2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(snapshot);
        assert_eq!(
            chunk_docs, 1,
            "snapshot pins one doc; the concurrent write is invisible"
        );
        assert_eq!(n1_present, 1, "the pre-write record is in the snapshot");
        assert_eq!(
            n2_present, 0,
            "the concurrent write must not bleed into the snapshot"
        );

        // And the real export entry point produces a bundle that round-trips to a
        // consistent projection (n1 present, importable) — proving write_bundle's
        // snapshot does not corrupt the output.
        let bundle_path = dir.path().join("ws.forgews");
        writer
            .export_workspace(&bundle_path, &ExportOptions::new("ws"))
            .unwrap();
        let restore_path = dir.path().join("restored.db");
        let restored = Store::import_workspace(&bundle_path, &restore_path, &idx).unwrap();
        assert!(
            restored.get_record("notes", "n1").unwrap().is_some(),
            "n1 round-trips"
        );
    }

    #[test]
    fn write_bundle_snapshot_excludes_a_write_that_lands_mid_export() {
        // The LOAD-BEARING regression for review 062 P1 #3: drive the REAL
        // `export_workspace` / `write_bundle` and, via the post-snapshot test hook,
        // commit a concurrent write on a second connection to the SAME file AFTER the
        // export's source snapshot has been frozen but BEFORE any `copy_*` reads. The
        // bundle must reflect only the pre-snapshot state. If the `src_tx` snapshot in
        // `write_bundle` is removed, each `copy_*` SELECT reads the live file and the
        // hook's `n2` row bleeds into the bundle — so this assertion fails, which is
        // exactly the regression guard the snapshot fix needs.
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("src.db");
        let idx = IndexManager::new();
        {
            let mut w = Store::open(&src_path).unwrap();
            w.apply_mutation_crdt(&insert("notes", "n1", json!({"title": "Alpha"}), 1), &idx)
                .unwrap();
        }
        let exporter = Store::open(&src_path).unwrap();

        // The hook fires after the snapshot is frozen: a SECOND handle commits n2.
        let hook_path = src_path.clone();
        set_post_snapshot_test_hook(move || {
            let mut concurrent = Store::open(&hook_path).unwrap();
            let hook_idx = IndexManager::new();
            concurrent
                .apply_mutation_crdt(
                    &insert("notes", "n2", json!({"title": "Beta"}), 2),
                    &hook_idx,
                )
                .unwrap();
        });

        let bundle_path = dir.path().join("ws.forgews");
        exporter
            .export_workspace(&bundle_path, &ExportOptions::new("ws"))
            .unwrap();

        // The bundle reflects the frozen snapshot only. n1 and n2 share the same
        // `collection/notes` doc, so the n2 write appears as a SECOND chunk on that
        // doc: at snapshot time there is exactly ONE chunk; a leaked mid-export write
        // would make it two.
        let bundle = Store::open(&bundle_path).unwrap();
        let chunks_on_notes: i64 = bundle
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM crdt_chunks WHERE doc_id = 'collection/notes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chunks_on_notes, 1,
            "the bundle carries only the pre-snapshot chunk; the concurrent write's chunk must not leak in"
        );

        // And the imported projection (re-derived from the bundle's chunks) has only
        // n1 — a split-brain bundle would resurrect n2 here.
        let restore_path = dir.path().join("restored.db");
        let restored = Store::import_workspace(&bundle_path, &restore_path, &idx).unwrap();
        assert!(
            restored.get_record("notes", "n1").unwrap().is_some(),
            "n1 round-trips"
        );
        assert!(
            restored.get_record("notes", "n2").unwrap().is_none(),
            "n2 must be absent"
        );
    }

    // --- fixtures/export descriptors (T017) ------------------------------

    /// The export fixture descriptors pin the expected bundle shape: format
    /// version, included/excluded tables, run-log policy, deterministic ordering,
    /// and the `missing_required_for_ga` sections. We parse them and assert our
    /// implementation's constants + policy agree with the canonical descriptors.
    #[derive(serde::Deserialize)]
    struct TinyDescriptor {
        export_format_version: i64,
        include_run_logs: bool,
        missing_required_for_ga: Vec<String>,
    }

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/export")
            .join(name)
    }

    fn load_fixture<T: serde::de::DeserializeOwned>(name: &str) -> T {
        let text = std::fs::read_to_string(fixture_path(name))
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"))
    }

    #[test]
    fn tiny_fixture_agrees_with_our_format_version_and_policy() {
        let fx: TinyDescriptor = load_fixture("tiny_workspace_descriptor.json");
        assert_eq!(
            fx.export_format_version, EXPORT_FORMAT_VERSION,
            "fixture pins our format version"
        );
        // Default policy excludes run logs, matching the tiny fixture.
        assert!(!fx.include_run_logs);
        assert_eq!(ExportOptions::default().run_logs, RunLogPolicy::Exclude);
        // The GA-missing sections are not yet persisted tables in M0a storage, so
        // they are legitimately absent from the bundle (documented in the spec).
        assert!(fx
            .missing_required_for_ga
            .contains(&"rbac_config".to_string()));
        assert!(fx
            .missing_required_for_ga
            .contains(&"index_defs".to_string()));
    }

    #[derive(serde::Deserialize)]
    struct RunLogsDescriptor {
        include_run_logs: bool,
    }

    #[test]
    fn run_logs_fixture_describes_the_include_policy() {
        let fx: RunLogsDescriptor = load_fixture("workspace_with_run_logs_descriptor.json");
        assert!(
            fx.include_run_logs,
            "the debug-bundle fixture opts into run logs"
        );
        // Our include policy maps to the same flag.
        assert_eq!(
            ExportOptions::new("ws").with_run_logs().run_logs,
            RunLogPolicy::Include
        );
    }

    #[derive(serde::Deserialize)]
    struct RedactedDescriptor {
        expected_exclusions: Vec<String>,
    }

    #[test]
    fn redacted_fixture_exclusions_match_our_guard() {
        let fx: RedactedDescriptor = load_fixture("redacted_secrets_descriptor.json");
        // The fixture's excluded kinds (secret plaintext, provider tokens, local
        // window state) all map to namespaces our guard refuses to export.
        assert!(fx
            .expected_exclusions
            .iter()
            .any(|e| e == "secret_plaintext"));
        assert!(is_local_only_namespace("secret/anything"));
        assert!(is_local_only_namespace("provider/anything"));
        assert!(is_local_only_namespace("local/window"));
    }
}
