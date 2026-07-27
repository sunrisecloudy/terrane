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
    assert_eq!(manifest["version"], "0.3.0");
    assert_eq!(manifest["icon"], "icon.svg");
    assert_eq!(manifest["ui"], "dist/index.html");
    assert_eq!(manifest["frontend"]["tool"], "terrane-app-build");
    assert_eq!(manifest["frontend"]["entry"], "src/main.tsx");
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
    assert!(backend.contains("entry.eaten_at = new Date().toISOString()"));
    assert!(backend.contains("normalized.eaten_at = normalizeEatenAt"));
    assert!(backend.contains("MAX_HISTORY = 1000"));
    assert!(backend.contains("MAX_DISHES = 12"));
    assert!(backend.contains("function normalizeDish(value, index)"));
    assert!(backend.contains("function dishTotals(dishes)"));
    assert!(backend.contains("every distinct visible dish"));
    assert!(backend.contains("Do not return combined nutrient totals"));
    assert!(backend.contains(r#""navigation.consume""#));
    assert!(backend.contains(r#""common.receive""#));
    assert!(backend.contains("function validateNavigation(value)"));
    assert!(backend.contains("MAX_PENDING_NAVIGATIONS = 8"));
    assert!(!backend.contains("fallback estimate"));

    let app = std::fs::read_to_string(bundle.join("src/App.tsx")).expect("read Health React app");
    let picker = std::fs::read_to_string(bundle.join("src/routes/AddMealRoute.tsx"))
        .expect("read Health add-meal route");
    let bridge =
        std::fs::read_to_string(bundle.join("src/terrane.ts")).expect("read Health bridge helpers");
    let router = std::fs::read_to_string(bundle.join("src/router.ts")).expect("read Health router");
    let meal = std::fs::read_to_string(bundle.join("src/routes/MealRoute.tsx"))
        .expect("read Health meal route");
    let calendar_route = std::fs::read_to_string(bundle.join("src/routes/CalendarRoute.tsx"))
        .expect("read Health calendar route");
    let styles =
        std::fs::read_to_string(bundle.join("src/app.css")).expect("read Health UI styles");
    let calendar = std::fs::read_to_string(bundle.join("src/domain/calendar.js"))
        .expect("read Health calendar domain");

    assert!(picker.contains(r#"type="file""#));
    assert!(picker.contains(r#"accept="image/jpeg,image/png,image/webp""#));
    assert!(picker.contains("Choose from Photos"));
    assert!(picker.contains(r#"<option value="opencode">OpenCode</option>"#));
    assert!(picker.contains("opencode-go/kimi-k2.6"));
    assert!(picker.contains("Estimates are approximate"));
    assert!(bridge.contains("window.terrane.pick({"));
    assert!(bridge.contains(r#"source: "photos""#));
    assert!(bridge.contains(r#"types: ["image"]"#));
    assert!(bridge.contains("multiple: false"));
    assert!(picker.contains("if (result.cancelled) return"));
    assert!(picker.contains(r#""estimate_blob""#));
    assert!(picker.contains(r#""estimate""#));
    assert!(picker.contains("Identifying dishes and estimating nutrition"));
    assert!(meal.contains("Dishes in this photo"));
    assert!(meal.contains("blobUrl(draft.blob_name)"));
    assert!(calendar_route.contains("HealthCalendar.summarize"));
    assert!(app.contains("publishSidebar"));
    assert!(app.contains(r#""navigation.consume""#));
    assert!(router.contains(r#"name === "meal""#));
    for route in ["add", "calendar", "history", "insights", "settings"] {
        assert!(app.contains(&format!(r#"case "{route}""#)) || route == "add");
        assert!(bridge.contains(&format!(r#"id: "{route}""#)));
    }

    assert!(styles.contains(".result-layout.has-image"));
    assert!(styles.contains(".insights-grid"));
    assert!(styles.contains(".route-stack"));
    assert!(calendar.contains("function monthCells(cursor, entries, today)"));
    assert!(calendar.contains("function summarize(entries, mode, selected)"));
    assert!(calendar.contains("var mondayOffset = (start.getDay() + 6) % 7"));
}
