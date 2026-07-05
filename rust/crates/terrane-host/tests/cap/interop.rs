use std::fs;

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn two_apps_call_each_other_and_resolve_item_uri() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let target = home.join("target-app");
    fs::create_dir(&target).unwrap();
    fs::write(
        target.join("manifest.json"),
        r#"{ "id": "target-app", "name": "Target", "runtime": "js", "backend": "main.js", "resources": ["kv"], "interfaces": ["items"] }"#,
    )
    .unwrap();
    fs::write(
        target.join("main.js"),
        r#"
            var actions = {
              seed: { run: function () { ctx.resource.kv.set("items/alpha", JSON.stringify({ id: "alpha", title: "Alpha", kind: "note" })); return "seeded"; } },
              "common.list": { run: function () { return JSON.stringify([{ id: "alpha", title: "Alpha", kind: "note" }]); } },
              "common.get": { run: function (args) {
                var raw = ctx.resource.kv.get("items/" + (args[0] || ""));
                return raw == null ? JSON.stringify({ ok: false, error: { code: "NotFound", id: args[0] || "" } }) : raw;
              } }
            };
        "#,
    )
    .unwrap();

    let caller = home.join("caller-app");
    fs::create_dir(&caller).unwrap();
    fs::write(
        caller.join("manifest.json"),
        r#"{ "id": "caller-app", "name": "Caller", "runtime": "js", "backend": "main.js", "resources": ["interop"], "interfaces": ["items"] }"#,
    )
    .unwrap();
    fs::write(
        caller.join("main.js"),
        r#"
            var actions = {
              resolve: { run: function (args) {
                return ctx.resource.interop.call(args[0], "common.get", args[1]);
              } }
            };
        "#,
    )
    .unwrap();

    for (id, path) in [("target-app", &target), ("caller-app", &caller)] {
        let (ok, _, err) = terrane(
            home,
            &[
                "app",
                "add",
                id,
                id,
                "--source",
                path.to_str().unwrap(),
            ],
        );
        assert!(ok, "app add {id} failed: {err}");
    }
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", "target-app", "kv"]);
    assert!(ok, "grant target kv failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "caller-app", "interop"],
    );
    assert!(ok, "grant caller interop failed: {err}");

    let (ok, _, err) = terrane(home, &["js-runtime", "run", "target-app", "seed"]);
    assert!(ok, "seed failed: {err}");
    let (ok, out, err) = terrane(
        home,
        &[
            "js-runtime",
            "run",
            "caller-app",
            "resolve",
            "target-app",
            "alpha",
        ],
    );
    assert!(ok, "interop resolve failed: {err}");
    assert!(out.contains(r#""title":"Alpha""#), "out: {out}");

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("interop.called caller-app -> target-app common.get"), "log: {log}");
}

#[test]
fn bundle_validation_rejects_missing_common_api() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bundle = home.join("bad-app");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "bad-app", "name": "Bad", "runtime": "js", "backend": "main.js", "resources": [], "interfaces": ["items"] }"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
            function handle(input) {
              if ((input[0] || "") === "__actions__") {
                return JSON.stringify({ actions: [{ verb: "status", args: [], returns: "ok" }] });
              }
              return "ok";
            }
        "#,
    )
    .unwrap();

    let (ok, _out, err) = terrane(home, &["app", "install", bundle.to_str().unwrap()]);

    assert!(!ok, "install should reject missing common API");
    assert!(
        err.contains("common.receive") || err.contains("required verb"),
        "err: {err}"
    );
}
