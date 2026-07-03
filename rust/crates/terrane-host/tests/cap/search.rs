//! e2e smoke for `search`. Logic detail is covered by
//! `rust/crates/terrane-core/tests/cap/search.rs`.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn search_upsert_remove_and_status_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    terrane(home, &["app", "add", "notes", "Notes"]);
    let (ok, out, err) = terrane(
        home,
        &["search", "upsert", "notes", "doc-1", "the quick brown fox"],
    );
    assert!(ok, "upsert failed: {err}");
    assert!(out.contains("kv.set"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &[
            "search",
            "setEmbedding",
            "notes",
            "doc-1",
            "[1.0,0.0,0.5]",
        ],
    );
    assert!(ok, "setEmbedding failed: {err}");
    assert!(out.contains("kv.set"), "out: {out}");

    let (ok, out, err) = terrane(home, &["search", "remove", "notes", "doc-1"]);
    assert!(ok, "remove failed: {err}");
    assert!(out.contains("kv.deleted"), "out: {out}");

    let (ok, _, err) = terrane(
        home,
        &["search", "setEmbedding", "notes", "doc-1", "[0.1]"],
    );
    assert!(!ok, "setEmbedding on missing doc should fail");
    assert!(err.contains("not indexed"), "stderr: {err}");
}

#[test]
#[ignore = "real embedding model; run with `cargo test -p terrane-host -- --ignored`"]
fn search_hybrid_query_with_real_embedding_e2e() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "notes", "Notes"]);
    let (ok, _, err) = terrane(home, &["local-model", "pull", "--embed"]);
    assert!(ok, "embed pull failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["search", "upsert", "notes", "doc-1", "the quick brown fox"],
    );
    assert!(ok, "upsert failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &["local-model", "embed", "notes", "the quick brown fox"],
    );
    assert!(ok, "embed failed: {err}");
    assert!(out.contains("local-model.embedded"), "out: {out}");

    // Re-run embed to capture vector output via resource path would need host.run;
    // this smoke proves the indexing + embed path is wired.
    let (ok, _, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
}