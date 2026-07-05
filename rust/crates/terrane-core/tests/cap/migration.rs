//! Engine tests for the `migration` capability.

use tempfile::tempdir;
use terrane_core::{Core, Error};

use crate::helpers::{grant_resource, public_req, req};

#[test]
fn migration_apply_records_data_and_version_in_one_replayable_batch() {
    let dir = tempdir().unwrap();
    let bundle = write_bundle(dir.path(), 2);
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["todo", "Todo", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    grant_resource(&mut core, "todo", "kv");

    let script = "function migrate(ctx) { ctx.resource.kv.set('title', 'after'); return 'ok'; }";
    let records = core
        .dispatch(req("migration.apply", &["todo", "2", script]))
        .unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].kind, "kv.set");
    assert_eq!(records[1].kind, "migration.applied");
    assert_eq!(core.state().kv.data["todo"]["title"], "after");
    assert_eq!(core.state().migration.apps["todo"].version, 2);
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn throwing_migration_commits_no_data_or_version() {
    let dir = tempdir().unwrap();
    let bundle = write_bundle(dir.path(), 2);
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["todo", "Todo", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    grant_resource(&mut core, "todo", "kv");

    let before = core.log_lines().unwrap().len();
    let script = "function migrate(ctx) { ctx.resource.kv.set('title', 'partial'); throw new Error('boom'); }";
    assert!(matches!(
        core.dispatch(req("migration.apply", &["todo", "2", script])),
        Err(Error::Runtime(_))
    ));

    assert_eq!(core.log_lines().unwrap().len(), before);
    assert!(!core.state().kv.data.contains_key("todo"));
    assert!(!core.state().migration.apps.contains_key("todo"));
}

#[test]
fn migration_apply_refuses_public_gap_and_downgrade_paths() {
    let dir = tempdir().unwrap();
    let bundle = write_bundle(dir.path(), 2);
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["todo", "Todo", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    grant_resource(&mut core, "todo", "kv");

    assert_eq!(
        core.dispatch(public_req("migration.apply", &["todo", "2", "function migrate() {}"])),
        Err(Error::InvalidInput(
            "migration.apply requires trusted host authority".into()
        ))
    );
    assert!(matches!(
        core.dispatch(req("migration.apply", &["todo", "3", "function migrate() {}"])),
        Err(Error::InvalidInput(message)) if message.contains("consecutive")
    ));

    core.dispatch(req(
        "migration.apply",
        &["todo", "2", "function migrate(ctx) {}"],
    ))
    .unwrap();
    assert!(matches!(
        core.dispatch(req("migration.apply", &["todo", "2", "function migrate() {}"])),
        Err(Error::InvalidInput(message)) if message.contains("consecutive")
    ));
}

#[test]
fn migration_state_drops_on_app_removed() {
    let dir = tempdir().unwrap();
    let bundle = write_bundle(dir.path(), 2);
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["todo", "Todo", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    grant_resource(&mut core, "todo", "kv");
    core.dispatch(req(
        "migration.apply",
        &["todo", "2", "function migrate(ctx) {}"],
    ))
    .unwrap();

    core.dispatch(req("app.remove", &["todo"])).unwrap();

    assert!(!core.state().migration.apps.contains_key("todo"));
    assert!(core.replay_matches().unwrap());
}

fn write_bundle(root: &std::path::Path, version: u64) -> std::path::PathBuf {
    let bundle = root.join("todo");
    std::fs::create_dir_all(bundle.join("migrations")).unwrap();
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
  "migrations":[{{"to":2,"script":"migrations/002.js"}}]
}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        bundle.join("main.js"),
        "function handle(input) { return 'ok'; }",
    )
    .unwrap();
    std::fs::write(
        bundle.join("migrations").join("002.js"),
        "function migrate(ctx) {}",
    )
    .unwrap();
    bundle
}
