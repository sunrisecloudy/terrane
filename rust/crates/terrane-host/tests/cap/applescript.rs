//! e2e smoke for `applescript`.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn applescript_rejects_unknown_app_and_empty_script() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "demo", "Demo"]);

    let (ok, _, err) = terrane(
        home,
        &["applescript", "run", "ghost", "return 1"],
    );
    assert!(!ok, "expected missing app rejection");
    assert!(err.contains("ghost") || err.contains("not found"), "err: {err}");

    let (ok, _, err) = terrane(home, &["applescript", "run", "demo", "   "]);
    assert!(!ok, "expected empty script rejection");
    assert!(!err.is_empty(), "err: {err}");
}

#[test]
#[ignore = "runs real osascript"]
fn applescript_run_e2e_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "calc", "Calc"]);

    let (ok, out, err) = terrane(
        home,
        &["applescript", "run", "calc", "return 2 + 2"],
    );
    assert!(ok, "run failed: {err}");
    assert!(out.contains("applescript.ran"), "out: {out}");

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("ok") || out.is_empty(), "replay out: {out}");
}