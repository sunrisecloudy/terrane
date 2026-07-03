//! e2e for `sysinfo` — live host metrics through a real app backend.
//!
//! Reads sample the edge and record nothing, so the event log stays free of
//! sysinfo.* events and replay is trivial.

use std::path::PathBuf;

use serde_json::Value;
use tempfile::tempdir;

use crate::helpers::terrane;

fn app_source(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps")
        .join(name)
        .canonicalize()
        .unwrap_or_else(|_| panic!("apps/{name} bundle exists"))
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn os_monitor_snapshot_returns_live_metrics_without_logging_reads() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("os-monitor");

    let (ok, out, err) = terrane(
        home,
        &["app", "add", "os-monitor", "OS Monitor", "--source", &src],
    );
    assert!(ok, "app add failed: {err}");
    assert!(out.contains("app.added"), "out: {out}");

    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "os-monitor", "sysinfo"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "os-monitor", "snapshot"]);
    assert!(ok, "snapshot failed: {err}");
    let value: Value = serde_json::from_str(out.trim()).expect("snapshot should be JSON");

    for section in [
        "cpu",
        "memory",
        "disk",
        "network",
        "battery",
        "system",
        "processes",
    ] {
        assert!(value.get(section).is_some(), "snapshot missing {section}");
    }
    assert!(
        value["memory"]["total"].as_u64().unwrap_or(0) > 0,
        "memory.total should be positive"
    );
    assert!(
        value["cpu"]["usage"].is_number(),
        "cpu.usage should be a number"
    );

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        !log.contains("sysinfo."),
        "live reads must not append sysinfo events: {log}"
    );

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "out: {out}");
}

#[test]
fn os_monitor_without_grant_reports_ungranted() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("os-monitor");

    terrane(
        home,
        &["app", "add", "os-monitor", "OS Monitor", "--source", &src],
    );
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "os-monitor", "cpu"]);
    assert!(ok, "backend should return a string even when ungranted: {err}");
    assert!(
        out.contains("sysinfo not granted"),
        "expected grant hint, got: {out}"
    );
}