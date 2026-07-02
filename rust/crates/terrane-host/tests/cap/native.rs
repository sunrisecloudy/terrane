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
