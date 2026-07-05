use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn compact_cli_snapshots_tail_and_home_remains_usable() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    assert!(terrane(home, &["app", "add", "demo", "Demo"]).0);
    assert!(terrane(home, &["kv", "set", "demo", "a", "one"]).0);
    assert!(terrane(home, &["kv", "set", "demo", "b", "two"]).0);

    let (ok, stdout, stderr) = terrane(home, &["compact", "--retain", "1", "--verify"]);
    assert!(ok, "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("compacted"));
    assert!(home.join("snapshot.bin").exists());
    assert!(home.join("log.bin.archive").exists());
    assert_eq!(terrane_core::read_log(&home.join("log.bin")).unwrap().len(), 1);

    let (ok, stdout, stderr) = terrane(home, &["replay"]);
    assert!(ok, "stdout={stdout}\nstderr={stderr}");
    assert!(stdout.contains("replay ok"));
    assert!(terrane(home, &["kv", "set", "demo", "c", "three"]).0);
}

#[test]
fn compact_cli_rejects_bad_retain_value() {
    let dir = tempdir().unwrap();
    let (ok, _stdout, stderr) = terrane(dir.path(), &["compact", "--retain", "nope"]);
    assert!(!ok);
    assert!(stderr.contains("--retain must be a non-negative integer"));
}
