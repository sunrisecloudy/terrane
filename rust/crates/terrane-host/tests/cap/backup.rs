use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn backup_create_restore_preserves_replay_state() {
    let src_dir = tempdir().unwrap();
    let dst_dir = tempdir().unwrap();
    let archive_dir = tempdir().unwrap();
    let source = src_dir.path();
    let target = dst_dir.path().join("restored");
    let archive = archive_dir.path().join("home.tzst");

    let (ok, _, err) = terrane(source, &["app", "add", "notes", "Notes"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(source, &["kv", "set", "notes", "theme", "dark"]);
    assert!(ok, "kv set failed: {err}");

    let (ok, out, err) = terrane(
        source,
        &["backup", "create", archive.to_str().unwrap()],
    );
    assert!(ok, "backup create failed: {err}");
    assert!(out.contains("backup created"), "out: {out}");

    let (ok, out, err) = terrane(
        source,
        &[
            "backup",
            "restore",
            archive.to_str().unwrap(),
            "--into",
            target.to_str().unwrap(),
        ],
    );
    assert!(ok, "backup restore failed: {err}");
    assert!(out.contains("replay_matches=true"), "out: {out}");
    assert!(out.contains("mode=restore"), "out: {out}");

    let source_core = terrane_host::open_at_home(source).unwrap();
    let restored_core = terrane_host::open_at_home(&target).unwrap();
    assert_eq!(source_core.state(), restored_core.state());
    assert!(restored_core.replay_matches().unwrap());
}

#[test]
fn backup_restore_refuses_nonempty_target() {
    let src_dir = tempdir().unwrap();
    let dst_dir = tempdir().unwrap();
    let archive_dir = tempdir().unwrap();
    let source = src_dir.path();
    let target = dst_dir.path().join("nonempty");
    let archive = archive_dir.path().join("home.tzst");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("keep.txt"), "occupied").unwrap();

    assert!(terrane(source, &["app", "add", "notes", "Notes"]).0);
    assert!(
        terrane(source, &["backup", "create", archive.to_str().unwrap()]).0,
        "backup create"
    );
    let (ok, out, err) = terrane(
        source,
        &[
            "backup",
            "restore",
            archive.to_str().unwrap(),
            "--into",
            target.to_str().unwrap(),
        ],
    );
    assert!(!ok, "restore should fail: {out}");
    assert!(err.contains("not empty"), "err: {err}");
}

#[test]
fn backup_restore_tamper_is_rejected() {
    let src_dir = tempdir().unwrap();
    let dst_dir = tempdir().unwrap();
    let archive_dir = tempdir().unwrap();
    let source = src_dir.path();
    let target = dst_dir.path().join("restored");
    let archive = archive_dir.path().join("home.tzst");

    assert!(terrane(source, &["app", "add", "notes", "Notes"]).0);
    assert!(terrane(source, &["backup", "create", archive.to_str().unwrap()]).0);
    let mut bytes = fs::read(&archive).unwrap();
    let idx = bytes.len() / 2;
    bytes[idx] ^= 0x55;
    fs::write(&archive, bytes).unwrap();

    let (ok, out, err) = terrane(
        source,
        &[
            "backup",
            "restore",
            archive.to_str().unwrap(),
            "--into",
            target.to_str().unwrap(),
        ],
    );
    assert!(!ok, "tampered restore should fail: {out}");
    assert!(!err.is_empty(), "tampered restore should explain failure");
}

#[test]
fn backup_restore_clone_rotates_peer() {
    let src_dir = tempdir().unwrap();
    let dst_dir = tempdir().unwrap();
    let archive_dir = tempdir().unwrap();
    let source = src_dir.path();
    let target = dst_dir.path().join("clone");
    let archive = archive_dir.path().join("home.tzst");

    assert!(terrane(source, &["app", "add", "notes", "Notes"]).0);
    assert!(terrane(source, &["backup", "create", archive.to_str().unwrap()]).0);
    let source_peer = terrane_core::Core::<terrane_core::NoEffects>::open(source.join("log.bin"))
        .unwrap()
        .state()
        .replica
        .peer;

    let (ok, out, err) = terrane(
        source,
        &[
            "backup",
            "restore",
            archive.to_str().unwrap(),
            "--into",
            target.to_str().unwrap(),
            "--clone",
        ],
    );
    assert!(ok, "clone restore failed: {err}");
    assert!(out.contains("mode=clone"), "out: {out}");
    let clone_peer = terrane_core::Core::<terrane_core::NoEffects>::open(target.join("log.bin"))
        .unwrap()
        .state()
        .replica
        .peer;
    assert_ne!(source_peer, clone_peer);
}

#[test]
fn export_import_round_trips_one_app_and_refuses_existing_id() {
    let src_dir = tempdir().unwrap();
    let dst_dir = tempdir().unwrap();
    let archive_dir = tempdir().unwrap();
    let source = src_dir.path();
    let target = dst_dir.path();
    let archive = archive_dir.path().join("notes.tzst");

    assert!(terrane(source, &["app", "add", "notes", "Notes"]).0);
    assert!(terrane(source, &["app", "add", "other", "Other"]).0);
    assert!(terrane(source, &["kv", "set", "notes", "theme", "dark"]).0);
    assert!(terrane(source, &["kv", "set", "other", "theme", "light"]).0);

    let (ok, out, err) = terrane(
        source,
        &["export", "notes", archive.to_str().unwrap()],
    );
    assert!(ok, "export failed: {err}");
    assert!(out.contains("exported notes"), "out: {out}");

    let (ok, out, err) = terrane(target, &["import", archive.to_str().unwrap()]);
    assert!(ok, "import failed: {err}");
    assert!(out.contains("imported notes"), "out: {out}");
    let imported = terrane_host::open_at_home(target).unwrap();
    assert!(imported.state().app.apps.contains_key("notes"));
    assert!(!imported.state().app.apps.contains_key("other"));
    assert_eq!(
        imported
            .state()
            .kv
            .data
            .get("notes")
            .and_then(|kv| kv.get("theme")),
        Some(&"dark".to_string())
    );
    drop(imported);

    let (ok, out, err) = terrane(target, &["import", archive.to_str().unwrap()]);
    assert!(!ok, "second import should fail: {out}");
    assert!(err.contains("app already exists"), "err: {err}");
}
