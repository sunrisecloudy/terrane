//! e2e checks for the `telemetry` capability over the real host edge:
//! sandbox console logging, exception capture, CLI tailing, and ring rotation.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::tempdir;

use crate::helpers::terrane;

fn write_bundle(home: &Path, id: &str, resources: &str, backend: &str) -> PathBuf {
    let dir = home.join(format!("{id}-bundle"));
    fs::create_dir(&dir).unwrap();
    fs::write(
        dir.join("manifest.json"),
        format!(
            r#"{{"id":"{id}","name":"{id}","runtime":"js","backend":"main.js","resources":{resources}}}"#
        ),
    )
    .unwrap();
    fs::write(dir.join("main.js"), backend).unwrap();
    dir
}

fn add_bundle(home: &Path, id: &str, resources: &str, backend: &str) {
    let source = write_bundle(home, id, resources, backend);
    let (ok, out, err) = terrane(
        home,
        &[
            "app",
            "add",
            id,
            id,
            "--source",
            source.to_str().unwrap(),
        ],
    );
    assert!(ok && out.contains("app.added"), "app add: {out} {err}");
}

#[test]
fn console_log_writes_local_buffer_and_cli_tails_it_without_grant() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    add_bundle(
        home,
        "console-app",
        "[]",
        r#"
        function handle(input) {
            console.log("started", {verb: input[0]});
            console.warn("careful");
            return "ok";
        }
        "#,
    );

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "console-app", "go"]);
    assert!(ok, "run failed: {err}");
    assert_eq!(out.trim(), "ok");

    let (ok, out, err) = terrane(home, &["logs", "console-app", "--tail", "10"]);
    assert!(ok, "logs failed: {err}");
    assert!(out.contains(r#""level":"info""#), "logs: {out}");
    assert!(out.contains("started"), "logs: {out}");
    assert!(out.contains(r#""level":"warn""#), "logs: {out}");

    let (ok, out, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        !out.contains("telemetry.error"),
        "console info/warn should not record events: {out}"
    );
}

#[test]
fn thrown_exception_writes_buffer_and_records_error_fact_when_granted() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    add_bundle(
        home,
        "crashy",
        r#"["telemetry"]"#,
        r#"
        function handle(input) {
            throw new Error("kaput " + input[0]);
        }
        "#,
    );
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "crashy", "telemetry"],
    );
    assert!(ok, "grant failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "crashy", "boom"]);
    assert!(!ok, "run should fail: {out}");
    assert!(err.contains("kaput boom"), "stderr: {err}");

    let (ok, out, err) = terrane(home, &["logs", "crashy", "--level", "error", "--tail", "5"]);
    assert!(ok, "logs failed: {err}");
    assert!(out.contains(r#""level":"error""#), "logs: {out}");
    assert!(out.contains("kaput boom"), "logs: {out}");

    let (ok, out, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(out.contains("telemetry.error crashy source=exception"), "log: {out}");
}

#[test]
fn app_remove_prunes_log_buffer_directory() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    add_bundle(
        home,
        "gone",
        "[]",
        r#"function handle(input) { console.log("before remove"); return "ok"; }"#,
    );
    let (ok, _, err) = terrane(home, &["js-runtime", "run", "gone"]);
    assert!(ok, "run failed: {err}");
    assert!(home.join("logs").join("gone").exists());

    let (ok, out, err) = terrane(home, &["app", "remove", "gone"]);
    assert!(ok && out.contains("app.removed"), "remove: {out} {err}");
    assert!(!home.join("logs").join("gone").exists());
}

#[test]
fn app_log_rotates_at_cap() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let big = "x".repeat((terrane_cap_telemetry::RING_ROTATE_BYTES as usize) + 1);

    terrane_host::app_log::append(home, "rotator", "info", &big, "{}").unwrap();
    terrane_host::app_log::append(home, "rotator", "info", "after", "{}").unwrap();

    let app_dir = home.join("logs").join("rotator");
    assert!(app_dir.join("1.jsonl").is_file());
    assert!(app_dir.join("current.jsonl").is_file());
    let tail = terrane_host::app_log::read_tail(home, "rotator", "info", 1).unwrap();
    assert!(tail.contains("after"), "tail: {tail}");
}
