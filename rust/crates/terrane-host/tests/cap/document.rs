//! e2e smoke for `document`. Logic detail is covered by `terrane-core` cap tests.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn document_e2e_runs_js_backend_and_cli_reads_folded_state() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"notes","name":"Notes","runtime":"js","backend":"main.js","resources":["document"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r##"
function handle(input) {
  var docs = ctx.resource.document;
  if (input[0] === "seed") {
    docs.create("daily", "Daily", "# Daily", JSON.stringify({kind:"note"}));
    docs.append("daily", "\nDone");
    docs.patch("daily", JSON.stringify({metadata:{kind:null,status:"ready"}}));
    return docs.exportMarkdown("daily");
  }
  if (input[0] === "get") {
    return docs.get("daily");
  }
  return JSON.stringify(docs.list());
}
"##,
    )
    .unwrap();

    let source = bundle.to_str().unwrap();
    let (ok, _, err) = terrane(
        home,
        &["app", "add", "notes", "Notes", "--source", source],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "notes", "document"],
    );
    assert!(ok, "auth grant failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "notes", "seed"]);
    assert!(ok, "js-runtime run failed: {err}");
    assert_eq!(out.trim(), "# Daily\nDone");

    let (ok, out, err) = terrane(home, &["document", "get", "notes", "daily"]);
    assert!(ok, "document get failed: {err}");
    assert!(out.contains(r#""title":"Daily""#), "get out: {out}");
    assert!(out.contains(r#""status":"ready""#), "get out: {out}");
    assert!(!out.contains(r#""kind":"note""#), "get out: {out}");

    let (ok, out, err) = terrane(home, &["document", "ls", "notes"]);
    assert!(ok, "document ls failed: {err}");
    assert!(out.contains(r#""bodyBytes":12"#), "ls out: {out}");

    let (ok, out, err) = terrane(home, &["document", "rm", "notes", "daily"]);
    assert!(ok, "document rm failed: {err}");
    assert!(out.contains("document.deleted"), "rm out: {out}");
    let (ok, out, err) = terrane(home, &["document", "get", "notes", "daily"]);
    assert!(ok, "document get after rm failed: {err}");
    assert_eq!(out.trim(), "null");
}
