//! e2e smoke for `search`. Logic detail is covered by
//! `rust/crates/terrane-core/tests/cap/search.rs`.

use std::fs;
use std::path::PathBuf;

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
        &["search", "setEmbedding", "notes", "doc-1", "[1.0,0.0,0.5]"],
    );
    assert!(ok, "setEmbedding failed: {err}");
    assert!(out.contains("kv.set"), "out: {out}");

    let (ok, out, err) = terrane(home, &["search", "remove", "notes", "doc-1"]);
    assert!(ok, "remove failed: {err}");
    assert!(out.contains("kv.deleted"), "out: {out}");

    let (ok, _, err) = terrane(home, &["search", "setEmbedding", "notes", "doc-1", "[0.1]"]);
    assert!(!ok, "setEmbedding on missing doc should fail");
    assert!(err.contains("not indexed"), "stderr: {err}");
}

#[test]
fn search_hybrid_query_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = dir.path().join("query-bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"notes","name":"Notes","runtime":"js","backend":"main.js","resources":["search"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
function handle(input) {
  return ctx.resource.search.query(
    "fox",
    JSON.stringify({ limit: 5, queryVec: [1.0, 0.0, 0.5] })
  );
}
"#,
    )
    .unwrap();

    let src = bundle.to_str().expect("utf-8 path");
    let (ok, _, err) = terrane(home, &["app", "add", "notes", "Notes", "--source", src]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "notes", "search"],
    );
    assert!(ok, "grant search failed: {err}");

    let (ok, _, err) = terrane(
        home,
        &["search", "upsert", "notes", "doc-1", "the quick brown fox"],
    );
    assert!(ok, "upsert failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["search", "setEmbedding", "notes", "doc-1", "[1.0,0.0,0.5]"],
    );
    assert!(ok, "setEmbedding failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "search",
            "upsert",
            "notes",
            "doc-2",
            "lazy dog sleeps all day",
        ],
    );
    assert!(ok, "upsert doc-2 failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "notes", "query"]);
    assert!(ok, "hybrid query via js-runtime failed: {err}");
    assert!(
        out.contains("doc-1"),
        "expected doc-1 in hybrid query output: {out}"
    );
}

#[test]
fn search_notes_app_bundle_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = app_source("search-notes");

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "search-notes",
            "Search Notes",
            "--source",
            &src,
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "auth",
            "grant",
            "user:local-owner",
            "search-notes",
            "search",
        ],
    );
    assert!(ok, "grant search failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &[
            "js-runtime",
            "run",
            "search-notes",
            "index",
            "doc-1",
            "the quick brown fox",
        ],
    );
    assert!(ok, "index failed: {err}");
    assert!(out.contains("indexed doc-1"), "out: {out}");

    let (ok, _, err) = terrane(
        home,
        &[
            "search",
            "setEmbedding",
            "search-notes",
            "doc-1",
            "[1.0,0.0,0.5]",
        ],
    );
    assert!(ok, "setEmbedding failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "search-notes", "bm25", "fox"]);
    assert!(ok, "bm25 failed: {err}");
    assert!(out.contains("doc-1"), "bm25 output: {out}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "search-notes", "status"]);
    assert!(ok, "status failed: {err}");
    assert!(out.contains("documentCount"), "status output: {out}");
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
