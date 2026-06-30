use tempfile::tempdir;
use terrane_core::read_log;

use super::*;

fn files() -> Vec<PreviewFile> {
    vec![
        PreviewFile {
            path: "manifest.json".to_string(),
            content: r#"{"id":"demo","name":"Demo","runtime":"js","ui":"ui/index.html","backend":"main.js","resources":["kv"]}"#.to_string(),
        },
        PreviewFile {
            path: "ui/index.html".to_string(),
            content: r#"<link rel="stylesheet" href="style.css">"#.to_string(),
        },
        PreviewFile {
            path: "ui/style.css".to_string(),
            content: "body { color: black; }".to_string(),
        },
        PreviewFile {
            path: "main.js".to_string(),
            content: r#"
                    var kv = ctx.resource.kv;
                    function handle(input) {
                        if (input[0] === "set") { kv.set(input[1], input[2]); return "ok"; }
                        if (input[0] === "get") { var v = kv.get(input[1]); return v == null ? "(none)" : v; }
                        return "?";
                    }
                "#
            .to_string(),
        },
    ]
}

#[test]
fn creates_ids_and_serves_assets_relative_to_ui_parent() {
    let mut store = PreviewStore::new();
    let created = store.create_preview(files(), &State::default()).unwrap();
    assert_eq!(created.id, "preview-demo-1");
    assert_eq!(created.frame_url, "terrane-preview://preview-demo-1/frame/");

    let index = store.read_asset(&created.id, "").unwrap();
    assert_eq!(index.content_type, "text/html; charset=utf-8");
    assert!(index.content.contains("style.css"));

    let css = store.read_asset(&created.id, "style.css").unwrap();
    assert_eq!(css.content_type, "text/css; charset=utf-8");
    assert!(css.content.contains("color"));
}

#[test]
fn rejects_invalid_files_and_manifest_refs() {
    let mut no_manifest = files();
    no_manifest.retain(|f| f.path != "manifest.json");
    assert!(PreviewStore::new()
        .create_preview(no_manifest, &State::default())
        .unwrap_err()
        .contains("missing manifest.json"));

    let mut absolute = files();
    absolute.push(PreviewFile {
        path: "/abs.js".to_string(),
        content: String::new(),
    });
    assert!(PreviewStore::new()
        .create_preview(absolute, &State::default())
        .unwrap_err()
        .contains("absolute paths"));

    let mut parent = files();
    parent.push(PreviewFile {
        path: "ui/../escape.js".to_string(),
        content: String::new(),
    });
    assert!(PreviewStore::new()
        .create_preview(parent, &State::default())
        .unwrap_err()
        .contains("parent-dir"));

    let mut duplicate = files();
    duplicate.push(PreviewFile {
        path: "./main.js".to_string(),
        content: String::new(),
    });
    assert!(PreviewStore::new()
        .create_preview(duplicate, &State::default())
        .unwrap_err()
        .contains("duplicate"));

    let mut unsupported = files();
    unsupported.push(PreviewFile {
        path: "README.md".to_string(),
        content: String::new(),
    });
    assert!(PreviewStore::new()
        .create_preview(unsupported, &State::default())
        .unwrap_err()
        .contains("unsupported"));

    let mut missing_ui = files();
    missing_ui[0].content =
        r#"{"id":"demo","runtime":"js","backend":"main.js","resources":["kv"]}"#.to_string();
    assert!(PreviewStore::new()
        .create_preview(missing_ui, &State::default())
        .unwrap_err()
        .contains("missing manifest.ui"));

    let mut missing_backend_ref = files();
    missing_backend_ref[0].content =
        r#"{"id":"demo","runtime":"js","ui":"ui/index.html","backend":"missing.js"}"#.to_string();
    assert!(PreviewStore::new()
        .create_preview(missing_backend_ref, &State::default())
        .unwrap_err()
        .contains("manifest.backend references missing file"));
}

#[test]
fn invoke_backend_keeps_preview_state_without_appending_real_log() {
    let dir = tempdir().unwrap();
    let core = crate::open_at_home(dir.path()).unwrap();
    let log = dir.path().join("log.bin");
    let before = read_log(&log).unwrap();

    let mut store = PreviewStore::new();
    let created = store.create_preview(files(), core.state()).unwrap();
    let requests = store.permission_requests("http://127.0.0.1:49152");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].status, "pending");
    assert_eq!(requests[0].source, "preview");
    assert_eq!(requests[0].app, created.id);
    assert!(
        store
            .invoke_backend(
                &created.id,
                "set",
                &["answer".to_string(), "42".to_string()],
            )
            .is_err(),
        "preview resources should be default-deny until approved"
    );
    let request_id = requests[0].request_id.clone();
    let approved = store
        .approve_permission_request(&request_id, "ok", "http://127.0.0.1:49152")
        .unwrap()
        .unwrap();
    assert_eq!(approved.status, "approved");
    let out = store
        .invoke_backend(
            &created.id,
            "set",
            &["answer".to_string(), "42".to_string()],
        )
        .unwrap();
    assert_eq!(out, "ok");
    let out = store
        .invoke_backend(&created.id, "get", &["answer".to_string()])
        .unwrap();
    assert_eq!(out, "42");

    let after = read_log(&log).unwrap();
    assert_eq!(after, before, "preview writes must not append to log");

    assert!(core.replay_matches().unwrap());
    assert!(!core.state().kv.data.contains_key(&created.id));
    store.destroy_preview(&created.id).unwrap();
    assert!(store
        .permission_request(&request_id, "http://127.0.0.1:49152")
        .is_none());
}

#[test]
fn parses_object_or_array_files_payload() {
    let mut store = PreviewStore::new();
    let raw_object = format!(r#"{{"files":{}}}"#, files().serialize_json());
    assert_eq!(
        store
            .create_preview_from_json(&raw_object, &State::default())
            .unwrap()
            .id,
        "preview-demo-1"
    );

    assert_eq!(
        store
            .create_preview_from_json(&files().serialize_json(), &State::default())
            .unwrap()
            .id,
        "preview-demo-2"
    );
}
