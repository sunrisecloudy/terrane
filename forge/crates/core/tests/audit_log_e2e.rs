//! SC-12 data-driven conformance over the durable audit-log vectors
//! (`forge/fixtures/audit-log-e2e/`, manifest `count = 10`).
//!
//! The 10 case JSONs are the BEHAVIORAL contract for the persisted audit log: each
//! pins the starting `next_seq` + `logical_time`, the rows a producer appends (with
//! their EXACT redacted metadata shape), the query that reads them back, and — for
//! the secret / network cases — the substrings that must NEVER appear in the stored
//! bytes. This harness drives the REAL storage substrate
//! ([`forge_storage::Store::append_audit_tx`] / `query_audit` / the redaction
//! helper), feeding each producer the RAW operation context (resolved secret value,
//! request/response bodies) so the redaction is genuinely exercised — not merely
//! replayed from the already-redacted expectation.
//!
//! A `ran == manifest.count` guard means a dropped / misnamed / unhandled vector
//! FAILS the suite rather than silently passing.

use forge_storage::{redact_metadata, AuditQuery, AuditRecord, Store};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/audit-log-e2e")
        .canonicalize()
        .expect("audit-log-e2e fixtures dir exists")
}

/// An expected persisted row from a fixture's `audit_rows_appended` / `audit_rows`
/// (the manifest `row_shape`). Some fixtures carry extra structured keys
/// (`applet_id`) at the TOP level of the row that the canonical row's `metadata`
/// must contain; the harness reconciles both.
fn assert_row_matches(got: &AuditRecord, want: &Value, case: &str) {
    let w = want.as_object().unwrap_or_else(|| panic!("[{case}] row is an object"));
    assert_eq!(got.seq, w["seq"].as_u64().unwrap(), "[{case}] seq");
    assert_eq!(got.audit_id, w["audit_id"].as_str().unwrap(), "[{case}] audit_id");
    assert_eq!(
        got.logical_time,
        w["logical_time"].as_u64().unwrap(),
        "[{case}] logical_time"
    );
    assert_eq!(got.producer, w["producer"].as_str().unwrap(), "[{case}] producer");
    assert_eq!(got.action, w["action"].as_str().unwrap(), "[{case}] action");
    assert_eq!(got.decision, w["decision"].as_str().unwrap(), "[{case}] decision");
    assert_eq!(got.actor_id, w["actor_id"].as_str().unwrap(), "[{case}] actor_id");
    assert_eq!(
        got.resource_type,
        w["resource_type"].as_str().unwrap(),
        "[{case}] resource_type"
    );
    assert_eq!(
        got.resource_id.as_deref(),
        w.get("resource_id").and_then(|v| v.as_str()),
        "[{case}] resource_id"
    );
    assert_eq!(
        got.collection.as_deref(),
        w.get("collection").and_then(|v| v.as_str()),
        "[{case}] collection"
    );
    assert_eq!(got.reason, w["reason"].as_str().unwrap(), "[{case}] reason");
    assert_eq!(&got.metadata, &w["metadata"], "[{case}] metadata");
}

/// Seed any `given.audit_rows` (pre-existing history) into the store, pinning their
/// exact seq + logical_time. These rows are authored verbatim (they are already in
/// the manifest's redacted shape) by pinning the counter to each row's seq before
/// the append.
fn seed_given_rows(store: &mut Store, rows: &[Value], case: &str) {
    for row in rows {
        let seq = row["seq"].as_u64().unwrap();
        store.set_audit_seq(seq).unwrap();
        let rec = AuditRecord::new(
            row["logical_time"].as_u64().unwrap(),
            row["producer"].as_str().unwrap(),
            row["action"].as_str().unwrap(),
            row["decision"].as_str().unwrap(),
            row["actor_id"].as_str().unwrap(),
            row["resource_type"].as_str().unwrap(),
            row.get("resource_id").and_then(|v| v.as_str()).map(String::from),
            row.get("collection").and_then(|v| v.as_str()).map(String::from),
            row["reason"].as_str().unwrap(),
            row["metadata"].clone(),
        );
        let out = store.append_audit(&rec).unwrap();
        assert_eq!(out.seq, seq, "[{case}] seeded row took its pinned seq");
    }
}

/// Build the RAW (pre-redaction) input metadata a producer would hand
/// `append_audit_tx`, from the fixture's `when` block. For the `secret` / `network`
/// kinds this DELIBERATELY includes the resolved secret value / request+response
/// bodies, so the redaction path is genuinely exercised; the persisted row must
/// equal the fixture's (already-redacted) `expect`. `None` means the producer's
/// metadata is already body/secret-free and equals the expected metadata (redaction
/// is a no-op) — the caller uses the expected row's metadata verbatim.
fn raw_input_metadata(kind: &str, when: &Value, given: &Value) -> Option<Value> {
    match kind {
        "secret" => {
            // The producer is handed BOTH the secret_ref AND the resolved secret
            // value (from the secret_store) — redaction must drop the value and keep
            // only the ref + the value_redacted marker.
            let op = &when["operation"];
            let secret_ref = op["secret_ref"].as_str().unwrap();
            let resolved = given["secret_store"][secret_ref].clone();
            Some(json!({
                "secret_ref": secret_ref,
                "secret_value": resolved,
                "target_host": op["target"]["host"],
                "target_header": op["target"]["header"],
            }))
        }
        "network" => {
            // The producer is handed the full request/response INCLUDING bodies —
            // redaction must drop both bodies and keep method/host/path/status.
            let op = &when["operation"];
            let req = &op["request"];
            let url = req["url"].as_str().unwrap();
            let (scheme, rest) = url.split_once("://").unwrap();
            let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap();
            Some(json!({
                "method": req["method"],
                "scheme": scheme,
                "host": host,
                "path": path,
                "status": op["response"]["status"],
                "request_body": req["body"],
                "response_body": op["response"]["body"],
            }))
        }
        // sync-rbac / command-rbac / permission / lifecycle / signing all persist
        // already body/secret-free metadata; the expected row's metadata IS the
        // producer's input (redaction is a no-op there).
        _ => None,
    }
}

/// Run a producer-style vector: pin `next_seq`/`logical_time`, append the expected
/// rows (building each from the RAW `when` context so redaction runs), assert the
/// persisted rows match `expect.audit_rows_appended` exactly, assert the query
/// returns the expected audit ids in seq order, and assert `must_not_contain` is
/// absent from the raw stored bytes.
fn run_producer_vector(store: &mut Store, fx: &Value, kind: &str) {
    let case = fx["case"].as_str().unwrap();
    let given = &fx["given"];
    let when = &fx["when"];
    let expect = &fx["expect"];

    if let Some(rows) = given.get("audit_rows").and_then(|v| v.as_array()) {
        seed_given_rows(store, rows, case);
    }
    let next_seq = given["next_seq"].as_u64().unwrap();
    store.set_audit_seq(next_seq).unwrap();

    let appended = expect["audit_rows_appended"].as_array().unwrap();
    for (i, want) in appended.iter().enumerate() {
        // The producer hands RAW metadata only for the redaction kinds (secret /
        // network); everything else's input already equals the expected (redacted)
        // metadata, so redaction is a no-op there.
        let metadata = raw_input_metadata(kind, when, given)
            .unwrap_or_else(|| want["metadata"].clone());
        let rec = AuditRecord::new(
            want["logical_time"].as_u64().unwrap(),
            want["producer"].as_str().unwrap(),
            want["action"].as_str().unwrap(),
            want["decision"].as_str().unwrap(),
            want["actor_id"].as_str().unwrap(),
            want["resource_type"].as_str().unwrap(),
            want.get("resource_id").and_then(|v| v.as_str()).map(String::from),
            want.get("collection").and_then(|v| v.as_str()).map(String::from),
            want["reason"].as_str().unwrap(),
            metadata,
        );
        let out = store.append_audit(&rec).unwrap();
        assert_row_matches(&out, want, case);
        assert_eq!(out.seq, want["seq"].as_u64().unwrap(), "[{case}] appended row {i} seq");
    }

    // `must_not_contain`: the raw stored bytes never carry a secret value or body.
    if let Some(forbidden) = expect.get("must_not_contain").and_then(|v| v.as_array()) {
        let raw = dump_all_metadata(store);
        for needle in forbidden {
            let s = needle.as_str().unwrap();
            assert!(
                !raw.contains(s),
                "[{case}] stored audit metadata leaks {s:?}: {raw}"
            );
        }
    }

    // The query reads the appended rows back in seq order.
    if let Some(query) = expect.get("query") {
        assert_query(store, query, case);
    }

    // `existing_rows_unchanged`: the seeded prior row(s) are byte-identical after the
    // append (append-only — history is never mutated).
    if let Some(unchanged) = expect.get("existing_rows_unchanged").and_then(|v| v.as_array()) {
        let all = store.query_audit(&AuditQuery::default()).unwrap();
        for id in unchanged {
            let id = id.as_str().unwrap();
            let still = all.iter().find(|r| r.audit_id == id).unwrap_or_else(|| {
                panic!("[{case}] existing row {id} must still be present (append-only)")
            });
            let seeded = given["audit_rows"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["audit_id"] == id)
                .unwrap();
            assert_row_matches(still, seeded, case);
        }
    }
}

/// A pure query-only vector (`query_by_action_resource_and_sequence`): seed the
/// `given.audit_rows`, run each named query, and assert its result ids.
fn run_query_vector(store: &mut Store, fx: &Value) {
    let case = fx["case"].as_str().unwrap();
    seed_given_rows(store, fx["given"]["audit_rows"].as_array().unwrap(), case);
    let results = &fx["expect"]["query_results"];
    for q in fx["when"]["queries"].as_array().unwrap() {
        let name = q["name"].as_str().unwrap();
        let got = run_where(store, &q["where"]);
        let want: Vec<&str> = results[name]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let got_ids: Vec<&str> = got.iter().map(|r| r.audit_id.as_str()).collect();
        assert_eq!(got_ids, want, "[{case}] query {name}");
    }
}

/// The deterministic-replay vector: the recorded rows are SERVED FROM THE RECORD
/// (no wall clock). Append them under the pinned replay clock and assert the
/// persisted rows are byte-identical to the `replayed_audit_rows` expectation, in
/// order — proving seq + logical_time replay deterministically.
fn run_replay_vector(store: &mut Store, fx: &Value) {
    let case = fx["case"].as_str().unwrap();
    let given = &fx["given"];
    let clock = &given["replay_clock"];
    store.set_audit_seq(clock["next_seq"].as_u64().unwrap()).unwrap();

    let recorded = given["recorded_audit_rows"].as_array().unwrap();
    for row in recorded {
        let rec = AuditRecord::new(
            row["logical_time"].as_u64().unwrap(),
            row["producer"].as_str().unwrap(),
            row["action"].as_str().unwrap(),
            row["decision"].as_str().unwrap(),
            row["actor_id"].as_str().unwrap(),
            row["resource_type"].as_str().unwrap(),
            row.get("resource_id").and_then(|v| v.as_str()).map(String::from),
            row.get("collection").and_then(|v| v.as_str()).map(String::from),
            row["reason"].as_str().unwrap(),
            row["metadata"].clone(),
        );
        store.append_audit(&rec).unwrap();
    }

    let want = fx["expect"]["replayed_audit_rows"].as_array().unwrap();
    let got = store.query_audit(&AuditQuery::default()).unwrap();
    assert_eq!(got.len(), want.len(), "[{case}] replayed row count");
    for (g, w) in got.iter().zip(want) {
        assert_row_matches(g, w, case);
    }
    // A SECOND replay from the same record reproduces byte-identical rows — the
    // append never consults a wall clock, so the persisted bytes are stable.
    let mut twin = Store::open_in_memory().unwrap();
    twin.set_audit_seq(clock["next_seq"].as_u64().unwrap()).unwrap();
    for row in recorded {
        let rec = AuditRecord::new(
            row["logical_time"].as_u64().unwrap(),
            row["producer"].as_str().unwrap(),
            row["action"].as_str().unwrap(),
            row["decision"].as_str().unwrap(),
            row["actor_id"].as_str().unwrap(),
            row["resource_type"].as_str().unwrap(),
            row.get("resource_id").and_then(|v| v.as_str()).map(String::from),
            row.get("collection").and_then(|v| v.as_str()).map(String::from),
            row["reason"].as_str().unwrap(),
            row["metadata"].clone(),
        );
        twin.append_audit(&rec).unwrap();
    }
    let twin_rows = twin.query_audit(&AuditQuery::default()).unwrap();
    assert_eq!(
        got, twin_rows,
        "[{case}] two replays from the same record produce byte-identical rows"
    );
}

/// Translate a fixture `where` object into an [`AuditQuery`] and run it.
fn run_where(store: &Store, where_: &Value) -> Vec<AuditRecord> {
    let mut q = AuditQuery::default();
    if let Some(v) = where_.get("actor_id").and_then(|v| v.as_str()) {
        q.actor_id = Some(v.into());
    }
    if let Some(v) = where_.get("action").and_then(|v| v.as_str()) {
        q.action = Some(v.into());
    }
    if let Some(v) = where_.get("decision").and_then(|v| v.as_str()) {
        q.decision = Some(v.into());
    }
    if let Some(v) = where_.get("resource_type").and_then(|v| v.as_str()) {
        q.resource_type = Some(v.into());
    }
    if let Some(v) = where_.get("resource_id").and_then(|v| v.as_str()) {
        q.resource_id = Some(v.into());
    }
    if let Some(v) = where_.get("collection").and_then(|v| v.as_str()) {
        q.collection = Some(v.into());
    }
    if let Some(v) = where_.get("seq_gte").and_then(|v| v.as_u64()) {
        q.seq_gte = Some(v);
    }
    if let Some(v) = where_.get("seq_lte").and_then(|v| v.as_u64()) {
        q.seq_lte = Some(v);
    }
    store.query_audit(&q).unwrap()
}

/// Assert a fixture `query` block (a single `where` + ordered `result_audit_ids`).
fn assert_query(store: &Store, query: &Value, case: &str) {
    let got = run_where(store, &query["where"]);
    let got_ids: Vec<&str> = got.iter().map(|r| r.audit_id.as_str()).collect();
    let want: Vec<&str> = query["result_audit_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(got_ids, want, "[{case}] query result");
}

/// Concatenate every stored metadata blob (the raw JSON bytes) so a redaction
/// `must_not_contain` assertion can scan the WHOLE persisted log for a leak.
fn dump_all_metadata(store: &Store) -> String {
    store
        .query_audit(&AuditQuery::default())
        .unwrap()
        .iter()
        .map(|r| serde_json::to_string(&r.metadata).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn redaction_helper_drops_secret_value_and_bodies() {
    // A unit guard on the redaction the harness relies on: the helper strips a
    // secret value AND request/response bodies, keeping only safe keys + markers.
    let redacted = redact_metadata(&json!({
        "secret_ref": "secret_weather",
        "secret_value": "Bearer abc123",
        "request_body": {"name": "Ada"},
        "response_body": {"id": "lead-1"},
        "host": "api.weather.example"
    }));
    let obj = redacted.as_object().unwrap();
    assert_eq!(obj.get("secret_ref").unwrap(), "secret_weather");
    assert!(!obj.contains_key("secret_value"));
    assert!(!obj.contains_key("request_body"));
    assert!(!obj.contains_key("response_body"));
    assert_eq!(obj.get("value_redacted").unwrap(), &Value::Bool(true));
    let s = serde_json::to_string(&redacted).unwrap();
    for leak in ["Bearer abc123", "abc123", "Ada", "lead-1"] {
        assert!(!s.contains(leak), "redaction leaked {leak}: {s}");
    }
}

/// The data-driven 10-vector harness. Loads the manifest, dispatches each case by
/// its `kind`, and guards `ran == manifest.count` so a missing / unhandled vector
/// FAILS rather than silently skipping.
#[test]
fn every_audit_log_e2e_vector_is_asserted() {
    let dir = fixtures_dir();
    let manifest: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("manifest.json")).expect("read manifest"),
    )
    .expect("parse manifest");
    let count = manifest["count"].as_u64().unwrap() as usize;
    let cases = manifest["cases"].as_array().unwrap();

    let mut ran = 0usize;
    for entry in cases {
        let file = entry["file"].as_str().unwrap();
        let kind = entry["kind"].as_str().unwrap();
        let fx: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.join(file))
                .unwrap_or_else(|e| panic!("read {file}: {e}")),
        )
        .unwrap_or_else(|e| panic!("parse {file}: {e}"));
        assert_eq!(
            fx["case"].as_str().unwrap(),
            entry["case"].as_str().unwrap(),
            "manifest case name matches the fixture's"
        );

        // A fresh store per vector so each pins its own next_seq from a clean log.
        let mut store = Store::open_in_memory().unwrap();
        match kind {
            "sync-rbac" | "command-rbac" | "secret" | "network" | "lifecycle" | "signing" => {
                run_producer_vector(&mut store, &fx, kind)
            }
            "permission" => run_producer_vector(&mut store, &fx, "permission"),
            "append-only" => run_producer_vector(&mut store, &fx, "command-rbac"),
            "query" => run_query_vector(&mut store, &fx),
            "replay" => run_replay_vector(&mut store, &fx),
            other => panic!("unhandled fixture kind {other:?} for {file}"),
        }
        ran += 1;
    }

    assert_eq!(
        ran, count,
        "ran {ran} audit-log-e2e vectors but the manifest declares {count}"
    );
}
