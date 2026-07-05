use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn web_publish_cli_records_status_domain_and_disable() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &["web-publish", "enable", "demo", "interactive", "demo-live"],
    );
    assert!(ok, "web-publish enable failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("web-publish.enabled"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &["web-publish", "domain", "set", "demo", "demo.example.com"],
    );
    assert!(ok, "web-publish domain set failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("web-publish.domain.set"), "out: {out}");

    let (ok, out, err) = terrane(home, &["web-publish", "status", "demo"]);
    assert!(ok, "web-publish status failed; stdout: {out}; stderr: {err}");
    assert!(out.contains(r#""mode":"interactive""#), "out: {out}");
    assert!(out.contains(r#""url":"https://demo.example.com""#), "out: {out}");

    let (ok, _, err) = terrane(home, &["web-publish", "disable", "demo"]);
    assert!(ok, "web-publish disable failed: {err}");
    let (ok, out, err) = terrane(home, &["web-publish", "status", "demo"]);
    assert!(ok, "web-publish status after disable failed: {err}");
    assert!(out.contains(r#""enabled":false"#), "out: {out}");

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed; stdout: {out}; stderr: {err}");
    assert!(out.contains("replay ok"), "out: {out}");
}

#[test]
fn web_publish_cli_rejects_bad_mode_before_recording() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "demo", "Demo"]);
    assert!(ok, "app add failed: {err}");

    let (ok, _out, err) = terrane(home, &["web-publish", "enable", "demo", "mutable"]);
    assert!(!ok, "bad mode should fail");
    assert!(err.contains("static or interactive"), "stderr: {err}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(!log.contains("web-publish.enabled"), "log: {log}");
}
