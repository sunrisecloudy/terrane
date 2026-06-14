    use super::*;
    use forge_domain::{
        AppResult, AppletId, CollectionId, CoreError, LogicalTimestamp, RecordEnvelope, RecordId,
        RecordedCall, Result, RunId, RunOutcome, RunRecord,
    };
    use rusqlite::params;
    use std::collections::BTreeMap;

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    fn fields(pairs: &[(&str, serde_json::Value)]) -> BTreeMap<String, serde_json::Value> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }

    fn sample_record(collection: &str, id: &str, title: &str) -> RecordEnvelope {
        RecordEnvelope::new(
            CollectionId::new(collection),
            RecordId::new(id),
            fields(&[("title", serde_json::json!(title))]),
            LogicalTimestamp(1),
        )
    }

    // Model lifted from domain::run tests so the roundtrip exercises a fully
    // populated record (calls, logs, completed outcome).
    fn sample_run(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: RunId::new(run_id),
            applet_id: AppletId::new("app_notes"),
            // Canonical sha256: of a stand-in body, so the sample is a
            // contract-valid record (its code_hash passes validate_code_hash,
            // which save_run/load_run now enforce — review 013/014).
            code_hash: forge_domain::code_hash("sample-body"),
            input: serde_json::json!({"name": "world"}),
            random_seed: 42,
            time_start: 1000,
            calls: vec![
                RecordedCall {
                    seq: 0,
                    method: "time.now".into(),
                    args: serde_json::json!(null),
                    response: serde_json::json!(1000),
                },
                RecordedCall {
                    seq: 1,
                    method: "storage.set".into(),
                    args: serde_json::json!(["name", "world"]),
                    response: serde_json::json!(null),
                },
            ],
            logs: vec!["hello".into()],
            permissions: forge_domain::PermissionSnapshot::default(),
            outcome: RunOutcome::Completed {
                result: AppResult { ok: true, value: serde_json::json!("Hello world") },
            },
        }
    }

    // --- open / schema ---------------------------------------------------

    #[test]
    fn open_in_memory_sets_wal_and_pragmas() {
        let s = store();
        let fk: i64 = s
            .conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1, "foreign_keys must be ON");
        // synchronous=NORMAL is integer 1.
        let sync: i64 = s
            .conn
            .query_row("PRAGMA synchronous", [], |r| r.get(0))
            .unwrap();
        assert_eq!(sync, 1, "synchronous must be NORMAL(=1)");
    }

    #[test]
    fn open_is_idempotent_on_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        {
            let mut s = Store::open(&path).unwrap();
            s.kv_set("ns", "k", b"v", "text/plain").unwrap();
        }
        // Re-opening the same file must not error on CREATE TABLE IF NOT EXISTS.
        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.kv_get("ns", "k").unwrap().as_deref(), Some(&b"v"[..]));
    }

    // --- KV --------------------------------------------------------------

    #[test]
    fn kv_roundtrip_and_overwrite() {
        let mut s = store();
        assert_eq!(s.kv_get("app", "k1").unwrap(), None);
        s.kv_set("app", "k1", b"hello", "text/plain").unwrap();
        assert_eq!(s.kv_get("app", "k1").unwrap().as_deref(), Some(&b"hello"[..]));
        s.kv_set("app", "k1", b"world", "text/plain").unwrap();
        assert_eq!(s.kv_get("app", "k1").unwrap().as_deref(), Some(&b"world"[..]));
        // logical_version bumps on overwrite.
        let ver: i64 = s
            .conn
            .query_row(
                "SELECT logical_version FROM kv WHERE namespace='app' AND key='k1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(ver, 2);
    }

    #[test]
    fn kv_namespaces_are_isolated() {
        let mut s = store();
        s.kv_set("a", "k", b"1", "text/plain").unwrap();
        s.kv_set("b", "k", b"2", "text/plain").unwrap();
        assert_eq!(s.kv_get("a", "k").unwrap().as_deref(), Some(&b"1"[..]));
        assert_eq!(s.kv_get("b", "k").unwrap().as_deref(), Some(&b"2"[..]));
    }

    #[test]
    fn kv_list_prefix_sorted_and_filtered() {
        let mut s = store();
        s.kv_set("ns", "app/b", b"1", "text/plain").unwrap();
        s.kv_set("ns", "app/a", b"1", "text/plain").unwrap();
        s.kv_set("ns", "other/x", b"1", "text/plain").unwrap();
        s.kv_set("other_ns", "app/z", b"1", "text/plain").unwrap();
        let keys = s.kv_list("ns", "app/").unwrap();
        assert_eq!(keys, vec!["app/a".to_string(), "app/b".to_string()]);
        // Empty prefix lists everything in the namespace.
        let all = s.kv_list("ns", "").unwrap();
        assert_eq!(all, vec!["app/a", "app/b", "other/x"]);
    }

    #[test]
    fn kv_list_escapes_like_metacharacters() {
        let mut s = store();
        s.kv_set("ns", "a%b", b"1", "text/plain").unwrap();
        s.kv_set("ns", "axb", b"1", "text/plain").unwrap();
        // Prefix "a%" must match only the literal "a%b", not "axb".
        let keys = s.kv_list("ns", "a%").unwrap();
        assert_eq!(keys, vec!["a%b".to_string()]);
    }

    #[test]
    fn kv_delete_tombstones_and_hides_from_get_and_list() {
        let mut s = store();
        s.kv_set("ns", "k", b"v", "text/plain").unwrap();
        s.kv_delete("ns", "k").unwrap();
        assert_eq!(s.kv_get("ns", "k").unwrap(), None);
        assert!(s.kv_list("ns", "").unwrap().is_empty());
        // Row still exists, tombstone=1.
        let tomb: i64 = s
            .conn
            .query_row(
                "SELECT tombstone FROM kv WHERE namespace='ns' AND key='k'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tomb, 1);
        // Re-setting clears the tombstone.
        s.kv_set("ns", "k", b"v2", "text/plain").unwrap();
        assert_eq!(s.kv_get("ns", "k").unwrap().as_deref(), Some(&b"v2"[..]));
        assert_eq!(s.kv_list("ns", "").unwrap(), vec!["k".to_string()]);
    }

    #[test]
    fn kv_delete_of_missing_key_is_noop() {
        let mut s = store();
        // Should not error even though nothing matches.
        s.kv_delete("ns", "nope").unwrap();
        assert_eq!(s.kv_get("ns", "nope").unwrap(), None);
    }

    /// Review 036 finding 3: `next_counter` reserves a strictly increasing,
    /// never-repeating value, starting at 1, and the reserved value is durably
    /// persisted (so a re-open continues monotonically). This is the atomic
    /// primitive the core uses to mint a unique per-execution `run_id`.
    #[test]
    fn next_counter_is_monotone_unique_and_persisted() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        let mut seen = std::collections::BTreeSet::new();
        {
            let mut s = Store::open(&path).unwrap();
            for expected in 1..=5u64 {
                let n = s.next_counter("__forge/meta", "run_counter").unwrap();
                assert_eq!(n, expected, "counter must increment by one from 1");
                assert!(seen.insert(n), "every reservation must be unique: {n}");
            }
        }
        // Re-open the SAME file: the counter is durable, so the next reservation
        // continues past the last persisted value rather than restarting.
        {
            let mut s2 = Store::open(&path).unwrap();
            let n = s2.next_counter("__forge/meta", "run_counter").unwrap();
            assert_eq!(n, 6, "re-open must continue the persisted counter");
            assert!(seen.insert(n));
        }
    }

    /// A corrupted (non-integer) persisted counter is a `StorageError`, not a
    /// silent reset to zero (which would re-issue already-used reservations).
    #[test]
    fn next_counter_rejects_a_corrupted_value() {
        let mut s = store();
        s.kv_set("__forge/meta", "run_counter", b"not-a-number", "text/plain").unwrap();
        let err = s.next_counter("__forge/meta", "run_counter").unwrap_err();
        assert_eq!(err.code(), "StorageError", "{err}");
    }

    /// Review 038 finding 3: two *separate* file-backed `Store` handles (the
    /// two-`WorkspaceCore` race) bumping the SAME counter concurrently must each
    /// receive a DISTINCT value — never a collision and never a spurious
    /// `database is locked`. With the old `DEFERRED` SELECT-then-upsert both
    /// connections could read the same snapshot; `BEGIN IMMEDIATE` + busy-timeout
    /// + retry serializes them so every reservation is unique and contiguous.
    #[test]
    fn next_counter_two_file_handles_never_collide_or_lock() {
        use std::collections::BTreeSet;
        use std::sync::{Arc, Barrier};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        // Create the file/schema once up front so both threads open an existing DB.
        Store::open(&path).unwrap();

        const PER_THREAD: u64 = 50;
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                // A genuinely independent handle (its own SQLite connection),
                // exactly the two-`WorkspaceCore::open()` scenario.
                let mut s = Store::open(&path).unwrap();
                barrier.wait(); // maximize the contention window
                let mut got = Vec::with_capacity(PER_THREAD as usize);
                for _ in 0..PER_THREAD {
                    got.push(
                        s.next_counter("__forge/meta", "run_counter")
                            .expect("a contended reservation must not surface a lock error"),
                    );
                }
                got
            }));
        }

        let mut all = Vec::new();
        for h in handles {
            all.extend(h.join().expect("counter thread panicked"));
        }

        // Every reservation across BOTH handles is unique (no collision → no
        // silently-overwritten run record).
        let unique: BTreeSet<u64> = all.iter().copied().collect();
        assert_eq!(
            unique.len(),
            all.len(),
            "two file handles must never reserve the same counter value"
        );
        // The set is exactly 1..=(2*PER_THREAD): no gaps and no duplicates, so the
        // two interleavings were serialized, not lost.
        let total = 2 * PER_THREAD;
        let expected: BTreeSet<u64> = (1..=total).collect();
        assert_eq!(unique, expected, "reservations must be contiguous 1..=2N");
    }

    // --- Records projection ----------------------------------------------

    #[test]
    fn record_put_get_roundtrips_full_envelope() {
        let s = store();
        let mut rec = sample_record("tasks", "rec_1", "Ship");
        rec.unknown_fields.insert("f_future".into(), serde_json::json!({"x": 1}));
        s.put_record(&rec).unwrap();
        let back = s.get_record("tasks", "rec_1").unwrap().unwrap();
        assert_eq!(back, rec, "stored envelope must reconstruct identically");
        assert_eq!(back.unknown_fields["f_future"], serde_json::json!({"x": 1}));
    }

    #[test]
    fn record_get_missing_is_none() {
        let s = store();
        assert_eq!(s.get_record("tasks", "nope").unwrap(), None);
    }

    #[test]
    fn record_put_overwrites_projection() {
        let s = store();
        s.put_record(&sample_record("tasks", "rec_1", "old")).unwrap();
        s.put_record(&sample_record("tasks", "rec_1", "new")).unwrap();
        let back = s.get_record("tasks", "rec_1").unwrap().unwrap();
        assert_eq!(back.fields["title"], serde_json::json!("new"));
        // Still exactly one row for that PK.
        let n: i64 = s
            .conn
            .query_row(
                "SELECT COUNT(*) FROM records WHERE collection='tasks' AND id='rec_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn record_data_is_valid_json_for_json1() {
        let s = store();
        s.put_record(&sample_record("tasks", "rec_1", "Ship MVP")).unwrap();
        // Exercise the JSON1 path the projection promises (DL-4/DL-5).
        let title: String = s
            .conn
            .query_row(
                "SELECT json_extract(data, '$.fields.title') FROM records
                  WHERE collection='tasks' AND id='rec_1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(title, "Ship MVP");
    }

    #[test]
    fn list_records_orders_by_id_and_scopes_collection() {
        let s = store();
        s.put_record(&sample_record("tasks", "b", "B")).unwrap();
        s.put_record(&sample_record("tasks", "a", "A")).unwrap();
        s.put_record(&sample_record("notes", "z", "Z")).unwrap();
        let recs = s.list_records("tasks").unwrap();
        let ids: Vec<_> = recs.iter().map(|r| r.entity_id.as_str().to_string()).collect();
        assert_eq!(ids, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(s.list_records("notes").unwrap().len(), 1);
        assert!(s.list_records("empty").unwrap().is_empty());
    }

    // --- Oplog -----------------------------------------------------------

    #[test]
    fn oplog_append_and_read_in_order() {
        let s = store();
        s.append_op("op2", "actor", "ws", 2, "insert", b"p2").unwrap();
        s.append_op("op1", "actor", "ws", 1, "insert", b"p1").unwrap();
        let ops = s.list_ops().unwrap();
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].op_id, "op1");
        assert_eq!(ops[0].lamport, 1);
        assert_eq!(ops[0].payload, b"p1");
        assert_eq!(ops[1].op_id, "op2");
        assert_eq!(ops[1].lamport, 2);
    }

    #[test]
    fn oplog_duplicate_op_id_is_storage_error() {
        let s = store();
        s.append_op("op1", "a", "ws", 1, "k", b"x").unwrap();
        let err = s.append_op("op1", "a", "ws", 1, "k", b"y").unwrap_err();
        assert_eq!(err.code(), "StorageError");
    }

    // --- CRDT blobs ------------------------------------------------------

    #[test]
    fn chunks_store_and_read() {
        let s = store();
        s.put_chunk("doc1", "c1", "loro", b"aaa").unwrap();
        s.put_chunk("doc1", "c2", "loro", b"bbb").unwrap();
        s.put_chunk("doc2", "c1", "loro", b"zzz").unwrap();
        let chunks = s.get_chunks("doc1").unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].chunk_id, "c1");
        assert_eq!(chunks[0].payload, b"aaa");
        assert_eq!(chunks[1].payload, b"bbb");
        assert_eq!(s.get_chunks("doc2").unwrap().len(), 1);
        assert!(s.get_chunks("missing").unwrap().is_empty());
    }

    #[test]
    fn list_doc_ids_returns_distinct_sorted_docs() {
        // The sync seam's per-doc frontier walk (SS-1/SS-2) needs the union of
        // doc ids that hold chunks: distinct, sorted, and empty when there are
        // none.
        let s = store();
        assert!(s.list_doc_ids().unwrap().is_empty());
        s.put_chunk("collection/notes", "c1", "loro", b"n").unwrap();
        s.put_chunk("collection/tasks", "c1", "loro", b"a").unwrap();
        s.put_chunk("collection/tasks", "c2", "loro", b"b").unwrap();
        assert_eq!(
            s.list_doc_ids().unwrap(),
            vec!["collection/notes".to_string(), "collection/tasks".to_string()],
            "doc ids are distinct and sorted regardless of chunk count"
        );
    }

    #[test]
    fn put_chunk_is_append_only_immutable() {
        // Append-only history (review 003): an identical re-write is an
        // idempotent no-op, but a conflicting re-write of the same chunk id is
        // refused with StorageError instead of silently rewriting history.
        let s = store();
        s.put_chunk("doc1", "c1", "loro", b"aaa").unwrap();

        // Idempotent: same content re-written is fine and does not duplicate.
        s.put_chunk("doc1", "c1", "loro", b"aaa").unwrap();
        assert_eq!(s.get_chunks("doc1").unwrap().len(), 1);

        // Conflicting payload for an existing chunk id -> StorageError.
        let err = s.put_chunk("doc1", "c1", "loro", b"DIFFERENT").unwrap_err();
        assert_eq!(err.code(), "StorageError");

        // History is unchanged: the original chunk still has its payload.
        let got = s.get_chunk("doc1", "c1").unwrap().unwrap();
        assert_eq!(got.payload, b"aaa");
    }

    #[test]
    fn snapshots_store_and_latest_wins() {
        let s = store();
        assert_eq!(s.latest_snapshot("doc1").unwrap(), None);
        s.put_snapshot("doc1", "s1", "loro", b"snap1", b"f1").unwrap();
        s.put_snapshot("doc1", "s2", "loro", b"snap2", b"f2").unwrap();
        let latest = s.latest_snapshot("doc1").unwrap().unwrap();
        // s2 inserted after s1 -> later created_at / higher id wins.
        assert_eq!(latest.snapshot_id, "s2");
        assert_eq!(latest.payload, b"snap2");
        assert_eq!(latest.frontier, b"f2");
    }

    #[test]
    fn snapshot_upsert_replaces_payload() {
        let s = store();
        s.put_snapshot("doc1", "s1", "loro", b"v1", b"f1").unwrap();
        s.put_snapshot("doc1", "s1", "loro", b"v2", b"f2").unwrap();
        let latest = s.latest_snapshot("doc1").unwrap().unwrap();
        assert_eq!(latest.payload, b"v2");
        assert_eq!(latest.frontier, b"f2");
    }

    // --- Runs ------------------------------------------------------------

    #[test]
    fn save_run_load_run_roundtrips() {
        let s = store();
        let run = sample_run("run_1");
        s.save_run(&run).unwrap();
        let back = s.load_run("run_1").unwrap().unwrap();
        assert_eq!(back, run, "loaded run must equal the original (replay source)");
        // And the replay fingerprint matches, proving the trace survived.
        assert!(run.replays_identically(&back));
    }

    #[test]
    fn load_run_missing_is_none() {
        let s = store();
        assert_eq!(s.load_run("nope").unwrap(), None);
    }

    #[test]
    fn save_run_rejects_noncanonical_code_hash() {
        // Provenance contract (review 013/014): a record carrying a divergent
        // code_hash (the runtime's old fnv1a64: form) must be refused on save
        // rather than persisting a row the pipeline can never reproduce.
        let s = store();
        let mut run = sample_run("run_bad");
        run.code_hash = "fnv1a64:0123456789abcdef".into();
        let err = s.save_run(&run).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        // Nothing was persisted: the rejected record never reached the table.
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "rejected record must not land in the runs table");
    }

    #[test]
    fn load_run_rejects_corrupted_legacy_code_hash() {
        // A legacy/corrupted row (written before this guard, or mangled in the
        // file) must surface a ValidationError on read, not a silent bad record
        // (review 013/014). Inject the bad row directly, bypassing save_run's
        // guard, to model the on-disk legacy/corruption case.
        let s = store();
        let mut run = sample_run("run_legacy");
        run.code_hash = "fnv1a64:deadbeef".into();
        let json = serde_json::to_string(&run).unwrap();
        s.conn
            .execute(
                "INSERT INTO runs (run_id, applet_id, record_json, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params!["run_legacy", "app_notes", json, 0i64],
            )
            .unwrap();
        let err = s.load_run("run_legacy").unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn save_run_load_run_validates_canonical_roundtrip() {
        // A round-tripped canonical record loads and re-validates cleanly.
        let s = store();
        let run = sample_run("run_ok");
        assert!(run.validate_code_hash().is_ok(), "sample must be canonical");
        s.save_run(&run).unwrap();
        let back = s.load_run("run_ok").unwrap().unwrap();
        assert_eq!(back, run);
        assert!(back.validate_code_hash().is_ok());
    }

    #[test]
    fn save_run_overwrites_same_id() {
        let s = store();
        let mut run = sample_run("run_1");
        s.save_run(&run).unwrap();
        run.random_seed = 99;
        s.save_run(&run).unwrap();
        let back = s.load_run("run_1").unwrap().unwrap();
        assert_eq!(back.random_seed, 99);
        let n: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }

    // --- transact (DL-4) -------------------------------------------------

    #[test]
    fn transact_commits_on_ok() {
        let mut s = store();
        s.transact(|tx| {
            tx.execute(
                "INSERT INTO kv (namespace, key, value, content_type, logical_version, updated_at, tombstone)
                 VALUES ('ns','k', x'01', 'text/plain', 1, 0, 0)",
                [],
            )
            .map_err(map_sql)?;
            Ok(())
        })
        .unwrap();
        assert_eq!(s.kv_get("ns", "k").unwrap().as_deref(), Some(&[1u8][..]));
    }

    #[test]
    fn transact_rolls_back_on_err_leaving_db_unchanged() {
        let mut s = store();
        s.kv_set("ns", "pre", b"x", "text/plain").unwrap();
        let before: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0))
            .unwrap();

        let result: Result<()> = s.transact(|tx| {
            tx.execute(
                "INSERT INTO kv (namespace, key, value, content_type, logical_version, updated_at, tombstone)
                 VALUES ('ns','mid', x'02', 'text/plain', 1, 0, 0)",
                [],
            )
            .map_err(map_sql)?;
            // Now bail; the mid insert must be rolled back.
            Err(CoreError::ValidationError("boom".into()))
        });
        assert_eq!(result.unwrap_err().code(), "ValidationError");

        let after: i64 = s
            .conn
            .query_row("SELECT COUNT(*) FROM kv", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, after, "rolled-back insert must not persist");
        assert_eq!(s.kv_get("ns", "mid").unwrap(), None);
        // Pre-existing data untouched.
        assert_eq!(s.kv_get("ns", "pre").unwrap().as_deref(), Some(&b"x"[..]));
    }

    #[test]
    fn transact_returns_closure_value() {
        let mut s = store();
        let n = s.transact(|_tx| Ok(7usize)).unwrap();
        assert_eq!(n, 7);
    }

    // --- durability / persistence ----------------------------------------

    #[test]
    fn file_backed_db_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        {
            let mut s = Store::open(&path).unwrap();
            s.kv_set("ns", "k", b"durable", "text/plain").unwrap();
            s.put_record(&sample_record("tasks", "rec_1", "kept")).unwrap();
            s.append_op("op1", "a", "ws", 1, "insert", b"p").unwrap();
            s.put_snapshot("doc1", "s1", "loro", b"snap", b"fr").unwrap();
            s.save_run(&sample_run("run_1")).unwrap();
        } // Store (and Connection) dropped — WAL flushed on close.

        let s2 = Store::open(&path).unwrap();
        assert_eq!(s2.kv_get("ns", "k").unwrap().as_deref(), Some(&b"durable"[..]));
        assert_eq!(
            s2.get_record("tasks", "rec_1").unwrap().unwrap().fields["title"],
            serde_json::json!("kept")
        );
        assert_eq!(s2.list_ops().unwrap().len(), 1);
        assert_eq!(s2.latest_snapshot("doc1").unwrap().unwrap().payload, b"snap");
        assert_eq!(s2.load_run("run_1").unwrap().unwrap().run_id, RunId::new("run_1"));
    }

    #[test]
    fn file_backed_db_uses_wal_journal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        let s = Store::open(&path).unwrap();
        let mode: String = s
            .conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal", "DL-23 requires WAL on disk");
    }

    // --- Query engine (DL-15) --------------------------------------------

    fn seed_tasks(s: &Store) {
        // Three tasks with display fields; prio numeric, status/title text.
        let mut a = sample_record("tasks", "tasks/1", "alpha");
        a.fields.insert("status".into(), serde_json::json!("todo"));
        a.fields.insert("prio".into(), serde_json::json!(1));
        let mut b = sample_record("tasks", "tasks/2", "beta");
        b.fields.insert("status".into(), serde_json::json!("done"));
        b.fields.insert("prio".into(), serde_json::json!(2));
        let mut c = sample_record("tasks", "tasks/3", "gamma");
        c.fields.insert("status".into(), serde_json::json!("todo"));
        c.fields.insert("prio".into(), serde_json::json!(3));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();
        s.put_record(&c).unwrap();
    }

    fn plan(v: serde_json::Value) -> Query {
        Query::from_fixture_value(&v).expect("parse plan")
    }

    #[test]
    fn query_eq_filters_and_orders() {
        let s = store();
        seed_tasks(&s);
        let q = plan(serde_json::json!({
            "from": "tasks", "where": ["status", "=", "todo"], "orderBy": ["prio", "asc"]
        }));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1", "tasks/3"]);
    }

    #[test]
    fn query_ne_excludes_match_and_treats_missing_as_differing() {
        let s = store();
        seed_tasks(&s);
        // A record without `status` at all: ne('done') should still include it
        // (missing differs from 'done').
        let mut d = sample_record("tasks", "tasks/4", "delta");
        d.fields.remove("status"); // sample_record only sets title anyway
        s.put_record(&d).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "where": ["status", "!=", "done"]}));
        let ids = s.query(&q).unwrap().ids();
        assert!(ids.contains(&"tasks/1".to_string()));
        assert!(ids.contains(&"tasks/3".to_string()));
        assert!(ids.contains(&"tasks/4".to_string()), "missing field differs from a value");
        assert!(!ids.contains(&"tasks/2".to_string()), "status=done excluded");
    }

    #[test]
    fn query_range_excludes_missing_and_non_numeric() {
        let s = store();
        seed_tasks(&s);
        // tasks/5 has a non-numeric prio; a range filter must not coerce it in.
        let mut e = sample_record("tasks", "tasks/5", "eps");
        e.fields.insert("prio".into(), serde_json::json!("99"));
        s.put_record(&e).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "where": ["prio", ">=", 2]}));
        let ids = s.query(&q).unwrap().ids();
        assert_eq!(ids, vec!["tasks/2", "tasks/3"], "string prio not coerced into range");
    }

    #[test]
    fn query_in_and_like() {
        let s = store();
        seed_tasks(&s);
        let q_in = plan(serde_json::json!({"from": "tasks", "where": ["status", "in", ["todo"]]}));
        assert_eq!(s.query(&q_in).unwrap().ids(), vec!["tasks/1", "tasks/3"]);

        let q_like = plan(serde_json::json!({"from": "tasks", "where": ["title", "like", "%a"]}));
        // alpha, beta, gamma all end in 'a'.
        assert_eq!(s.query(&q_like).unwrap().ids(), vec!["tasks/1", "tasks/2", "tasks/3"]);
    }

    #[test]
    fn query_like_escapes_metacharacters() {
        let s = store();
        let mut a = sample_record("notes", "n1", "a%b");
        a.fields.insert("title".into(), serde_json::json!("a%b"));
        let mut b = sample_record("notes", "n2", "axb");
        b.fields.insert("title".into(), serde_json::json!("axb"));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();
        // Pattern 'a\%b' (escaped) must match only the literal 'a%b'.
        let q = plan(serde_json::json!({"from": "notes", "where": ["title", "like", "a\\%b"]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["n1"]);
    }

    #[test]
    fn query_like_is_ascii_case_insensitive() {
        let s = store();
        let mut a = sample_record("notes", "n1", "Plan");
        a.fields.insert("title".into(), serde_json::json!("Plan"));
        s.put_record(&a).unwrap();
        let q = plan(serde_json::json!({"from": "notes", "where": ["title", "like", "plan"]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["n1"], "LIKE folds ASCII case");
    }

    #[test]
    fn query_eq_null_matches_missing_field() {
        let s = store();
        let mut a = sample_record("tasks", "tasks/1", "a"); // no `assignee`
        a.fields.remove("assignee");
        let mut b = sample_record("tasks", "tasks/2", "b");
        b.fields.insert("assignee".into(), serde_json::json!("ada"));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "where": ["assignee", "=", null]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1"], "missing reads as null for eq(null)");
        let q2 = plan(serde_json::json!({"from": "tasks", "where": ["assignee", "!=", null]}));
        assert_eq!(s.query(&q2).unwrap().ids(), vec!["tasks/2"]);
    }

    #[test]
    fn query_hides_tombstoned_unless_include_deleted() {
        let s = store();
        seed_tasks(&s);
        s.delete_record("tasks", "tasks/2", Some(9)).unwrap();
        let q = plan(serde_json::json!({"from": "tasks", "orderBy": ["prio", "asc"]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1", "tasks/3"]);
        let q_all = plan(serde_json::json!({
            "from": "tasks", "orderBy": ["prio", "asc"], "includeDeleted": true
        }));
        assert_eq!(s.query(&q_all).unwrap().ids(), vec!["tasks/1", "tasks/2", "tasks/3"]);
    }

    #[test]
    fn query_count_and_group_by() {
        let s = store();
        seed_tasks(&s);
        let q = plan(serde_json::json!({
            "from": "tasks", "where": ["status", "=", "todo"], "aggregate": {"op": "count"}
        }));
        match s.query(&q).unwrap() {
            QueryResult::Aggregate(a) => assert_eq!(a.count, Some(2)),
            other => panic!("expected aggregate, got {other:?}"),
        }
        let qg = plan(serde_json::json!({
            "from": "tasks", "groupBy": "status", "aggregate": {"sum": "prio"}
        }));
        match s.query(&qg).unwrap() {
            QueryResult::Groups(g) => {
                assert_eq!(g.len(), 2);
                // Keys ascending: done, todo.
                assert_eq!(g[0].key, serde_json::json!("done"));
                assert_eq!(g[0].aggregate.sum, Some(2.0));
                assert_eq!(g[1].key, serde_json::json!("todo"));
                assert_eq!(g[1].aggregate.sum, Some(4.0)); // 1 + 3
            }
            other => panic!("expected groups, got {other:?}"),
        }
    }

    /// Review 040 finding 3: `eq`/`ne`/`in` over a JSON boolean must not coerce
    /// to numbers. On one collection where the same field holds a boolean on some
    /// records and a numeric 0/1 on others, `done.eq(false)` matches ONLY the
    /// boolean-false record (not the stored numeric 0), and `eq(true)` matches
    /// only boolean-true (not numeric 1). `json_extract` renders both as SQL 0/1,
    /// so without the json_type guard this silently over-matches.
    #[test]
    fn query_boolean_eq_does_not_coerce_numeric_zero_or_one() {
        let s = store();
        let mut bf = sample_record("flags", "bool_false", "bf");
        bf.fields.insert("done".into(), serde_json::json!(false));
        let mut bt = sample_record("flags", "bool_true", "bt");
        bt.fields.insert("done".into(), serde_json::json!(true));
        let mut n0 = sample_record("flags", "num_0", "n0");
        n0.fields.insert("done".into(), serde_json::json!(0));
        let mut n1 = sample_record("flags", "num_1", "n1");
        n1.fields.insert("done".into(), serde_json::json!(1));
        for r in [&bf, &bt, &n0, &n1] {
            s.put_record(r).unwrap();
        }

        // eq(false) -> only the boolean false, NOT numeric 0.
        let q = plan(serde_json::json!({"from": "flags", "where": ["done", "=", false]}));
        assert_eq!(s.query(&q).unwrap().ids(), vec!["bool_false"], "eq(false) must not match numeric 0");
        // eq(true) -> only the boolean true, NOT numeric 1.
        let qt = plan(serde_json::json!({"from": "flags", "where": ["done", "=", true]}));
        assert_eq!(s.query(&qt).unwrap().ids(), vec!["bool_true"], "eq(true) must not match numeric 1");
    }

    #[test]
    fn query_boolean_ne_and_in_do_not_coerce_numbers() {
        let s = store();
        let mut bf = sample_record("flags", "bool_false", "bf");
        bf.fields.insert("done".into(), serde_json::json!(false));
        let mut bt = sample_record("flags", "bool_true", "bt");
        bt.fields.insert("done".into(), serde_json::json!(true));
        let mut n0 = sample_record("flags", "num_0", "n0");
        n0.fields.insert("done".into(), serde_json::json!(0));
        for r in [&bf, &bt, &n0] {
            s.put_record(r).unwrap();
        }

        // ne(false): boolean-true and numeric-0 both DIFFER from boolean false;
        // only boolean-false is excluded.
        let qne = plan(serde_json::json!({"from": "flags", "where": ["done", "!=", false]}));
        assert_eq!(
            s.query(&qne).unwrap().ids(),
            vec!["bool_true", "num_0"],
            "ne(false) must treat numeric 0 as differing"
        );
        // in [false]: only the boolean false, NOT numeric 0.
        let qin = plan(serde_json::json!({"from": "flags", "where": ["done", "in", [false]]}));
        assert_eq!(s.query(&qin).unwrap().ids(), vec!["bool_false"], "in[false] must not match numeric 0");
    }

    /// Review 041 finding 5: a stable field id containing a `.` (e.g. `f_dev.01_0`,
    /// mintable from an actor id `dev.01`) must address the literal JSON key, not
    /// a nested json1 path. Filter, index DDL, and FTS all read the same quoted
    /// path, so a query over a dotted id returns the right rows and an index over
    /// it is actually consulted.
    #[test]
    fn query_with_dotted_field_id_addresses_literal_key() {
        let s = store();
        let mut a = sample_record("tasks", "t1", "a");
        a.field_ids.insert("f_dev.01_0".into(), serde_json::json!("open"));
        let mut b = sample_record("tasks", "t2", "b");
        b.field_ids.insert("f_dev.01_0".into(), serde_json::json!("closed"));
        s.put_record(&a).unwrap();
        s.put_record(&b).unwrap();

        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_dev.01_0", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        // Without the quoted path this returns [] (json1 reads NULL for the dotted
        // path) instead of the matching row.
        assert_eq!(s.query(&q).unwrap().ids(), vec!["t1"], "dotted field id must read the literal key");
    }

    #[test]
    fn value_index_over_dotted_field_id_is_consulted() {
        let s = store();
        let mut a = sample_record("tasks", "t1", "a");
        a.field_ids.insert("f_dev.01_0".into(), serde_json::json!("open"));
        s.put_record(&a).unwrap();
        let mut mgr = IndexManager::new();
        s.create_index(&mut mgr, "tasks", "f_dev.01_0", CreateIndexKind::Value)
            .unwrap();
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_dev.01_0", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        let planned = s.query_planned(&q, &mgr).unwrap();
        assert!(planned.uses_index, "index over dotted field id must be usable");
        assert_eq!(planned.ids(), vec!["t1"], "indexed query over dotted id returns the row");
        // The expression index over the quoted path is the one SQLite consults.
        let plan: String = s
            .connection()
            .query_row(
                "EXPLAIN QUERY PLAN SELECT id FROM records \
                 WHERE collection = 'tasks' AND json_extract(data, '$.field_ids.\"f_dev.01_0\"') = 'open'",
                [],
                |r| r.get::<_, String>(3),
            )
            .unwrap();
        assert!(
            plan.contains("idx_records_tasks_f_dev.01_0"),
            "SQLite must consult the dotted-id expression index, got: {plan}"
        );
    }

    #[test]
    fn fts_over_dotted_field_id_matches() {
        let s = store();
        let mut env = RecordEnvelope::new(
            CollectionId::new("notes"),
            RecordId::new("n1"),
            fields(&[("body", serde_json::json!("offline rebuild keeps indexes honest"))]),
            LogicalTimestamp(1),
        );
        env.field_ids
            .insert("f_dev.01_0".into(), serde_json::json!("offline rebuild keeps indexes honest"));
        s.put_record(&env).unwrap();
        let mut mgr = IndexManager::new();
        s.create_index(&mut mgr, "notes", "f_dev.01_0", CreateIndexKind::Fts)
            .unwrap();
        // Populated from the literal dotted key, so the term is searchable.
        let hits = mgr.fts_match(s.connection(), "notes", "f_dev.01_0", "offline").unwrap();
        assert_eq!(hits, vec!["n1".to_string()], "FTS over a dotted field id must mirror the literal key");
    }

    // --- Mutations (DL-17) -----------------------------------------------

    #[test]
    fn insert_patch_delete_sequence_post_state() {
        let mut s = store();
        let indexes = IndexManager::new();
        // insert
        let ins = Mutation::Insert {
            collection: "tasks".into(),
            id: Some("task_001".into()),
            fields: serde_json::json!({"title": "Draft", "status": "draft", "prio": 1})
                .as_object()
                .unwrap()
                .clone(),
            logical_at: Some(1),
        };
        s.apply_mutation(&ins, &indexes).unwrap();
        let after_insert = s.get_record("tasks", "task_001").unwrap().unwrap();
        assert_eq!(after_insert.fields["status"], serde_json::json!("draft"));
        assert!(!after_insert.deleted);

        // patch merges (status+prio change, title preserved)
        let patched = s
            .patch_record(
                "tasks",
                "task_001",
                serde_json::json!({"status": "open", "prio": 2})
                    .as_object()
                    .unwrap(),
                Some(2),
            )
            .unwrap();
        assert_eq!(patched.fields["status"], serde_json::json!("open"));
        assert_eq!(patched.fields["prio"], serde_json::json!(2));
        assert_eq!(patched.fields["title"], serde_json::json!("Draft"), "omitted field preserved");
        assert_eq!(patched.updated_at, forge_domain::LogicalTimestamp(2));

        // delete tombstones, retains fields
        let deleted = s.delete_record("tasks", "task_001", Some(3)).unwrap();
        assert!(deleted.deleted);
        assert_eq!(deleted.fields["status"], serde_json::json!("open"));
        assert_eq!(deleted.updated_at, forge_domain::LogicalTimestamp(3));
        assert_eq!(deleted.created_at, forge_domain::LogicalTimestamp(1), "created_at preserved");

        // hidden from a normal query, retained in the table
        let q = Query::from("tasks");
        assert!(s.query(&q).unwrap().ids().is_empty());
        assert!(s.get_record("tasks", "task_001").unwrap().is_some());
    }

    #[test]
    fn update_replaces_known_fields_but_preserves_unknown() {
        let s = store();
        let mut rec = sample_record("tasks", "t1", "old");
        rec.fields.insert("status".into(), serde_json::json!("todo"));
        rec.unknown_fields.insert("f_future".into(), serde_json::json!({"x": 1}));
        s.put_record(&rec).unwrap();
        // update only sets title; status is dropped from display fields, but
        // unknown_fields must survive (DL-9).
        let updated = s
            .update_record(
                "tasks",
                "t1",
                serde_json::json!({"title": "new"}).as_object().unwrap(),
                Some(5),
            )
            .unwrap();
        assert_eq!(updated.fields.get("title"), Some(&serde_json::json!("new")));
        assert_eq!(updated.fields.get("status"), None, "update replaces display fields");
        assert_eq!(updated.unknown_fields["f_future"], serde_json::json!({"x": 1}));
    }

    #[test]
    fn patch_of_missing_record_is_query_error() {
        let s = store();
        let empty = serde_json::Map::new();
        let err = s.patch_record("tasks", "nope", &empty, None).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn transact_group_is_atomic_and_visible() {
        let mut s = store();
        let mut seed = sample_record("tasks", "tasks/1", "Existing");
        seed.fields.insert("done".into(), serde_json::json!(false));
        s.put_record(&seed).unwrap();

        let items = vec![
            Mutation::Insert {
                collection: "tasks".into(),
                id: Some("tasks/2".into()),
                fields: serde_json::json!({"title": "New", "done": false})
                    .as_object()
                    .unwrap()
                    .clone(),
                logical_at: None,
            },
            Mutation::Patch {
                collection: "tasks".into(),
                id: "tasks/1".into(),
                fields: serde_json::json!({"done": true}).as_object().unwrap().clone(),
                logical_at: None,
            },
        ];
        let n = s.transact_mutations(&items, &IndexManager::new()).unwrap();
        assert_eq!(n, 2);
        let q = Query::from("tasks");
        assert_eq!(s.query(&q).unwrap().ids(), vec!["tasks/1", "tasks/2"]);
        assert_eq!(
            s.get_record("tasks", "tasks/1").unwrap().unwrap().fields["done"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn transact_group_rolls_back_on_failure() {
        let mut s = store();
        // The group inserts tasks/2 then patches a missing record -> the whole
        // group must roll back, so tasks/2 is NOT visible afterward.
        let items = vec![
            Mutation::Insert {
                collection: "tasks".into(),
                id: Some("tasks/2".into()),
                fields: serde_json::Map::new(),
                logical_at: None,
            },
            Mutation::Patch {
                collection: "tasks".into(),
                id: "missing".into(),
                fields: serde_json::Map::new(),
                logical_at: None,
            },
        ];
        let err = s.transact_mutations(&items, &IndexManager::new()).unwrap_err();
        assert_eq!(err.code(), "QueryError");
        assert!(
            s.get_record("tasks", "tasks/2").unwrap().is_none(),
            "rolled-back insert must not persist"
        );
    }

    #[test]
    fn nested_transact_flattens_into_one_transaction() {
        let mut s = store();
        let items = vec![Mutation::Transact {
            items: vec![
                Mutation::Insert {
                    collection: "tasks".into(),
                    id: Some("a".into()),
                    fields: serde_json::Map::new(),
                    logical_at: None,
                },
                Mutation::Insert {
                    collection: "tasks".into(),
                    id: Some("b".into()),
                    fields: serde_json::Map::new(),
                    logical_at: None,
                },
            ],
        }];
        assert_eq!(s.transact_mutations(&items, &IndexManager::new()).unwrap(), 2);
        assert_eq!(s.list_records("tasks").unwrap().len(), 2);
    }

    // --- Index-synced record writes (DL-5 FTS maintenance) ---------------

    /// A note envelope whose stable `field_ids.f_alice_0` carries `body` (so an
    /// FTS index over that field id has text to mirror).
    fn note(id: &str, body: &str) -> RecordEnvelope {
        let mut env = RecordEnvelope::new(
            CollectionId::new("notes"),
            RecordId::new(id),
            fields(&[("body", serde_json::json!(body))]),
            LogicalTimestamp(1),
        );
        env.field_ids
            .insert("f_alice_0".into(), serde_json::json!(body));
        env
    }

    /// Create an Active FTS index on notes/body via the Store create API.
    fn notes_fts(store: &Store) -> IndexManager {
        let mut mgr = IndexManager::new();
        store
            .create_index(&mut mgr, "notes", "f_alice_0", CreateIndexKind::Fts)
            .expect("create fts index");
        mgr
    }

    #[test]
    fn put_record_indexed_keeps_fts_in_sync_on_insert_and_update() {
        let mut s = store();
        let mgr = notes_fts(&s);
        // Insert: searchable immediately.
        s.put_record_indexed(&note("n1", "offline rebuild keeps indexes honest"), &mgr)
            .unwrap();
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "offline").unwrap(),
            vec!["n1".to_string()]
        );
        // Overwrite the same id with new text: old term gone, new term present,
        // no duplicate rows.
        s.put_record_indexed(&note("n1", "lunch plans for the team"), &mgr)
            .unwrap();
        assert!(mgr
            .fts_match(s.connection(), "notes", "f_alice_0", "offline")
            .unwrap()
            .is_empty());
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "lunch").unwrap(),
            vec!["n1".to_string()]
        );
    }

    #[test]
    fn patch_record_indexed_runs_fts_sync_without_corrupting_the_row() {
        let mut s = store();
        let mgr = notes_fts(&s);
        s.put_record_indexed(&note("n1", "offline rebuild"), &mgr).unwrap();
        // Patch a display field. The FTS index mirrors the stable
        // `field_ids.f_alice_0` (DL-7), which `patch` does not touch, so the
        // searchable value is unchanged — but the sync still runs and must leave
        // exactly one consistent row (no duplicate, no loss).
        let env = s
            .patch_record_indexed(
                "notes",
                "n1",
                &fields(&[("tag", serde_json::json!("data"))])
                    .into_iter()
                    .collect(),
                None,
                &mgr,
            )
            .unwrap();
        assert_eq!(env.fields.get("tag"), Some(&serde_json::json!("data")));
        let hits = mgr
            .fts_match(s.connection(), "notes", "f_alice_0", "offline")
            .unwrap();
        assert_eq!(
            hits,
            vec!["n1".to_string()],
            "exactly one FTS row mirrors the canonical field_id after patch"
        );
    }

    #[test]
    fn put_record_indexed_with_changed_field_id_updates_fts() {
        // When the stable field_id value itself changes (the case FTS truly
        // tracks), put_record_indexed re-mirrors it.
        let mut s = store();
        let mgr = notes_fts(&s);
        s.put_record_indexed(&note("n1", "offline rebuild"), &mgr).unwrap();
        s.put_record_indexed(&note("n1", "lunch plans"), &mgr).unwrap();
        assert!(mgr
            .fts_match(s.connection(), "notes", "f_alice_0", "offline")
            .unwrap()
            .is_empty());
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "lunch").unwrap(),
            vec!["n1".to_string()]
        );
    }

    #[test]
    fn delete_record_indexed_drops_record_from_fts() {
        let mut s = store();
        let mgr = notes_fts(&s);
        s.put_record_indexed(&note("n1", "offline rebuild"), &mgr).unwrap();
        s.put_record_indexed(&note("n2", "offline sync path"), &mgr).unwrap();
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "offline").unwrap(),
            vec!["n1".to_string(), "n2".to_string()]
        );
        // Tombstone n1: it drops out of FTS; n2 still matches.
        let env = s.delete_record_indexed("notes", "n1", None, &mgr).unwrap();
        assert!(env.deleted);
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "offline").unwrap(),
            vec!["n2".to_string()]
        );
    }

    /// Review 041/042 finding 3: the applet-facing DL-17 mutation path
    /// (`apply_mutation` / `transact_mutations`) must keep active FTS shadow
    /// tables in sync in the same transaction. Build FTS, insert/patch a matching
    /// record through the mutation surface, then query WITHOUT a manual rebuild —
    /// the record is found because the mutation refreshed FTS atomically.
    #[test]
    fn apply_mutation_keeps_active_fts_in_sync_without_rebuild() {
        let mut s = store();
        // FTS index over the stable id the display field `body` MATERIALIZES to
        // through the mutation surface (`f_body`), so a record inserted purely via
        // the DL-17 path — with NO pre-seeded field_ids and NO manual rebuild — is
        // searchable. This is the mutation-discriminating coverage review 045/046
        // finding 1 asks for: the mutation must materialize the stable field id
        // (not leave it empty), or the FTS row is never created.
        let mut mgr = IndexManager::new();
        s.create_index(&mut mgr, "notes", "f_body", CreateIndexKind::Fts)
            .expect("create fts index over the materialized stable id");

        // Insert through the applet mutation surface ONLY — no put_record pre-seed.
        let insert = Mutation::Insert {
            collection: "notes".into(),
            id: Some("n1".into()),
            fields: serde_json::json!({"body": "offline rebuild keeps indexes honest"})
                .as_object()
                .unwrap()
                .clone(),
            logical_at: Some(1),
        };
        s.apply_mutation(&insert, &mgr).unwrap();

        // The inserted record carries the materialized stable id...
        let stored = s.get_record("notes", "n1").unwrap().unwrap();
        assert_eq!(
            stored.field_ids.get("f_body"),
            Some(&serde_json::json!("offline rebuild keeps indexes honest")),
            "insert must materialize the stable field_id the index reads"
        );
        // ...and is FTS-visible WITHOUT a rebuild, because the insert synced FTS.
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_body", "offline").unwrap(),
            vec!["n1".to_string()],
            "mutation insert must populate active FTS without a rebuild"
        );

        // A patch that changes the indexed display field re-mirrors FTS in the
        // same transaction: the old term drops, the new term matches.
        let patch = Mutation::Patch {
            collection: "notes".into(),
            id: "n1".into(),
            fields: serde_json::json!({"body": "lunch plans for the team"})
                .as_object()
                .unwrap()
                .clone(),
            logical_at: Some(2),
        };
        s.apply_mutation(&patch, &mgr).unwrap();
        assert!(
            mgr.fts_match(s.connection(), "notes", "f_body", "offline").unwrap().is_empty(),
            "patch must drop the stale term from active FTS"
        );
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_body", "lunch").unwrap(),
            vec!["n1".to_string()],
            "patch must re-mirror the new term into active FTS without a rebuild"
        );

        // A transact group is equally synced: tombstoning n1 drops it from FTS.
        let del = vec![Mutation::Delete {
            collection: "notes".into(),
            id: "n1".into(),
            logical_at: Some(3),
        }];
        s.transact_mutations(&del, &mgr).unwrap();
        assert!(
            mgr.fts_match(s.connection(), "notes", "f_body", "lunch").unwrap().is_empty(),
            "transact delete must drop the record from active FTS"
        );
    }

    #[test]
    fn mutation_preserves_existing_schema_field_ids_and_index_visibility() {
        // Review 049: a display-name mutation must NOT clobber a record's
        // schema-minted stable ids. Seed a record carrying `f_alice_0` and an FTS
        // index keyed on it; a patch of an UNRELATED display field must keep
        // `f_alice_0` present AND keep the record FTS-visible (the active-FTS sync
        // deletes-then-reinserts reading `$.field_ids.f_alice_0`, so dropping that
        // id would strand the record from search).
        let mut s = store();
        let mut mgr = IndexManager::new();

        // Seed a record whose field_ids use a SCHEMA-minted id (not the f_<name>
        // stand-in the mutation surface would mint).
        let mut env = RecordEnvelope::new(
            forge_domain::CollectionId::new("notes"),
            forge_domain::RecordId::new("n1"),
            [("title".to_string(), serde_json::json!("alpha beta gamma"))]
                .into_iter()
                .collect(),
            forge_domain::LogicalTimestamp(1),
        );
        env.field_ids.insert("f_alice_0".into(), serde_json::json!("alpha beta gamma"));
        s.put_record(&env).unwrap();

        // FTS index over the schema id, populated from the canonical record.
        s.create_index(&mut mgr, "notes", "f_alice_0", CreateIndexKind::Fts)
            .expect("create fts index on the schema-minted id");
        s.build_indexes(&mgr).unwrap();
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "beta").unwrap(),
            vec!["n1".to_string()],
            "baseline: record is FTS-visible on its schema id"
        );

        // Patch an UNRELATED display field through the DL-17 mutation surface.
        let patch = Mutation::Patch {
            collection: "notes".into(),
            id: "n1".into(),
            fields: serde_json::json!({"tag": "urgent"}).as_object().unwrap().clone(),
            logical_at: Some(2),
        };
        s.apply_mutation(&patch, &mgr).unwrap();

        let stored = s.get_record("notes", "n1").unwrap().unwrap();
        // The schema id SURVIVES the mutation (the bug clobbered it to f_title/f_tag).
        assert_eq!(
            stored.field_ids.get("f_alice_0"),
            Some(&serde_json::json!("alpha beta gamma")),
            "schema-minted f_alice_0 must survive a display-name patch (review 049)"
        );
        // And the record is STILL FTS-visible on the schema id after the patch.
        assert_eq!(
            mgr.fts_match(s.connection(), "notes", "f_alice_0", "beta").unwrap(),
            vec!["n1".to_string()],
            "record must remain FTS-visible on its schema id after an unrelated patch"
        );
        // The new display field also got its stand-in (so it is itself indexable).
        assert_eq!(
            stored.field_ids.get("f_tag"),
            Some(&serde_json::json!("urgent")),
            "the patched display field still materializes its own stand-in id"
        );
    }

    /// Review 041/042 finding 4: text_search composes with the rest of the
    /// pipeline. The FTS MATCH set is filtered by `where`, reduced by `aggregate`,
    /// and bounded by rank-path `limit` — none of which the early-return path used
    /// to honor.
    /// Build a note with a `tag` display field (so a text search can compose
    /// with a `tag` filter), populating the FTS-mirrored `field_ids.f_alice_0`.
    fn tagged_note(id: &str, body: &str, tag: &str) -> RecordEnvelope {
        let mut env = note(id, body);
        env.fields.insert("tag".into(), serde_json::json!(tag));
        env
    }

    #[test]
    fn text_search_composes_with_filter_aggregate_and_limit() {
        let mut s = store();
        let mgr = notes_fts(&s);
        // Three notes match "offline"; tag distinguishes two buckets.
        s.put_record_indexed(&tagged_note("n1", "offline rebuild keeps indexes honest", "data"), &mgr).unwrap();
        s.put_record_indexed(&tagged_note("n2", "lunch plans for the team", "personal"), &mgr).unwrap(); // no "offline"
        s.put_record_indexed(&tagged_note("n3", "offline sync path for the data plane", "data"), &mgr).unwrap();
        s.put_record_indexed(&tagged_note("n4", "offline indexing notes", "personal"), &mgr).unwrap();

        // text_search + filter: "offline" matches n1,n3,n4; tag=data keeps n1,n3.
        let q_filter = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "where": {"field": "tag", "op": "eq", "value": "data"}
        }))
        .unwrap();
        let planned_filter = s.query_planned(&q_filter, &mgr).unwrap();
        let mut ids = planned_filter.ids();
        ids.sort();
        assert_eq!(ids, vec!["n1", "n3"], "text search must compose with the where filter");
        // Review 045/046 finding 2: the FTS MATCH serves the search (uses_index),
        // but the `tag` filter is over an UNINDEXED field, so the planner must NOT
        // suppress the full_scan warning just because a text search is present.
        assert!(planned_filter.uses_index, "the FTS match still uses its index");
        assert!(
            planned_filter.warnings.iter().any(|w| {
                w.code == "planner.full_scan"
                    && w.reason == FullScanReason::NoIndex
                    && w.field_name.as_deref() == Some("tag")
            }),
            "text_search must not mask the uncovered `tag` filter scan: {:?}",
            planned_filter.warnings
        );

        // text_search + aggregate(count): the FTS-matched, filtered set is 2.
        let q_count = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "where": {"field": "tag", "op": "eq", "value": "data"},
            "aggregate": {"op": "count"}
        }))
        .unwrap();
        match s.query_planned(&q_count, &mgr).unwrap().result {
            QueryResult::Aggregate(a) => assert_eq!(a.count, Some(2), "aggregate over the match set"),
            other => panic!("expected aggregate, got {other:?}"),
        }

        // text_search + rank-path limit: limit bounds the rank-ordered result
        // (previously dropped on the rank path).
        let q_limit = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "order_by": [{"field": "rank", "dir": "asc"}],
            "limit": 1
        }))
        .unwrap();
        assert_eq!(
            s.query_planned(&q_limit, &mgr).unwrap().ids().len(),
            1,
            "rank-path limit must bound the result"
        );
    }

    #[test]
    fn text_search_without_active_fts_falls_back_and_still_composes() {
        // No FTS index registered: the portable scan finds the match set and the
        // filter/limit pipeline still composes (records are canonical), with a
        // fts_not_available warning surfaced.
        let s = store();
        let empty = IndexManager::new();
        s.put_record(&tagged_note("n1", "offline rebuild", "data")).unwrap();
        s.put_record(&tagged_note("n2", "offline lunch", "personal")).unwrap();
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "notes",
            "text_search": {"field_id": "f_alice_0", "match": "offline"},
            "where": {"field": "tag", "op": "eq", "value": "data"}
        }))
        .unwrap();
        let planned = s.query_planned(&q, &empty).unwrap();
        assert!(!planned.uses_index, "no active FTS -> fallback scan");
        assert_eq!(planned.warnings[0].reason, FullScanReason::FtsNotAvailable);
        assert_eq!(planned.ids(), vec!["n1"], "fallback still composes the filter");
    }

    #[test]
    fn store_create_index_builds_value_index_from_existing_records() {
        // DL-6: create after records exist -> the planner can use it.
        let s = store();
        let mut env = sample_record("tasks", "t1", "Ship");
        env.field_ids.insert("f_alice_1".into(), serde_json::json!("open"));
        s.put_record(&env).unwrap();
        let mut mgr = IndexManager::new();
        let id = s
            .create_index(&mut mgr, "tasks", "f_alice_1", CreateIndexKind::Value)
            .unwrap();
        assert_eq!(id, "idx_records_tasks_f_alice_1");
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_alice_1", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        let planned = s.query_planned(&q, &mgr).unwrap();
        assert!(planned.uses_index);
        assert_eq!(planned.index_id.as_deref(), Some("idx_records_tasks_f_alice_1"));
    }
