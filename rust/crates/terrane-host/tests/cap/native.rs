//! e2e smoke for `native`. Real OS UI work stays in connector tests; this
//! drives the trusted CLI command path and log descriptions.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn native_cli_records_request_and_trusted_completion() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");
    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "platform.observe",
            "local",
            "macos",
            "test-1",
            "external.openUrl",
        ],
    );
    assert!(ok, "platform observe failed: {err}");
    assert!(out.contains("native.platform.observed"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "external.open-url",
            "demo",
            "req-1",
            "https://example.com",
        ],
    );
    assert!(ok, "native request failed: {err}");
    assert!(out.contains("native.requested"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &["native", "complete", "demo", "req-1", r#"{"ok":true}"#],
    );
    assert!(ok, "native complete failed: {err}");
    assert!(out.contains("native.completed"), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        log.contains("native.platform.observed local macos"),
        "log: {log}"
    );
    assert!(
        log.contains("native.requested demo req-1 external.openUrl -> local"),
        "log: {log}"
    );
    assert!(log.contains("native.completed demo req-1"), "log: {log}");
}

#[test]
fn native_cli_exposes_explicit_host_drain_service() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(home, &["native", "observe-default"]);
    assert!(ok, "observe default failed: {err}");
    assert!(out.contains("native.platform.observed"), "out: {out}");

    let (ok, out, err) = terrane(home, &["native", "drain-once"]);
    assert!(ok, "drain once failed: {err}");
    assert_eq!(out.trim(), "native drain idle");
}

#[test]
fn native_cli_records_v2_requests_and_stub_completion() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "native",
            "platform.observe",
            "local",
            "macos",
            "test-1",
            "clipboard.readText",
            "screen.capture",
            "tray.setMenu",
        ],
    );
    assert!(ok, "platform observe failed: {err}");

    let (ok, out, err) = terrane(home, &["native", "clipboard.read-text", "demo", "clip-1"]);
    assert!(ok, "clipboard read request failed: {err}");
    assert!(out.contains("native.requested"), "out: {out}");
    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "complete",
            "demo",
            "clip-1",
            r#"{"text":"hello","truncated":false}"#,
        ],
    );
    assert!(ok, "clipboard read complete failed: {err}");
    assert!(out.contains("native.completed"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "native",
            "tray.set-menu",
            "demo",
            "tray-1",
            "Demo",
            r#"[{"id":"open","label":"Open"}]"#,
        ],
    );
    assert!(ok, "tray request failed: {err}");
    assert!(out.contains("native.requested"), "out: {out}");
    let (ok, _, err) = terrane(
        home,
        &["native", "complete", "demo", "tray-1", r#"{"installed":true}"#],
    );
    assert!(ok, "tray complete failed: {err}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("native.requested demo clip-1 clipboard.readText -> local"));
    assert!(log.contains("native.requested demo tray-1 tray.setMenu -> local"));
}

#[test]
#[ignore = "drives real macOS chrome/TCC; connector implementation lives at the host edge"]
fn native_real_macos_v2_operations_are_host_edge_cases() {}
