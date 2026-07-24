//! App-side lower-sidebar data behavior. These tests run real built-in bundles
//! against throwaway homes; native chrome rendering is covered by macOS tests.

use std::path::PathBuf;

use serde_json::Value;
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

fn install_kv_app(home: &std::path::Path, id: &str, name: &str) {
    let source = app_source(id);
    let (ok, _, err) = terrane(home, &["app", "add", id, name, "--source", source.as_str()]);
    assert!(ok, "app add {id} failed: {err}");
    let (ok, _, err) = terrane(home, &["auth", "grant", "user:local-owner", id, "kv"]);
    assert!(ok, "grant {id} kv failed: {err}");
}

fn run_json(home: &std::path::Path, app: &str, args: &[&str]) -> Value {
    let mut command = vec!["js-runtime", "run", app];
    command.extend_from_slice(args);
    let (ok, out, err) = terrane(home, &command);
    assert!(ok, "{app} {:?} failed: {err}", args);
    serde_json::from_str(out.trim())
        .unwrap_or_else(|error| panic!("{app} {:?} returned invalid JSON {out:?}: {error}", args))
}

#[test]
fn pixel_paint_canvases_create_switch_and_preserve_legacy_canvas() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    install_kv_app(home, "pixel-paint", "Pixel Paint");

    let (ok, _, err) = terrane(
        home,
        &[
            "js-runtime",
            "run",
            "pixel-paint",
            "set",
            "2",
            "3",
            "#ff0066",
        ],
    );
    assert!(ok, "paint legacy canvas failed: {err}");

    let created = run_json(home, "pixel-paint", &["new"]);
    assert_eq!(created["selectedCanvas"], "c1");
    assert_eq!(created["canvases"].as_array().unwrap().len(), 2);

    let (ok, _, err) = terrane(
        home,
        &[
            "js-runtime",
            "run",
            "pixel-paint",
            "set",
            "5",
            "6",
            "#00ff00",
        ],
    );
    assert!(ok, "paint second canvas failed: {err}");

    let selected = run_json(home, "pixel-paint", &["select", "default"]);
    assert_eq!(selected["selectedCanvas"], "default");
    assert_eq!(selected["pixels"]["2,3"], "#ff0066");
    assert!(selected["pixels"].get("5,6").is_none());
}

#[test]
fn todo_completed_view_is_durable_and_restorable() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    install_kv_app(home, "todo", "Todo");

    let (ok, _, err) = terrane(
        home,
        &["js-runtime", "run", "todo", "add", "ship", "sidebar"],
    );
    assert!(ok, "add todo failed: {err}");
    let (ok, _, err) = terrane(home, &["js-runtime", "run", "todo", "done", "1"]);
    assert!(ok, "complete todo failed: {err}");

    assert_eq!(
        run_json(home, "todo", &["items", "open"]),
        Value::Array(vec![])
    );
    let completed = run_json(home, "todo", &["items", "completed"]);
    assert_eq!(completed[0]["text"], "ship sidebar");

    let (ok, _, err) = terrane(home, &["js-runtime", "run", "todo", "restore", "1"]);
    assert!(ok, "restore todo failed: {err}");
    assert_eq!(run_json(home, "todo", &["items", "open"])[0]["id"], 1);

    let (ok, out, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
    assert!(out.contains("replay ok"), "replay output: {out}");
}
