use super::*;

fn generated_json() -> String {
    let manifest = concat!(
        r#"{"id":"demo","name":"Demo","version":"0.1.0","#,
        r#""runtime":"js","backend":"main.js","ui":"index.html","#,
        r#""resources":["kv"]}"#
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
fn parses_and_validates_generated_bundle_files() {
    let files = parse_generated_files(&generated_json(), "demo", "Demo").unwrap();
    assert_eq!(files.len(), 4);
    assert_eq!(files[0].path, "index.html");
    assert!(files.iter().any(|f| f.path == "manifest.json"));
}

#[test]
fn accepts_planned_document_resource_in_generated_manifests() {
    let with_document = generated_json().replace("\\\"kv\\\"", "\\\"document\\\"");
    let files = parse_generated_files(&with_document, "demo", "Demo").unwrap();
    assert!(files.iter().any(|f| f.path == "manifest.json"));
}

#[test]
fn rejects_unsafe_or_mismatched_generated_files() {
    let bad_path = generated_json().replace("style.css", "../escape.css");
    assert!(parse_generated_files(&bad_path, "demo", "Demo")
        .unwrap_err()
        .to_string()
        .contains("parent-dir"));

    let bad_id = generated_json().replace("\\\"id\\\":\\\"demo\\\"", "\\\"id\\\":\\\"other\\\"");
    assert!(parse_generated_files(&bad_id, "demo", "Demo")
        .unwrap_err()
        .to_string()
        .contains("must match"));

    let bad_resource = generated_json().replace("\\\"kv\\\"", "\\\"net\\\"");
    assert!(parse_generated_files(&bad_resource, "demo", "Demo")
        .unwrap_err()
        .to_string()
        .contains("unsupported"));
}
