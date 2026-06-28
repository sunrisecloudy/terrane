//! Engine tests for the `host` capability — running a JS backend in QuickJS over
//! a sandboxed app-scoped `ctx.resource.kv`. Uses tiny inline bundles so the
//! tests are self-contained (the full todo e2e lives in terrane-host).

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_core::cap::app::AppRecord;
use terrane_core::cap::host::{run_memory_backend, MemoryBackendBundle};
use terrane_core::{fold_records_in_memory, Core, State};

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
    let records = core
        .dispatch(req("host.run", &["demo", "set", "a", "1"]))
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "kv.set");
    assert_eq!(core.take_last_output().as_deref(), Some("ok a"));
    assert_eq!(core.state().kv.data["demo"]["a"], "1");

    // Reads (no record).
    core.dispatch(req("host.run", &["demo", "get", "a"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("1"));
    core.dispatch(req("host.run", &["demo", "get", "missing"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("(none)"));

    // In-run read-after-write: two sets then a get inside ONE run see the latest.
    core.dispatch(req("host.run", &["demo", "raw"])).unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("v2"));

    // all() in a later run sees state committed by earlier runs.
    core.dispatch(req("host.run", &["demo", "all"])).unwrap();
    let all = core.take_last_output().unwrap();
    assert!(all.contains("a=1") && all.contains("k=v2"), "all: {all}");

    // Remove, then it's gone.
    core.dispatch(req("host.run", &["demo", "rm", "a"]))
        .unwrap();
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
fn build_resource_compiles_typescript_inside_quickjs() {
    let dir = tempdir().unwrap();
    let backend = r#"
        var build = ctx.resource.build;
        function handle(input) {
            var result = JSON.parse(build.compileTs("main.ts", "const value: number = 1; export const next = value + 1;"));
            if (!result.ok) return result.error;
            return result.code.indexOf("const value = 1") >= 0 &&
                   result.code.indexOf("export const next = value + 1") >= 0 &&
                   result.code.indexOf(": number") < 0 ? "compiled" : result.code;
        }
    "#;
    let src = write_bundle(
        dir.path(),
        "build-demo",
        r#"{ "id": "build-demo", "name": "Build Demo", "backend": "main.js", "resources": ["build"] }"#,
        backend,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req(
        "app.add",
        &["build-demo", "Build Demo", "--source", &src],
    ))
    .unwrap();

    let records = core.dispatch(req("host.run", &["build-demo"])).unwrap();

    assert!(records.is_empty());
    assert_eq!(core.take_last_output().as_deref(), Some("compiled"));
}

#[test]
fn backend_cannot_use_eval_or_function_constructor() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return String(typeof eval) + "," + String(typeof Function);
        }
    "#;
    let src = write_bundle(
        dir.path(),
        "no-eval",
        r#"{ "id": "no-eval", "name": "No Eval", "backend": "main.js", "resources": [] }"#,
        backend,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["no-eval", "No Eval", "--source", &src]))
        .unwrap();

    core.dispatch(req("host.run", &["no-eval"])).unwrap();

    assert_eq!(
        core.take_last_output().as_deref(),
        Some("undefined,undefined")
    );
}

#[test]
fn failed_run_clears_last_output() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // A successful run sets the output; we deliberately do NOT consume it.
    core.dispatch(req("host.run", &["demo", "set", "b", "2"]))
        .unwrap();
    // A subsequent failed run must not leave the stale string behind.
    let _ = core.dispatch(req("host.run", &["ghost", "x"]));
    assert_eq!(
        core.take_last_output(),
        None,
        "stale output after a failed run"
    );
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

/// A backend that declares an `actions` table (no hand-written `handle`): the
/// runtime synthesizes dispatch, `__actions__`, usage, and the unknown-verb help.
const ACTIONS_BACKEND: &str = r#"
var kv = ctx.resource.kv;
var description = "demo actions app";
var actions = {
  set: {
    summary: "Store the greeting.",
    args: [{ name: "value", required: true }],
    run: function (args, usage) {
      if (args.length === 0) return usage();
      kv.set("greeting", args.join(" "));
      return "ok";
    }
  },
  get: {
    summary: "Read the greeting.",
    args: [],
    run: function () { var v = kv.get("greeting"); return v == null ? "(none)" : v; }
  }
};
"#;

#[test]
fn actions_table_backend_is_synthesized_and_self_describes() {
    let dir = tempdir().unwrap();
    let src = write_bundle(
        dir.path(),
        "acts",
        r#"{ "id": "acts", "name": "Acts Demo", "backend": "main.js", "resources": ["kv"] }"#,
        ACTIONS_BACKEND,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["acts", "Acts Demo", "--source", &src]))
        .unwrap();

    // Dispatch works with no hand-written handle; reads see prior writes.
    core.dispatch(req("host.run", &["acts", "set", "hi", "there"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    core.dispatch(req("host.run", &["acts", "get"])).unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("hi there"));

    // usage() is derived from the action's declared args.
    core.dispatch(req("host.run", &["acts", "set"])).unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("usage: set <value>")
    );

    // The unknown-verb help lists the table's keys.
    core.dispatch(req("host.run", &["acts", "frob"])).unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("unknown verb: frob (try set | get)")
    );

    // __actions__ self-describes, with app id/name pulled from the manifest.
    core.dispatch(req("host.run", &["acts", "__actions__"]))
        .unwrap();
    let out = core.take_last_output().unwrap();
    assert!(out.contains("\"app\":\"acts\""), "id from manifest: {out}");
    assert!(
        out.contains("\"title\":\"Acts Demo\""),
        "name from manifest: {out}"
    );
    assert!(out.contains("demo actions app"), "description: {out}");
    assert!(
        out.contains("\"verb\":\"set\"") && out.contains("\"verb\":\"get\""),
        "verbs: {out}"
    );

    // Still Option-A: only the kv.* writes were recorded; replay rebuilds it.
    assert!(core.replay_matches().unwrap());
}

#[test]
fn actions_table_accepts_direct_function_entries() {
    let dir = tempdir().unwrap();
    let src = write_bundle(
        dir.path(),
        "counter",
        r#"{ "id": "counter", "name": "Counter", "backend": "main.js", "resources": ["kv"] }"#,
        r#"
var kv = ctx.resource.kv;
function readCount() {
  var raw = kv.get("count");
  var count = parseInt(String(raw == null ? "0" : raw), 10);
  return isNaN(count) ? 0 : count;
}
function writeCount(count) {
  kv.set("count", String(count));
  return String(count);
}
var actions = {
  get: function () { return String(readCount()); },
  increment: function () { return writeCount(readCount() + 1); },
  reset: function () { return writeCount(0); }
};
"#,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["counter", "Counter", "--source", &src]))
        .unwrap();

    core.dispatch(req("host.run", &["counter", "reset"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("0"));
    core.dispatch(req("host.run", &["counter", "increment"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("1"));
    core.dispatch(req("host.run", &["counter", "get"])).unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("1"));

    assert!(core.replay_matches().unwrap());
}

#[test]
fn redundant_same_key_set_is_coalesced_within_a_run() {
    let dir = tempdir().unwrap();
    let mut core = install_demo(dir.path());

    // `raw` sets "k" twice in one run → only the last set is committed.
    let records = core.dispatch(req("host.run", &["demo", "raw"])).unwrap();
    assert_eq!(
        records.len(),
        1,
        "two sets of one key coalesce to one record"
    );
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
    assert_eq!(
        records.len(),
        1,
        "the rm cancels the preceding same-key set"
    );
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
    let err = core
        .dispatch(req("host.run", &["typed", "go"]))
        .unwrap_err();
    match err {
        terrane_core::Error::InvalidInput(msg) => {
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

/// The live runtime installs EXACTLY the resource surface the capabilities
/// declare via `resource_api()` — no more, no less. Catches a bug in the
/// declaration-driven install loop in `cap/host.rs`.
#[test]
fn runtime_resource_surface_matches_declarations() {
    let declared = terrane_core::declared_resource_surface();
    assert!(!declared.is_empty(), "kv should declare a resource surface");

    // Grant exactly the declared namespaces, then introspect the LIVE ctx.resource.
    let namespaces: BTreeSet<&str> = declared
        .iter()
        .filter_map(|m| m.split('.').nth(2))
        .collect();
    let resources_json = namespaces
        .iter()
        .map(|n| format!("\"{n}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let introspect = r#"
        function handle(input) {
            var out = [];
            var nss = Object.keys(ctx.resource).sort();
            for (var i = 0; i < nss.length; i++) {
                var ns = nss[i];
                var ms = Object.keys(ctx.resource[ns]).sort();
                for (var j = 0; j < ms.length; j++) {
                    out.push("ctx.resource." + ns + "." + ms[j]);
                }
            }
            return out.join("\n");
        }
    "#;
    let dir = tempdir().unwrap();
    let src = write_bundle(
        dir.path(),
        "introspect",
        &format!(
            r#"{{ "id": "introspect", "name": "Introspect", "backend": "main.js", "resources": [{resources_json}] }}"#
        ),
        introspect,
    );
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req(
        "app.add",
        &["introspect", "Introspect", "--source", &src],
    ))
    .unwrap();
    core.dispatch(req("host.run", &["introspect", "list"]))
        .unwrap();
    let runtime: BTreeSet<String> = core
        .take_last_output()
        .unwrap_or_default()
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    assert_eq!(
        declared,
        runtime,
        "runtime ctx.resource differs from the declared surface.\n\
         declared-only: {:?}\n runtime-only: {:?}",
        declared.difference(&runtime).collect::<Vec<_>>(),
        runtime.difference(&declared).collect::<Vec<_>>(),
    );
}

#[test]
fn memory_backend_run_returns_records_for_caller_owned_fold() {
    let mut state = State::default();
    state.app.apps.insert(
        "preview-demo".to_string(),
        AppRecord {
            id: "preview-demo".to_string(),
            name: "Preview Demo".to_string(),
            source: None,
        },
    );
    let bundle = MemoryBackendBundle {
        source: BACKEND.to_string(),
        name: "Preview Demo".to_string(),
        resources: vec!["kv".to_string()],
    };

    let result = run_memory_backend(
        "preview-demo",
        &["set".to_string(), "a".to_string(), "1".to_string()],
        &bundle,
        state.clone(),
    )
    .unwrap();

    assert_eq!(result.output, "ok a");
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].kind, "kv.set");
    assert!(!state.kv.data.contains_key("preview-demo"));

    fold_records_in_memory(&mut state, &result.records).unwrap();
    assert_eq!(state.kv.data["preview-demo"]["a"], "1");
}

/// The generated `ctx.resource` section between the markers in `docs/APP_API.md`
/// matches the generator. Change a capability's `resource_api()` and forget to
/// regenerate → this fails. Regenerate with `UPDATE_DOCS=1 cargo test`.
#[test]
fn app_api_doc_resource_section_is_generated() {
    const START: &str = "<!-- generated:resource-api:start -->";
    const END: &str = "<!-- generated:resource-api:end -->";
    let doc_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../docs/APP_API.md");
    let doc = fs::read_to_string(&doc_path).expect("docs/APP_API.md exists");

    let start = doc.find(START).expect("resource-api start marker") + START.len();
    let end = doc.find(END).expect("resource-api end marker");
    assert!(start <= end, "resource-api markers out of order");
    let generated = terrane_core::resource_api_markdown();

    if std::env::var_os("UPDATE_DOCS").is_some() {
        let new_doc = format!("{}\n{generated}\n{}", &doc[..start], &doc[end..]);
        fs::write(&doc_path, new_doc).expect("rewrite docs/APP_API.md");
        return;
    }

    // Compare CONTENT, not exact formatting: collapse whitespace and drop table
    // separator rows, so a markdown formatter reflowing the table can't break the
    // test, while any real change (a method, signature, or kind) still does.
    assert_eq!(
        normalize_md(&doc[start..end]),
        normalize_md(&generated),
        "docs/APP_API.md resource section is stale — regenerate with \
         `UPDATE_DOCS=1 cargo test -p terrane-core --test cap app_api_doc`"
    );
}

/// Normalize markdown for content comparison: trim + collapse intra-line
/// whitespace, drop blank lines and table separator rows (`| --- | --- |`).
fn normalize_md(md: &str) -> Vec<String> {
    md.lines()
        .map(|l| l.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|l| !l.is_empty())
        .filter(|l| !l.chars().all(|c| matches!(c, '|' | '-' | ':' | ' ')))
        .collect()
}
