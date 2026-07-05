//! e2e for `publish`: signed export/install, tamper rejection, and key-change stop.

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
fn publish_export_install_round_trip_records_provenance_and_replays() {
    let publisher = tempdir().unwrap();
    let receiver = tempdir().unwrap();
    let bundle = publisher.path().join("demo-v1");
    write_bundle(&bundle, "1.0.0", "first");

    let (ok, out, err) =
        terrane_file_secret_store(publisher.path(), &["app", "install-kv", path(&bundle)]);
    assert!(ok, "install-kv failed: {err}");
    assert!(out.contains("installed"), "out: {out}");

    let archive = publisher.path().join("demo-1.0.0.terrane");
    let (ok, out, err) = terrane_file_secret_store(
        publisher.path(),
        &["app", "export", "demo", "-o", path(&archive)],
    );
    assert!(ok, "export failed: {err}");
    assert!(out.contains("exported demo 1.0.0"), "out: {out}");
    assert!(archive.is_file());

    let (ok, out, err) =
        terrane_file_secret_store(receiver.path(), &["app", "install", path(&archive)]);
    assert!(ok, "signed install failed: {err}");
    assert!(out.contains("publish.trusted"), "out: {out}");
    assert!(out.contains("app.added"), "out: {out}");
    assert!(out.contains("publish.installed"), "out: {out}");

    let (ok, log, err) = terrane_file_secret_store(receiver.path(), &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("publish.trusted"), "log: {log}");
    assert!(log.contains("publish.installed demo 1.0.0"), "log: {log}");
    assert!(log.contains("kv.set demo/__terrane/app-bundle/main.js"), "log: {log}");
    assert!(!log.contains("ed25519"), "log leaked secret marker: {log}");

    let (ok, out, err) = terrane_file_secret_store(receiver.path(), &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "out: {out}");
}

#[test]
fn publish_install_rejects_tampered_archive_without_log_events() {
    let publisher = tempdir().unwrap();
    let receiver = tempdir().unwrap();
    let bundle = publisher.path().join("demo-v1");
    write_bundle(&bundle, "1.0.0", "first");
    assert!(terrane_file_secret_store(publisher.path(), &["app", "install-kv", path(&bundle)]).0);
    let archive = publisher.path().join("demo.terrane");
    assert!(terrane_file_secret_store(
        publisher.path(),
        &["app", "export", "demo", "-o", path(&archive)]
    )
    .0);

    let mut bytes = std::fs::read(&archive).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    let tampered = publisher.path().join("tampered.terrane");
    std::fs::write(&tampered, bytes).unwrap();

    let (ok, out, err) =
        terrane_file_secret_store(receiver.path(), &["app", "install", path(&tampered)]);
    assert!(!ok, "tampered install should fail: {out}");
    assert!(
        err.contains("signature")
            || err.contains("bundleHash")
            || err.contains("corrupt")
            || err.contains("publish.json"),
        "stderr: {err}"
    );
    let (ok, log, err) = terrane_file_secret_store(receiver.path(), &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(!log.contains("publish.installed"), "log: {log}");
    assert!(!log.contains("app.added demo"), "log: {log}");
}

#[test]
fn publish_install_stops_on_publisher_key_change_for_existing_app() {
    let publisher_a = tempdir().unwrap();
    let publisher_b = tempdir().unwrap();
    let receiver = tempdir().unwrap();
    let bundle_a = publisher_a.path().join("demo-v1");
    let bundle_b = publisher_b.path().join("demo-v2");
    write_bundle(&bundle_a, "1.0.0", "first");
    write_bundle(&bundle_b, "1.1.0", "second");
    assert!(terrane_file_secret_store(publisher_a.path(), &["app", "install-kv", path(&bundle_a)]).0);
    assert!(terrane_file_secret_store(publisher_b.path(), &["app", "install-kv", path(&bundle_b)]).0);

    let archive_a = publisher_a.path().join("demo-a.terrane");
    let archive_b = publisher_b.path().join("demo-b.terrane");
    assert!(terrane_file_secret_store(
        publisher_a.path(),
        &["app", "export", "demo", "-o", path(&archive_a)]
    )
    .0);
    assert!(terrane_file_secret_store(
        publisher_b.path(),
        &["app", "export", "demo", "-o", path(&archive_b)]
    )
    .0);
    assert!(terrane_file_secret_store(receiver.path(), &["app", "install", path(&archive_a)]).0);

    let (ok, out, err) =
        terrane_file_secret_store(receiver.path(), &["app", "install", path(&archive_b)]);
    assert!(!ok, "key-change install should fail: {out}");
    assert!(err.contains("publisher key changed"), "stderr: {err}");
}

fn write_bundle(bundle: &Path, version: &str, marker: &str) {
    std::fs::create_dir_all(bundle).unwrap();
    std::fs::write(
        bundle.join("manifest.json"),
        format!(
            r#"{{
  "id":"demo",
  "name":"Demo",
  "version":"{version}",
  "runtime":"js",
  "backend":"main.js",
  "resources":["kv"]
}}"#
        ),
    )
    .unwrap();
    std::fs::write(
        bundle.join("main.js"),
        format!(
            "function handle(input) {{ if (input[0] === '__actions__') return JSON.stringify({{actions:[{{verb:'common.receive'}},{{verb:'common.list'}},{{verb:'common.get'}}]}}); if (input[0] === 'common.receive') return '{{}}'; if (input[0] === 'common.list') return '[]'; if (input[0] === 'common.get') return JSON.stringify({{error:{{code:'NotFound',message:'item not found'}}}}); return '{marker}'; }}"
        ),
    )
    .unwrap();
}

fn path(path: &Path) -> &str {
    path.to_str().unwrap()
}
