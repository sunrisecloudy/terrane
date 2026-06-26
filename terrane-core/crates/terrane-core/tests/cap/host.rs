//! Engine tests for the `host` capability — running a JS backend in QuickJS over
//! a sandboxed app-scoped `ctx.resource.kv`. Uses a tiny inline bundle so the
//! test is self-contained (the full todo e2e lives in terrane-cli).

use std::fs;

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
    return "?";
}
"#;

fn install_demo(dir: &std::path::Path) -> (Core, String) {
    let bundle = dir.join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "demo", "name": "Demo", "backend": "main.js" }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), BACKEND).unwrap();

    let mut core = Core::open(dir.join("log.bin")).unwrap();
    core.dispatch(req(
        "app.add",
        &["demo", "Demo", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    (core, bundle.to_str().unwrap().to_string())
}

#[test]
fn host_run_executes_js_records_kv_and_prints_output() {
    let dir = tempdir().unwrap();
    let (mut core, _) = install_demo(dir.path());

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
    let (mut core, _) = install_demo(dir.path());

    // Unknown app.
    assert!(core.dispatch(req("host.run", &["ghost", "all"])).is_err());

    // App with no --source bundle can't run.
    core.dispatch(req("app.add", &["bare", "Bare"])).unwrap();
    assert!(core.dispatch(req("host.run", &["bare", "all"])).is_err());
}
