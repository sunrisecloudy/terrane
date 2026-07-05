//! e2e tests for manifest-declared app data migrations.

use std::path::Path;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn migration_e2e_gates_stale_run_then_applies_manifest_step() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("todo");
    write_bundle(&bundle, 1, &[], backend_v1());

    let (ok, _, err) = terrane(
        home,
        &["app", "add", "todo", "Todo", "--source", path(&bundle)],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "todo", "kv"]);
    assert!(ok, "grant failed: {err}");

    let (ok, out, err) = terrane(home, &["run", "todo", "set-old", "milk"]);
    assert!(ok, "v1 run failed: {err}");
    assert_eq!(out.trim(), "old=milk");

    write_bundle(
        &bundle,
        2,
        &[("migrations/002.js", "function migrate(ctx) { const value = ctx.resource.kv.get('old'); ctx.resource.kv.set('title', value || ''); ctx.resource.kv.rm('old'); }")],
        backend_v2(),
    );

    let (ok, out, err) = terrane(home, &["run", "todo", "get"]);
    assert!(!ok, "stale run should fail: {out}");
    assert!(err.contains("run `terrane migrate todo`"), "stderr: {err}");

    let (ok, out, err) = terrane(home, &["migrate", "status", "todo"]);
    assert!(ok, "status failed: {err}");
    assert!(out.contains(r#""stateVersion":1"#), "out: {out}");
    assert!(out.contains(r#""manifestVersion":2"#), "out: {out}");

    let (ok, out, err) = terrane(home, &["migrate", "todo"]);
    assert!(ok, "migrate failed: {err}");
    assert!(out.contains("migrated todo 1 -> 2"), "out: {out}");

    let (ok, out, err) = terrane(home, &["run", "todo", "get"]);
    assert!(ok, "v2 run failed: {err}");
    assert_eq!(out.trim(), "title=milk");
}

#[test]
fn migration_e2e_resumes_after_mid_sequence_failure() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("todo");
    write_bundle(
        &bundle,
        3,
        &[
            ("migrations/002.js", "function migrate(ctx) { ctx.resource.kv.set('v2', 'done'); }"),
            ("migrations/003.js", "function migrate(ctx) { throw new Error('stop at v3'); }"),
        ],
        backend_v3(),
    );

    let (ok, _, err) = terrane(
        home,
        &["app", "add", "todo", "Todo", "--source", path(&bundle)],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "todo", "kv"]);
    assert!(ok, "grant failed: {err}");

    let (ok, out, err) = terrane(home, &["migrate", "todo"]);
    assert!(!ok, "second step should fail: {out}");
    assert!(err.contains("stop at v3"), "stderr: {err}");

    let (ok, out, err) = terrane(home, &["migrate", "status", "todo"]);
    assert!(ok, "status failed: {err}");
    assert!(out.contains(r#""stateVersion":2"#), "out: {out}");
    assert!(out.contains(r#""pending":[3]"#), "out: {out}");

    std::fs::write(
        bundle.join("migrations").join("003.js"),
        "function migrate(ctx) { ctx.resource.kv.set('v3', 'done'); }",
    )
    .unwrap();
    let (ok, out, err) = terrane(home, &["migrate", "todo"]);
    assert!(ok, "resume failed: {err}");
    assert!(out.contains("migrated todo 2 -> 3"), "out: {out}");

    let (ok, out, err) = terrane(home, &["run", "todo", "check"]);
    assert!(ok, "v3 run failed: {err}");
    assert_eq!(out.trim(), "v2=done;v3=done");
}

fn write_bundle(
    bundle: &Path,
    version: u64,
    migrations: &[(&str, &str)],
    backend: &str,
) {
    std::fs::create_dir_all(bundle.join("migrations")).unwrap();
    let migration_json = migrations
        .iter()
        .enumerate()
        .map(|(i, (script, _))| format!(r#"{{"to":{},"script":"{}"}}"#, i + 2, script))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{
  "id":"todo",
  "name":"Todo",
  "runtime":"js",
  "backend":"main.js",
  "resources":["kv"],
  "dataVersion":{version},
  "migrations":[{migration_json}]
}}"#
        ),
    )
    .unwrap();
    std::fs::write(bundle.join("main.js"), backend).unwrap();
    for (script, source) in migrations {
        let path = bundle.join(script);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, source).unwrap();
    }
}

fn backend_v1() -> &'static str {
    "function handle(input) { if (input[0] === 'set-old') { ctx.resource.kv.set('old', input[1]); return 'old=' + input[1]; } return 'unknown'; }"
}

fn backend_v2() -> &'static str {
    "function handle(input) { if (input[0] === 'get') { return 'title=' + (ctx.resource.kv.get('title') || ''); } return 'unknown'; }"
}

fn backend_v3() -> &'static str {
    "function handle(input) { if (input[0] === 'check') { return 'v2=' + ctx.resource.kv.get('v2') + ';v3=' + ctx.resource.kv.get('v3'); } return 'unknown'; }"
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
