//! Builder allow-list: the `build` resource namespace is accepted (symmetric to
//! the `relational_db` case in the in-crate tests), and a namespace absent from
//! the allow-list is rejected. Integration test over the public API only; lives
//! in its own file.

use terrane_cap_builder::parse_generated_files;

const ALLOWED: &[&str] = &[
    "kv",
    "crdt",
    "relational_db",
    "build",
    "search",
    "local-model",
];

/// Build a builder-output bundle whose manifest requests `resources_json`
/// (e.g. `["build"]`). Mirrors the in-crate `generated_json` fixture shape.
fn generated_json(resources_json: &str) -> String {
    let manifest = format!(
        concat!(
            r#"{{"id":"demo","name":"Demo","version":"0.1.0","#,
            r#""runtime":"js","backend":"main.js","ui":"index.html","#,
            r#""resources":{resources}}}"#
        ),
        resources = resources_json
    );
    let main_js = concat!(
        r#"var actions={hello:{summary:"Say hello.","#,
        r#"args:[],run:function(){return "hi";}}};"#
    );
    format!(
        r#"{{"files":[
{{"path":"manifest.json","content":{manifest:?}}},
{{"path":"main.js","content":{main_js:?}}},
{{"path":"index.html","content":"<!doctype html><title>Demo</title><script src=\"app.js\"></script>"}},
{{"path":"style.css","content":"body {{ font-family: system-ui; }}"}}
]}}"#
    )
}

#[test]
fn build_resource_is_accepted_by_allow_list() {
    let files =
        parse_generated_files(&generated_json(r#"["build"]"#), "demo", "Demo", ALLOWED).unwrap();
    assert!(files.iter().any(|f| f.path == "manifest.json"));
}

#[test]
fn search_resource_is_accepted_by_allow_list() {
    let files = parse_generated_files(
        &generated_json(r#"["search"]"#),
        "demo",
        "Demo",
        ALLOWED,
    )
    .unwrap();
    assert!(files.iter().any(|f| f.path == "manifest.json"));
}

#[test]
fn resource_absent_from_allow_list_is_rejected() {
    // `net` is not in the derived allow-list (no registered grant spec today).
    assert!(parse_generated_files(&generated_json(r#"["net"]"#), "demo", "Demo", ALLOWED).is_err());
}
