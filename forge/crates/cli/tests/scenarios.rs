//! Data-driven e2e coverage over the committed scenario corpus
//! (prd-merged/09 M0a exit). Each `fixtures/e2e/<name>/` directory is a full
//! spine case — `applet.ts` + `manifest.json` + `input.json` + `expect.json` —
//! and this test runs every one **through the command/event facade**
//! ([`forge_core::WorkspaceCore::handle`]) and asserts its `expect.json`
//! outcome. Together with `e2e.rs` (notes-lite) these are the spine's real
//! acceptance coverage.
//!
//! ## Why this test drives the facade (not the runtime directly)
//!
//! This corpus is the CLI/PS-5 acceptance proof (prd-merged/06 PS-5,
//! prd-merged/09 M0a exit), so it must exercise the **same path a shell uses**:
//! `applet.install` (TS → SWC transpile + policy scan → store) and
//! `runtime.run` (capability-gated QuickJS → SQLite write → UI patch → recorded
//! [`RunRecord`]) and `runtime.replay`, all through [`WorkspaceCore::handle`] —
//! command authorization, run persistence, and the response contract included.
//!
//! `expect.json` pins a per-scenario `random_seed`/`time_start` (e.g.
//! `seeded_random` is recorded under seed 7, `time_log` under time_start 500).
//! Those seeds flow into the asserted `result`/`records`/`ui`. The facade's
//! `runtime.run` command accepts an explicit `(random_seed, time_start)`
//! override (review 032 finding 1), so each scenario is reproduced through the
//! real command — not a parallel pipeline+runtime+bridge path.

use forge_cli::{handle, install, list_records};
use forge_core::WorkspaceCore;
use forge_domain::CoreError;
use std::path::{Path, PathBuf};

/// The applet id every scenario installs under (one applet per fresh workspace).
const SCENARIO_APPLET_ID: &str = "scenario";

/// The parsed `expect.json` of a scenario (the subset this corpus uses).
#[derive(serde::Deserialize)]
struct Expect {
    /// "run" (compile + execute) or "install" (compile must reject).
    stage: String,
    #[serde(default)]
    random_seed: u64,
    #[serde(default)]
    time_start: u64,
    #[serde(default)]
    result: serde_json::Value,
    #[serde(default)]
    records: Vec<ExpectRecord>,
    #[serde(default)]
    storage: serde_json::Map<String, serde_json::Value>,
    #[serde(default)]
    ui_contains: Vec<String>,
    #[serde(default)]
    host_call_methods: Vec<String>,
    #[serde(default)]
    replay_identical: bool,
    // install-stage fields
    #[serde(default)]
    install_rejected: bool,
    #[serde(default)]
    error_code: Option<String>,
    #[serde(default)]
    error_contains: Option<String>,
}

#[derive(serde::Deserialize)]
struct ExpectRecord {
    collection: String,
    id: String,
    fields: serde_json::Value,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/e2e")
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name))
        .unwrap_or_else(|e| panic!("read {}/{name}: {e}", dir.display()))
}

/// Run every scenario directory and assert its `expect.json`.
#[test]
fn every_committed_scenario_meets_its_expectation() {
    let root = fixtures_dir();
    let mut scenarios: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("read fixtures dir {}: {e}", root.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    scenarios.sort();
    assert!(
        scenarios.len() >= 8,
        "expected the 8 committed scenarios, found {}",
        scenarios.len()
    );

    for dir in &scenarios {
        let name = dir.file_name().unwrap().to_string_lossy().to_string();
        run_one(dir).unwrap_or_else(|e| panic!("scenario {name:?} failed: {e}"));
    }
}

/// Run a single scenario through the facade and assert it. Returns `Err(String)`
/// with a precise message so the harness can name the failing scenario.
fn run_one(dir: &Path) -> Result<(), String> {
    let ts = read(dir, "applet.ts");
    let manifest_json = read(dir, "manifest.json");
    let expect: Expect = serde_json::from_str(&read(dir, "expect.json"))
        .map_err(|e| format!("parse expect.json: {e}"))?;

    match expect.stage.as_str() {
        "install" => assert_install_stage(&manifest_json, &ts, &expect),
        "run" => assert_run_stage(dir, &manifest_json, &ts, &expect),
        other => Err(format!("unknown stage {other:?}")),
    }
}

/// Install stage (e.g. `rejected_eval`): `applet.install` — the front of the
/// spine through the facade (static policy scan + transpile) — must REJECT with
/// the expected `CoreError`, the applet never installs, and a follow-up
/// `runtime.run` reports it missing (so nothing was stored).
fn assert_install_stage(manifest_json: &str, ts: &str, expect: &Expect) -> Result<(), String> {
    assert!(expect.install_rejected, "install-stage expect must set install_rejected");

    let mut core = WorkspaceCore::in_memory("ws-scenario").map_err(|e| format!("open core: {e}"))?;

    let err = match install(&mut core, SCENARIO_APPLET_ID, manifest_json, ts) {
        Ok(_) => return Err("applet.install accepted a source that must be rejected".into()),
        Err(e) => e,
    };

    if let Some(code) = &expect.error_code {
        if err.code() != code {
            return Err(format!("error_code: expected {code}, got {}", err.code()));
        }
    }
    if let Some(needle) = &expect.error_contains {
        if !err.to_string().contains(needle) {
            return Err(format!("error_contains {needle:?}: got {err}"));
        }
    }

    // The applet never installed ⇒ a run reports it missing (nothing stored).
    let run = handle(
        &mut core,
        Some(SCENARIO_APPLET_ID),
        "runtime.run",
        serde_json::json!({ "input": {} }),
    );
    match run {
        Ok(_) => Err("a rejected install must leave no runnable applet".into()),
        Err(CoreError::ValidationError(_)) => Ok(()),
        Err(e) => Err(format!("expected ValidationError (applet missing), got {e}")),
    }
}

/// Run stage: install the applet, run it through `runtime.run` with the
/// scenario's pinned seeds, then assert the result/records/storage/ui/host-calls
/// and that `runtime.replay` reports byte-identical replay — every link asserted
/// through the facade's command/response contract.
fn assert_run_stage(
    dir: &Path,
    manifest_json: &str,
    ts: &str,
    expect: &Expect,
) -> Result<(), String> {
    let input: serde_json::Value =
        serde_json::from_str(&read(dir, "input.json")).map_err(|e| format!("parse input.json: {e}"))?;

    let mut core = WorkspaceCore::in_memory("ws-scenario").map_err(|e| format!("open core: {e}"))?;

    // ---- applet.install (TS → SWC → policy scan → store), through the facade --
    install(&mut core, SCENARIO_APPLET_ID, manifest_json, ts)
        .map_err(|e| format!("applet.install: {e}"))?;

    // ---- runtime.run with the scenario's pinned deterministic seeds ----------
    let run_resp = handle(
        &mut core,
        Some(SCENARIO_APPLET_ID),
        "runtime.run",
        serde_json::json!({
            "input": input,
            "random_seed": expect.random_seed,
            "time_start": expect.time_start,
        }),
    )
    .map_err(|e| format!("runtime.run: {e}"))?;

    let run_id = run_resp
        .get("run_id")
        .and_then(|v| v.as_str())
        .ok_or("runtime.run returned no run_id")?
        .to_string();

    // ---- result (ok + value, OR ok:false + error_code, e.g. denied_capability)
    assert_result(&run_resp, expect)?;

    // ---- host-call trace (method sequence the run issued), from the response --
    if !expect.host_call_methods.is_empty() {
        let got: Vec<String> = run_resp
            .get("host_call_methods")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        if got != expect.host_call_methods {
            return Err(format!(
                "host_call_methods: expected {:?}, got {:?}",
                expect.host_call_methods, got
            ));
        }
    }

    // ---- records stored (the SQLite write link), read back via query.execute --
    assert_records(&mut core, expect)?;

    // ---- storage KV (counter scenario), read back from the workspace store ----
    assert_storage(&core, expect)?;

    // ---- ui markers present in the emitted tree(s) (from the run response) ----
    let ui_trees: Vec<serde_json::Value> = run_resp
        .get("ui_renders")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for marker in &expect.ui_contains {
        if !ui_trees.iter().any(|t| ui_contains(t, marker)) {
            return Err(format!("ui_contains {marker:?} not found in {ui_trees:?}"));
        }
    }

    // ---- deterministic replay (byte-identical fingerprint), via runtime.replay
    let replay_resp = handle(
        &mut core,
        None,
        "runtime.replay",
        serde_json::json!({ "run_id": run_id }),
    );
    match (&replay_resp, expect.replay_identical) {
        (Ok(payload), true) => {
            let identical = payload
                .get("replays_identically")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !identical {
                return Err("runtime.replay reported the run does NOT replay identically".into());
            }
        }
        (Ok(_), false) => {
            return Err("expected replay to differ, but runtime.replay reported identical".into());
        }
        (Err(e), true) => return Err(format!("runtime.replay: {e}")),
        // A scenario that expects non-identical replay (none in the run corpus)
        // would legitimately surface a replay error; tolerate that here.
        (Err(_), false) => {}
    }

    Ok(())
}

/// Assert the run's `result` against the scenario's expectation. The
/// `runtime.run` response carries `ok` plus the `AppResult` (`{ ok, value }`) on
/// success, or `{ error: <CoreError> }` on a run-outcome failure.
fn assert_result(run_resp: &serde_json::Value, expect: &Expect) -> Result<(), String> {
    let want_ok = expect
        .result
        .get("ok")
        .and_then(|v| v.as_bool())
        .ok_or("expect.result.ok missing")?;
    let got_ok = run_resp.get("ok").and_then(|v| v.as_bool()).ok_or("response.ok missing")?;

    if want_ok != got_ok {
        return Err(format!(
            "result.ok: expected {want_ok}, got {got_ok} (result: {})",
            run_resp.get("result").unwrap_or(&serde_json::Value::Null)
        ));
    }

    if want_ok {
        if let Some(want_value) = expect.result.get("value") {
            let got_value = run_resp
                .get("result")
                .and_then(|r| r.get("value"))
                .unwrap_or(&serde_json::Value::Null);
            if got_value != want_value {
                return Err(format!(
                    "result.value mismatch: expected {want_value}, got {got_value}"
                ));
            }
        }
    } else if let Some(code) = expect.result.get("error_code").and_then(|v| v.as_str()) {
        // EXACT denial-code equality (review 032 finding 2): the run-outcome
        // error's `kind` must equal the fixture's pinned code, not a fuzzy
        // "any denial" match. The failure outcome is `{ "error": { "kind": ... } }`.
        let got_code = run_resp
            .get("result")
            .and_then(|r| r.get("error"))
            .and_then(|e| e.get("kind"))
            .and_then(|k| k.as_str())
            .ok_or_else(|| {
                format!(
                    "run failed but response carried no error.kind: {}",
                    run_resp.get("result").unwrap_or(&serde_json::Value::Null)
                )
            })?;
        if got_code != code {
            return Err(format!("result.error_code: expected {code}, got {got_code}"));
        }
    }
    Ok(())
}

/// Assert the records projection through the facade's `query.execute`, including
/// the empty-set cases (denied_capability writes nothing).
fn assert_records(core: &mut WorkspaceCore, expect: &Expect) -> Result<(), String> {
    // Group expected records by collection so we can check each collection's
    // full contents (including the empty-set cases).
    let mut collections: std::collections::BTreeSet<String> = expect
        .records
        .iter()
        .map(|r| r.collection.clone())
        .collect();
    // Collections the corpus uses that may be expected empty:
    for known in ["notes", "inventory", "audit_log"] {
        collections.insert(known.to_string());
    }

    for collection in &collections {
        let rows =
            list_records(core, collection).map_err(|e| format!("query.execute({collection}): {e}"))?;
        let want: Vec<&ExpectRecord> = expect
            .records
            .iter()
            .filter(|r| &r.collection == collection)
            .collect();
        if rows.len() != want.len() {
            return Err(format!(
                "records[{collection}]: expected {} record(s), stored {}",
                want.len(),
                rows.len()
            ));
        }
        for (got, exp) in rows.iter().zip(want.iter()) {
            let got_id = got.get("id").and_then(|v| v.as_str()).unwrap_or_default();
            if got_id != exp.id {
                return Err(format!(
                    "records[{collection}] id: expected {}, got {got_id}",
                    exp.id
                ));
            }
            let got_fields = got.get("fields").cloned().unwrap_or(serde_json::Value::Null);
            if got_fields != exp.fields {
                return Err(format!(
                    "records[{collection}/{}] fields: expected {}, got {got_fields}",
                    exp.id, exp.fields
                ));
            }
        }
    }
    Ok(())
}

/// Assert the applet KV storage (counter scenario) by reading the workspace
/// store the facade wrote through (`applet/<id>` namespace, read-only).
fn assert_storage(core: &WorkspaceCore, expect: &Expect) -> Result<(), String> {
    let ns = format!("applet/{SCENARIO_APPLET_ID}");
    for (key, want) in &expect.storage {
        let bytes = core
            .store()
            .kv_get(&ns, key)
            .map_err(|e| format!("kv_get({key}): {e}"))?
            .ok_or_else(|| format!("storage key {key:?} not set"))?;
        // The bridge stores JSON-encoded values; the corpus records the string
        // form (counter stores `String(next)` ⇒ a JSON string).
        let got: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("decode storage {key}: {e}"))?;
        if &got != want {
            return Err(format!("storage[{key}]: expected {want}, got {got}"));
        }
    }
    Ok(())
}

/// Does `tree` contain a node matching `marker`?
///
/// Marker grammar (from the corpus): `"Type"` (a node of that type exists) or
/// `"Type:payload"` where payload is the node's display string — `text` for
/// `Text`, `value` for `TextField`.
fn ui_contains(tree: &serde_json::Value, marker: &str) -> bool {
    let (ty, payload) = match marker.split_once(':') {
        Some((ty, p)) => (ty, Some(p)),
        None => (marker, None),
    };
    node_matches(tree, ty, payload)
}

fn node_matches(node: &serde_json::Value, ty: &str, payload: Option<&str>) -> bool {
    if node.get("type").and_then(|v| v.as_str()) == Some(ty) {
        match payload {
            None => return true,
            Some(p) => {
                // Text uses `text`; TextField uses `value`.
                let display = node
                    .get("text")
                    .or_else(|| node.get("value"))
                    .and_then(|v| v.as_str());
                if display == Some(p) {
                    return true;
                }
            }
        }
    }
    // Recurse into children/items.
    for key in ["children", "items"] {
        if let Some(arr) = node.get(key).and_then(|v| v.as_array()) {
            if arr.iter().any(|c| node_matches(c, ty, payload)) {
                return true;
            }
        }
    }
    false
}
