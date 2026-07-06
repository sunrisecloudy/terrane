//! e2e for `person` — drive the real binary and prove first-run person
//! creation stores only public facts in the log while replay rebuilds identity.

use std::fs;
use std::path::Path;
use std::process::Command;

use tempfile::tempdir;

fn terrane_file_secret_store(home: &Path, args: &[&str]) -> (bool, String, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(args)
        .env("TERRANE_HOME", home)
        .env("TERRANE_SECRET_STORE", "file")
        .output()
        .expect("spawn terrane");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn person_first_run_creates_replica_attested_identity_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, whoami, err) = terrane_file_secret_store(home, &["person", "whoami"]);
    assert!(ok, "person whoami failed: {err}");
    assert!(whoami.contains("\"person_id\""), "whoami: {whoami}");
    assert!(whoami.contains("\"kind\":\"replica\""), "whoami: {whoami}");
    assert!(!whoami.contains("ed25519"), "whoami leaked secret slot: {whoami}");
    let whoami_json: serde_json::Value = serde_json::from_str(&whoami).unwrap();
    let person_id = whoami_json
        .get("person_id")
        .and_then(serde_json::Value::as_str)
        .unwrap();
    let owner_subject = format!("user:{person_id}");

    let (ok, _out, err) = terrane_file_secret_store(home, &["app", "add", "after-person", "After Person"]);
    assert!(ok, "app add failed: {err}");

    let (ok, log, err) = terrane_file_secret_store(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("person.created"), "log: {log}");
    assert!(log.contains("person.attested"), "log: {log}");
    assert!(
        log.contains(&format!("{owner_subject} app.added after-person")),
        "log: {log}"
    );
    assert!(!log.contains("secrets.key"), "log leaked secret filename: {log}");

    let (ok, out, err) = terrane_file_secret_store(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay out: {out}");

    let second = tempdir().unwrap();
    fs::copy(home.join("log.bin"), second.path().join("log.bin")).unwrap();
    let (ok, copied_whoami, err) = terrane_file_secret_store(second.path(), &["person", "whoami"]);
    assert!(ok, "copied home whoami failed: {err}");
    assert_eq!(copied_whoami, whoami);
}
