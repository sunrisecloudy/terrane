//! Engine tests for the `host` capability — running a JS backend in QuickJS over
//! a sandboxed app-scoped `ctx.resource.kv`. Uses tiny inline bundles so the
//! tests are self-contained (the full todo e2e lives in terrane-cli).

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_core::Core;

use crate::helpers::req;

/// A minimal backend exercising every kv bridge method, incl. in-run
/// read-after-write (`raw`).
const BACKEND: &str = r#"
var kv = ctx.resource.kv;
function handle(input) {
    var verb = input[0];
    if (verb === "set") { kv.set(input[1], input[2]); return "ok " + input[1]; }
    if (verb === "get") { var v = kv.get(input[1]); return v == null ? "(none)" : v; }
    if (verb === "rm")  { kv.rm(input[1]); return "rm " + input[1]; }
    if (verb === "all") {
        var a = kv.all(); var ks = [];
        for (var k in a) { ks.push(k + "=" + a[k]); }
        ks.sort();
        return ks.join(",");
    }
    if (verb === "raw") { kv.set("k", "v1"); kv.set("k", "v2"); return kv.get("k"); }
    if (verb === "setrm") { kv.set("z", "1"); kv.rm("z"); return "setrm"; }
    return "?";
}
"#;

/// Write a bundle dir (manifest + main.js) and return its path.
fn write_bundle(dir: &Path, name: &str, manifest: &str, backend: &str) -> String {
    let bundle = dir.join(name);
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("manifest.json"), manifest).unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

/// Install the `demo` app (kv-enabled) and return an open core.
fn install_demo(dir: &Path) -> Core {
    let src = write_bundle(
        dir,
        "demo",
        r#"{ "id": "demo", "name": "Demo", "backend": "main.js", "resources": ["kv"] }"#,
        BACKEND,
    );
    let mut core = Core::open(dir.join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo", "--source", &src]))
        .unwrap();
    core
}

#[test]
fn host_run_executes_js_records_kv_and_prints_output() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // A write produces exactly one kv.set record and the backend's printed string.
    let records = core.dispatch(req("host.run", &["demo", "set", "a", "1"])).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "kv.set");
    assert_eq!(core.take_last_output().as_deref(), Some("ok a"));
    assert_eq!(core.state().kv.data["demo"]["a"], "1");

    // Reads (no record).
    core.dispatch(req("host.run", &["demo", "get", "a"])).unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("1"));
    core.dispatch(req("host.run", &["demo", "get", "missing"])).unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("(none)"));

    // In-run read-after-write: two sets then a get inside ONE run see the latest.
    core.dispatch(req("host.run", &["demo", "raw"])).unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("v2"));

    // all() in a later run sees state committed by earlier runs.
    core.dispatch(req("host.run", &["demo", "all"])).unwrap();
    let all = core.take_last_output().unwrap();
    assert!(all.contains("a=1") && all.contains("k=v2"), "all: {all}");

    // Remove, then it's gone.
    core.dispatch(req("host.run", &["demo", "rm", "a"])).unwrap();
    assert!(!core.state().kv.data["demo"].contains_key("a"));

    // Option-A: replay rebuilds from kv.* alone; the log has no host.* record.
    assert!(core.replay_matches().unwrap());
    let lines = core.log_lines().unwrap();
    assert!(
        lines.iter().all(|l| !l.starts_with("host.")),
        "no host.* in log: {lines:?}"
    );
}

#[test]
fn host_run_rejects_missing_app_and_missing_source() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // Unknown app.
    assert!(core.dispatch(req("host.run", &["ghost", "all"])).is_err());

    // App with no --source bundle can't run.
    core.dispatch(req("app.add", &["bare", "Bare"])).unwrap();
    assert!(core.dispatch(req("host.run", &["bare", "all"])).is_err());
}

#[test]
fn undeclared_resource_is_not_installed() {
    let dir = tempdir().unwrap();
    // manifest declares no resources → ctx.resource.kv is absent (undefined).
    let src = write_bundle(
        dir.path(),
        "noperm",
        r#"{ "id": "noperm", "name": "NoPerm", "backend": "main.js", "resources": [] }"#,
        r#"function handle(input) { return ctx.resource.kv.get("x"); }"#,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["noperm", "NoPerm", "--source", &src]))
        .unwrap();
    // Touching an undeclared resource throws (kv is undefined) → the run errors.
    assert!(core.dispatch(req("host.run", &["noperm", "x"])).is_err());
}

#[test]
fn failed_run_clears_last_output() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // A successful run sets the output; we deliberately do NOT consume it.
    core.dispatch(req("host.run", &["demo", "set", "b", "2"])).unwrap();
    // A subsequent failed run must not leave the stale string behind.
    let _ = core.dispatch(req("host.run", &["ghost", "x"]));
    assert_eq!(core.take_last_output(), None, "stale output after a failed run");
}

#[test]
fn handle_must_return_a_string() {
    let dir = tempdir().unwrap();
    let src = write_bundle(
        dir.path(),
        "bad",
        r#"{ "id": "bad", "name": "Bad", "backend": "main.js", "resources": ["kv"] }"#,
        r#"function handle(input) { return 42; }"#,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["bad", "Bad", "--source", &src]))
        .unwrap();
    let result = core.dispatch(req("host.run", &["bad", "go"]));
    assert!(result.is_err(), "non-string handle() return should error");
}

#[test]
fn redundant_same_key_set_is_coalesced_within_a_run() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // `raw` sets "k" twice in one run → only the last set is committed.
    let records = core.dispatch(req("host.run", &["demo", "raw"])).unwrap();
    assert_eq!(records.len(), 1, "two sets of one key coalesce to one record");
    assert_eq!(records[0].kind, "kv.set");
    assert_eq!(core.state().kv.data["demo"]["k"], "v2");

    // The coalesced log still replays to the identical state.
    assert!(core.replay_matches().unwrap());
}

#[test]
fn set_then_rm_same_key_cancels_the_set() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // `setrm` sets "z" then removes it in one run → the set is superseded by the
    // later rm, so only the delete is committed.
    let records = core.dispatch(req("host.run", &["demo", "setrm"])).unwrap();
    assert_eq!(records.len(), 1, "the rm cancels the preceding same-key set");
    assert_eq!(records[0].kind, "kv.deleted");
    assert!(!core
        .state()
        .kv
        .data
        .get("demo")
        .map(|m| m.contains_key("z"))
        .unwrap_or(false));

    assert!(core.replay_matches().unwrap());
}

#[test]
fn kv_set_with_non_string_arg_gives_attributable_error() {
    let dir = tempdir().unwrap();
    let src = write_bundle(
        dir.path(),
        "typed",
        r#"{ "id": "typed", "name": "Typed", "backend": "main.js", "resources": ["kv"] }"#,
        r#"function handle(input) { ctx.resource.kv.set(1, "v"); return "ok"; }"#,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["typed", "Typed", "--source", &src]))
        .unwrap();

    // A non-string key aborts the run with a typed error naming the kv call —
    // not a generic rquickjs conversion message — and commits nothing.
    let err = core.dispatch(req("host.run", &["typed", "go"])).unwrap_err();
    match err {
        terrane_domain::Error::InvalidInput(msg) => {
            assert!(msg.contains("kv.set"), "error names the call: {msg}");
            assert!(msg.contains("key"), "error names the bad param: {msg}");
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn manifest_ignores_nested_backend_key() {
    let dir = tempdir().unwrap();
    // A nested object carries a decoy "backend"; only the TOP-LEVEL one counts.
    // A positional scan would pick "DECOY.js" (which does not exist) and fail.
    let src = write_bundle(
        dir.path(),
        "nested",
        r#"{ "id": "nested", "settings": { "backend": "DECOY.js" }, "backend": "main.js", "resources": ["kv"] }"#,
        BACKEND,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["nested", "Nested", "--source", &src]))
        .unwrap();

    core.dispatch(req("host.run", &["nested", "set", "a", "1"]))
        .unwrap();
    assert_eq!(core.state().kv.data["nested"]["a"], "1");
}

#[test]
fn manifest_decodes_string_escapes() {
    let dir = tempdir().unwrap();
    // The resource is written as the JSON escape `v` (built from char 92, a
    // backslash) which decodes to 'v' → "kv". A raw-substring scan would keep the
    // literal `kv` and fail to grant the kv resource, so the run would error.
    let bsl = char::from_u32(92).unwrap();
    let manifest =
        format!(r#"{{ "id": "esc", "backend": "main.js", "resources": ["k{bsl}u0076"] }}"#);
    let src = write_bundle(dir.path(), "esc", &manifest, BACKEND);
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["esc", "Esc", "--source", &src]))
        .unwrap();

    core.dispatch(req("host.run", &["esc", "set", "a", "1"]))
        .unwrap();
    assert_eq!(core.state().kv.data["esc"]["a"], "1");
}
