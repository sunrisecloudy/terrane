//! e2e smoke for `app` — the real binary parses, dispatches, and reports.
//! Logic detail (replay, cascade, validation) is covered by
//! `rust/crates/terrane-core/tests/cap/app.rs`.

use tempfile::tempdir;
use std::path::Path;

use crate::helpers::terrane;

#[test]
fn app_e2e_smoke() {
    let dir = tempdir().unwrap();
    let (ok, out, err) = terrane(dir.path(), &["app", "add", "notes", "Notes Lite"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("app.added"), "out: {out}");
}

#[test]
fn app_upgrade_e2e_replaces_bundle_runs_migration_and_archives_versions() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let v1 = home.join("demo-v1");
    let v2 = home.join("demo-v2");
    write_bundle(
        &v1,
        "1.0.0",
        1,
        &[],
        "function handle(input) { if (input[0] === '__actions__') return JSON.stringify({actions:[{verb:'common.receive'},{verb:'common.list'},{verb:'common.get'},{verb:'set-old',args:['value']}]}); if (input[0] === 'common.receive') return '{}'; if (input[0] === 'common.list') return '[]'; if (input[0] === 'common.get') return JSON.stringify({error:{code:'NotFound',message:'item not found'}}); if (input[0] === 'set-old') { ctx.resource.kv.set('old', input[1]); return 'old=' + input[1]; } return 'unknown'; }",
        &[("legacy.txt", "remove me")],
    );
    write_bundle(
        &v2,
        "1.1.0",
        2,
        &[("migrations/002.js", "function migrate(ctx) { const value = ctx.resource.kv.get('old'); ctx.resource.kv.set('title', value || ''); ctx.resource.kv.rm('old'); }")],
        "function handle(input) { if (input[0] === '__actions__') return JSON.stringify({actions:[{verb:'common.receive'},{verb:'common.list'},{verb:'common.get'},{verb:'get',args:[]}]}); if (input[0] === 'common.receive') return '{}'; if (input[0] === 'common.list') return '[]'; if (input[0] === 'common.get') return JSON.stringify({error:{code:'NotFound',message:'item not found'}}); if (input[0] === 'get') { return 'title=' + ctx.resource.kv.get('title'); } return 'unknown'; }",
        &[("fresh.txt", "new file")],
    );

    let (ok, out, err) = terrane(home, &["app", "install-kv", path(&v1)]);
    assert!(ok, "install failed: {err}");
    assert!(out.contains("installed"), "out: {out}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "demo", "kv"]);
    assert!(ok, "grant failed: {err}");
    let (ok, out, err) = terrane(home, &["run", "demo", "set-old", "milk"]);
    assert!(ok, "v1 run failed: {err}");
    assert_eq!(out.trim(), "old=milk");

    let (ok, out, err) = terrane(home, &["app", "upgrade", "demo", path(&v2)]);
    assert!(ok, "upgrade failed: {err}");
    assert!(out.contains("-> migration.applied"), "out: {out}");
    assert!(out.contains("-> app.upgraded"), "out: {out}");
    assert!(out.contains("-> blob.stored"), "out: {out}");
    assert!(out.contains("-> kv.deleted"), "out: {out}");
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("migration.applied demo 1 -> 2"), "log: {log}");
    assert!(log.contains("app.upgraded demo 1.0.0 -> 1.1.0"), "log: {log}");
    assert!(log.contains("blob.stored demo/__app__/demo/1.0.0"), "log: {log}");
    assert!(log.contains("blob.stored demo/__app__/demo/1.1.0"), "log: {log}");
    assert!(log.contains("kv.deleted demo/__terrane/app-bundle/legacy.txt"), "log: {log}");

    let (ok, out, err) = terrane(home, &["run", "demo", "get"]);
    assert!(ok, "v2 run failed: {err}");
    assert_eq!(out.trim(), "title=milk");
    let archive = home.join("v2.archive");
    let (ok, out, err) = terrane(
        home,
        &["blob", "get", "demo", "__app__/demo/1.1.0", path(&archive)],
    );
    assert!(ok, "blob get failed: {err}");
    assert!(out.contains("wrote"), "out: {out}");
    assert!(std::fs::metadata(&archive).unwrap().len() > 0);

    let (ok, out, err) = terrane(home, &["app", "upgrade", "demo", path(&v2)]);
    assert!(!ok, "same-version upgrade should fail: {out}");
    assert!(err.contains("already at 1.1.0"), "stderr: {err}");
    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "out: {out}");
}

#[test]
fn app_install_runs_optional_bundle_smoke_tests() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("smoke-pass");
    write_smoke_bundle(
        &bundle,
        "smoke-pass",
        r#"[
  {"verb":"echo","args":["hello"],"expect":{"contains":"Echo: hello"}},
  {"verb":"profile","args":["Ada"],"expect":{"jsonSubset":{"ok":true,"user":{"name":"Ada"}}}},
  {"verb":"profile","args":["Ada"],"expect":{"shape":{"ok":"boolean","user":{"name":"string","score":"number"}}}}
]"#,
    );

    let (ok, out, err) = terrane(home, &["app", "install", path(&bundle)]);

    assert!(ok, "install should accept passing tests.json: {err}");
    assert!(out.contains("installed"), "out: {out}");
}

#[test]
fn app_install_rejects_failing_bundle_smoke_test() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("smoke-fail");
    write_smoke_bundle(
        &bundle,
        "smoke-fail",
        r#"[{"verb":"echo","args":["hello"],"expect":{"contains":"goodbye"}}]"#,
    );

    let (ok, out, err) = terrane(home, &["app", "install", path(&bundle)]);

    assert!(!ok, "install should reject failing tests.json: {out}");
    assert!(err.contains("tests.json case 1 (echo) failed expectation"), "err: {err}");
    assert!(err.contains("goodbye"), "err: {err}");
}

fn write_bundle(
    bundle: &Path,
    version: &str,
    data_version: u64,
    migrations: &[(&str, &str)],
    backend: &str,
    extra_files: &[(&str, &str)],
) {
    std::fs::create_dir_all(bundle).unwrap();
    let migration_json = migrations
        .iter()
        .map(|(script, _)| format!(r#"{{"to":2,"script":"{}"}}"#, script))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{
  "id":"demo",
  "name":"Demo",
  "version":"{version}",
  "runtime":"js",
  "backend":"main.js",
  "resources":["kv"],
  "dataVersion":{data_version},
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
    for (file, content) in extra_files {
        std::fs::write(bundle.join(file), content).unwrap();
    }
}

fn write_smoke_bundle(bundle: &Path, id: &str, tests_json: &str) {
    std::fs::create_dir_all(bundle).unwrap();
    std::fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{
  "id":"{id}",
  "name":"Smoke Test",
  "runtime":"js",
  "backend":"main.js",
  "resources":[]
}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        bundle.join("main.js"),
        r#"
var actions = {
  echo: {
    run: function (args) {
      return "Echo: " + (args[0] || "");
    }
  },
  profile: {
    run: function (args) {
      return JSON.stringify({ ok: true, user: { name: args[0] || "", score: 7 }, extra: "kept" });
    }
  }
};
"#,
    )
    .unwrap();
    std::fs::write(bundle.join("tests.json"), tests_json).unwrap();
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
