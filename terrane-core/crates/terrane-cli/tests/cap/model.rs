//! e2e smoke for `model` — a real agent call through the binary, so `#[ignore]`d.

use tempfile::tempdir;

use crate::helpers::{on_path, terrane};

#[test]
#[ignore = "real agent call (needs claude on PATH + auth; costs tokens); run with `--ignored`"]
fn model_e2e_smoke_real() {
    if !on_path("claude") {
        eprintln!("skipping model e2e: `claude` not on PATH");
        return;
    }
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "asst", "Assistant"]);

    let (ok, out, err) = terrane(
        home,
        &["model", "ask", "asst", "claude", "Reply with exactly the two characters: OK"],
    );
    assert!(ok, "agent call failed; stderr: {err}");
    assert!(out.contains("model.responded"), "out: {out}");
}
