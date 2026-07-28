use std::path::PathBuf;

fn spending_bundle() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../apps/spending")
}

#[test]
fn checked_in_spending_bundle_has_real_invoice_vision_and_accounting_workflows() {
    let bundle = spending_bundle();
    terrane_host::validate_common_api_bundle(&bundle).expect("valid Spending common API");

    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(bundle.join("manifest.json")).expect("read Spending manifest"),
    )
    .expect("parse Spending manifest");
    assert_eq!(manifest["id"], "spending");
    assert_eq!(
        manifest["resources"],
        serde_json::json!(["kv", "blob", "model"])
    );

    let backend = std::fs::read_to_string(bundle.join("main.js")).expect("read backend");
    assert!(backend.contains("blob.put(blobName, base64, mime)"));
    assert!(backend.contains("model.ask(agentSelector(settings), request)"));
    assert!(backend.contains(r#"{ blob: blobName }"#));
    assert!(backend.contains("analyze_invoice"));
    assert!(backend.contains("analyze_blob"));
    assert!(backend.contains("blob.stat(blobName)"));
    assert!(backend.contains("if (!previewOnly) saveExpense(entry)"));
    assert!(backend.contains("create_manual"));
    assert!(backend.contains("create_pocket"));
    assert!(backend.contains("update_pocket"));
    assert!(!backend.contains("fallback invoice"));

    let ui = std::fs::read_to_string(bundle.join("index.html")).expect("read UI");
    assert!(ui.contains(r#"type="file""#));
    assert!(ui.contains(r#"accept="image/jpeg,image/png,image/webp""#));
    assert!(ui.contains(r#"canvas.toDataURL("image/jpeg", .84)"#));
    assert!(ui.contains("Analyze invoice"));
    assert!(ui.contains("Review before saving"));
    assert!(ui.contains(r#"selected.model, "preview")"#));
    assert!(ui.contains("setSidebarSection"));
    assert!(ui.contains("onSidebarItemSelect"));
    assert!(ui.contains("onSidebarCreate"));
    assert!(ui.contains("Budget pockets"));
}
