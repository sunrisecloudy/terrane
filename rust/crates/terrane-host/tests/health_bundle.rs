use std::path::PathBuf;

fn health_bundle() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../apps/health")
}

#[test]
fn checked_in_health_bundle_is_valid_and_uses_real_vision_resources() {
    let bundle = health_bundle();
    terrane_host::validate_common_api_bundle(&bundle).expect("valid Health common API");

    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle.join("manifest.json")).expect("read Health manifest"),
    )
    .expect("parse Health manifest");
    assert_eq!(manifest["id"], "health");
    assert_eq!(manifest["icon"], "icon.svg");
    assert_eq!(
        manifest["resources"],
        serde_json::json!(["kv", "blob", "model"])
    );

    let backend = std::fs::read_to_string(bundle.join("main.js")).expect("read Health backend");
    assert!(backend.contains("blob.put(blobName, base64, mime)"));
    assert!(backend.contains("function estimateBlob(args, usage)"));
    assert!(backend.contains("blob.stat(blobName)"));
    assert!(backend.contains("analyzeStoredBlob(blobName, note, settings, id)"));
    assert!(backend.contains(r#"DEFAULT_PROVIDER = "opencode""#));
    assert!(backend.contains(r#"DEFAULT_OPENCODE_MODEL = "opencode-go/kimi-k2.6""#));
    assert!(backend.contains("model.ask(agentSelector(settings), request)"));
    assert!(backend.contains(r#"{ blob: blobName }"#));
    assert!(backend.contains("function removeOwnedUnreferencedImport(name)"));
    assert!(backend.contains("if (!isPickerImport(name) || blobIsReferenced(name)) return false"));
    assert!(!backend.contains("fallback estimate"));

    let ui = std::fs::read_to_string(bundle.join("index.html")).expect("read Health UI");
    assert!(ui.contains(r#"type="file""#));
    assert!(ui.contains(r#"accept="image/jpeg,image/png,image/webp""#));
    assert!(ui.contains(r#"canvas.toDataURL("image/jpeg", .82)"#));
    assert!(ui.contains(r#"id="photos""#));
    assert!(ui.contains("window.terrane.pick({"));
    assert!(ui.contains(r#"source: "photos""#));
    assert!(ui.contains(r#"types: ["image"]"#));
    assert!(ui.contains("multiple: false"));
    assert!(ui.contains("if (!result || result.cancelled) return"));
    assert!(ui.contains("previewEl.src = blobUrl(item.name)"));
    assert!(ui.contains(r#""estimate_blob","#));
    assert!(ui.contains(r#""estimate","#));
    assert!(ui.contains(r#"<option value="opencode">OpenCode</option>"#));
    assert!(ui.contains("opencode-go/kimi-k2.6"));
    assert!(ui.contains("Analyzing the food and portion"));
    assert!(ui.contains("Estimates are approximate"));
}
