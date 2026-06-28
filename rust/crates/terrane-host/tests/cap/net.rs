//! e2e smoke for `net` — a real HTTP fetch through the binary, so `#[ignore]`d.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
#[ignore = "real network fetch; run with `cargo test -- --ignored`"]
fn net_e2e_smoke_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "web", "Web App"]);

    let (ok, out, err) = terrane(home, &["net", "fetch", "web", "http://example.com"]);
    assert!(ok, "fetch failed; stderr: {err}");
    assert!(out.contains("net.fetched"), "out: {out}");
}
