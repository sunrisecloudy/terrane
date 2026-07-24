//! End-to-end proof for the built-in Control Room app. The test uses a
//! throwaway home, drives the real JS runtime, and plants a sentinel raw value
//! to prove the catalog returns counts rather than contents.

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

#[test]
fn control_room_catalog_is_comprehensive_read_only_and_redacted() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let source = app_source("control-room");

    let (ok, out, err) = terrane(
        home,
        &[
            "app",
            "add",
            "control-room",
            "Control Room",
            "--source",
            &source,
        ],
    );
    assert!(ok && out.contains("app.added"), "app add: {out} {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "auth",
            "grant",
            "user:local-owner",
            "control-room",
            "control-room",
        ],
    );
    assert!(ok, "control-room grant failed: {err}");

    // Plant content that must never appear in the management catalog.
    let secret = "SENTINEL-control-room-must-not-leak";
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "control-room", "kv"],
    );
    assert!(ok, "kv grant failed: {err}");
    let (ok, _, err) = terrane(home, &["kv", "set", "control-room", "private", secret]);
    assert!(ok, "kv set failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "control-room", "catalog"]);
    assert!(ok, "catalog failed: {err}");
    assert!(!out.contains(secret), "catalog leaked a raw KV value");
    assert!(
        !out.contains(home.to_string_lossy().as_ref()),
        "catalog leaked its home/source path"
    );

    let value: Value = serde_json::from_str(out.trim()).expect("catalog should be JSON");
    assert_eq!(value["privacy"]["mode"], "metadata-only");
    assert_eq!(
        value["generatedFrom"]["rawSensitiveContentsIncluded"],
        false
    );
    assert!(
        value["capabilities"]
            .as_array()
            .is_some_and(|items| items.len() >= 40),
        "expected the full registered capability inventory"
    );
    assert!(
        value["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .any(|capability| capability["namespace"] == "control-room"),
        "new capability should describe itself"
    );
    assert!(
        value["mcp"]["tools"]
            .as_array()
            .is_some_and(|tools| tools.len() >= 10),
        "expected compiled MCP tool inventory"
    );
    assert_eq!(value["storage"]["kv"]["contents"], "redacted");
    assert_eq!(value["storage"]["kv"]["records"], 1);

    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(
        !log.contains("control-room."),
        "catalog reads must not append capability events: {log}"
    );
}
