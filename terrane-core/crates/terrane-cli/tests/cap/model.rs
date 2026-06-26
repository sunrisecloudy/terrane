//! e2e for the `model` capability — a real agent call, so `#[ignore]`d.

use tempfile::tempdir;

use crate::helpers::{on_path, terrane};

#[test]
#[ignore = "real agent call (needs claude on PATH + auth; costs tokens); run with `--ignored`"]
fn model_capability_e2e_real() {
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

    let (_, log, _) = terrane(home, &["log"]);
    assert!(
        log.contains("model.responded asst via claude"),
        "log: {log}"
    );

    // Replay rebuilds the recorded transcript — the agent is not re-run.
    let (ok, replay, _) = terrane(home, &["replay"]);
    assert!(ok);
    assert!(replay.contains("replay ok"), "replay: {replay}");
}
