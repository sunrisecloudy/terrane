//! e2e smoke for `query`. Engine detail is covered by `terrane-core/tests/cap/query.rs`.

use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn query_e2e_js_backend_reads_pipeline_and_jmespath() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("query-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"query-app","name":"Query App","runtime":"js","backend":"main.js","resources":["kv","query"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
function handle(input) {
  ctx.resource.kv.set("orders/1", JSON.stringify({ day: "2026-07-06", total: 10 }));
  ctx.resource.kv.set("orders/2", JSON.stringify({ day: "2026-07-06", total: 7 }));
  const source = JSON.stringify({ kv: { prefix: "orders/" } });
  const pipeline = JSON.stringify([
    { $group: { _id: "$day", total: { $sum: "$total" }, count: { $count: {} } } }
  ]);
  const rows = JSON.parse(ctx.resource.query.pipeline(source, pipeline));
  const firstTotal = JSON.parse(ctx.resource.query.jmespath(source, "[0].total"));
  return JSON.stringify({ rows, firstTotal });
}
"#,
    )
    .unwrap();

    let src = bundle.to_str().unwrap();
    let (ok, _, err) = terrane(
        home,
        &["app", "add", "query-app", "Query App", "--source", src],
    );
    assert!(ok, "app add failed: {err}");
    for namespace in ["kv", "query"] {
        let (ok, _, err) = terrane(
            home,
            &["auth", "grant", "user:local-owner", "query-app", namespace],
        );
        assert!(ok, "grant {namespace} failed: {err}");
    }

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "query-app", "go"]);
    assert!(ok, "run failed: {err}");
    let value: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(value["rows"][0]["total"], 17);
    assert_eq!(value["rows"][0]["count"], 2);
    assert_eq!(value["firstTotal"], 10);
}

#[test]
fn query_cli_jmespath_reads_folded_state() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let (ok, _, err) = terrane(home, &["app", "add", "shop", "Shop"]);
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "kv",
            "set",
            "shop",
            "orders/1",
            r#"{"day":"2026-07-06","total":10}"#,
        ],
    );
    assert!(ok, "kv set failed: {err}");
    let (ok, out, err) = terrane(
        home,
        &[
            "query",
            "jmespath",
            "shop",
            r#"{"kv":{"prefix":"orders/"}}"#,
            "[0].total",
        ],
    );
    assert!(ok, "query jmespath failed: {err}");
    assert_eq!(out.trim(), "10");
}
