//! e2e for the `net` capability — a real HTTP fetch, so `#[ignore]`d.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
#[ignore = "real network fetch; run with `cargo test -- --ignored`"]
fn net_capability_e2e_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);

    let (ok, out, err) = terrane(home, &["net", "fetch", "web", "http://example.com"]);
    assert!(ok, "fetch failed; stderr: {err}");
    assert!(out.contains("net.fetched"), "out: {out}");

    let (_, log, _) = terrane(home, &["log"]);
    assert!(
        log.contains("net.fetched web http://example.com"),
        "log: {log}"
    );

    // Replay rebuilds the recorded response from the log — no second fetch.
    let (ok, replay, _) = terrane(home, &["replay"]);
    assert!(ok);
    assert!(replay.contains("replay ok"), "replay: {replay}");
}
