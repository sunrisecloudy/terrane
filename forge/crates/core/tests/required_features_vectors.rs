//! Data-driven conformance over the T038 MP-8 required-features / capability
//! negotiation vectors (`forge/fixtures/required-features/*.json`, spec
//! `forge/spec/required-features.md`, prd-merged/08 MP-8).
//!
//! Each vector pins a SEMANTIC install decision — proceed or refuse — for a
//! package's `compatibility` (`required_features` + optional `min_app_version`)
//! against a specific installing client's supported feature set. The harness
//! installs the case's client registry through the TRUSTED
//! `WorkspaceCore::set_client_feature_registry` seam (never the request payload),
//! materializes the package compatibility onto a real, compiling manifest, and
//! drives a REAL `applet.install` through the SAME facade a shell uses
//! (`WorkspaceCore::handle`). It asserts the live install path actually proceeds
//! or is REFUSED — and that a refusal's typed error ENUMERATES every unsupported
//! feature with the pinned message substrings (the MP-8 "enumerated unsupported
//! list" contract). This proves the gate is live-wired, not a tested-but-
//! disconnected check.
//!
//! The guard `ran == manifest.count` (10) keeps the corpus honest: every declared
//! vector is exercised, and a newly added fixture fails the suite until it is driven.

use forge_core::{ClientFeatureRegistry, WorkspaceCore};
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, WorkspaceId};
use serde_json::Value;
use std::path::{Path, PathBuf};

const APPLET_ID: &str = "applet.market";

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/core
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/required-features")
        .canonicalize()
        .expect("required-features fixtures dir exists")
}

/// The owner actor (permits installing in M0a).
fn owner() -> ActorContext {
    ActorContext::owner("alice")
}

fn cmd(name: &str, payload: Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("r1"),
        actor: owner(),
        workspace_id: WorkspaceId::new("ws1"),
        applet_id: Some(AppletId::new(APPLET_ID)),
        name: name.into(),
        payload,
    }
}

/// A real, compiling single-file applet source — the negotiation gate runs BEFORE
/// compilation, but using genuine source means an install that PASSES negotiation
/// also compiles and stores, so the "install" decision is a real end-to-end install.
const SOURCE: &str = r#"
    export async function main(ctx: any, input: any): Promise<any> {
        return { ok: true, value: null };
    }
"#;

/// Build the package manifest carrying the case's `compatibility` (the rest is a
/// minimal, valid M0a manifest). The compatibility object deserializes straight
/// into `forge_domain::Compatibility` via the manifest's `compatibility` field.
fn manifest_with_compatibility(compat: &Value) -> Value {
    serde_json::json!({
        "entrypoint": "src/main.ts",
        "min_api": "forge-api@0.1",
        "deterministic": true,
        "capabilities": { "ui": true },
        "limits": {
            "wall_ms": 3000,
            "fuel": 10000000,
            "memory_bytes": 67108864,
            "max_host_calls": 10000,
            "storage_bytes": 10485760,
            "log_bytes": 262144
        },
        "compatibility": compat
    })
}

/// Build the case's client feature registry from its `client_features` pairs.
fn registry_from(case: &Value) -> ClientFeatureRegistry {
    let pairs = case["client_features"]
        .as_array()
        .expect("client_features array")
        .iter()
        .map(|f| {
            (
                f["feature_id"].as_str().expect("feature_id").to_string(),
                f["version"].as_str().expect("version").to_string(),
            )
        })
        .collect::<Vec<_>>();
    ClientFeatureRegistry::from_pairs(pairs)
}

#[test]
fn required_features_vectors_conformance() {
    let dir = fixtures_dir();
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("manifest.json")).unwrap())
            .expect("manifest.json parses");
    let declared = manifest["count"].as_u64().expect("manifest.count") as usize;

    let mut ran = 0usize;
    for entry in manifest["cases"].as_array().expect("manifest.cases") {
        let file = entry["file"].as_str().expect("case file");
        let vector: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.join(file)).unwrap())
                .unwrap_or_else(|e| panic!("vector {file} parses: {e}"));
        drive_vector(&vector);
        ran += 1;
    }

    assert_eq!(
        ran, declared,
        "every declared T038 required-features vector ({declared}) must be driven; ran {ran}"
    );
    assert_eq!(declared, 10, "the T038 suite pins 10 required-features vectors");
}

/// Drive ONE vector through a real `applet.install` and assert its decision.
fn drive_vector(vector: &Value) {
    let case = vector["case"].as_str().expect("case name");
    let mut core = WorkspaceCore::in_memory("ws1").unwrap();

    // Install the case's TRUSTED client feature registry (the source of truth the
    // gate reads — never the request payload).
    core.set_client_feature_registry(registry_from(vector))
        .expect("set client feature registry");

    // Drive the REAL install command through the facade.
    let resp = core.handle(cmd(
        "applet.install",
        serde_json::json!({
            "manifest": manifest_with_compatibility(&vector["manifest_compatibility"]),
            "sources": { "src/main.ts": SOURCE },
        }),
    ));

    let expect = &vector["expect"];
    match expect["decision"].as_str().expect("decision") {
        "install" => {
            assert!(
                resp.ok,
                "case {case:?}: install must proceed, got error {:?}",
                resp.error
            );
            // A real install: the active applet is enabled v1.
            assert_eq!(resp.payload["lifecycle"], serde_json::json!("enabled"));
            assert_eq!(resp.payload["version"], serde_json::json!(1));
            // And it is genuinely installed (a real run on the active applet succeeds).
            assert!(
                applet_runs(&mut core),
                "case {case:?}: a passing negotiation actually stored a runnable applet"
            );
        }
        "refuse" => {
            assert!(
                !resp.ok,
                "case {case:?}: install must be REFUSED, but it succeeded: {:?}",
                resp.payload
            );
            let err = resp.error.expect("a refusal carries an error");
            // The refusal is a ValidationError (the uniform install-refusal kind).
            assert_eq!(err.code(), "ValidationError", "case {case:?}: {err}");
            let msg = err.to_string();

            // EVERY pinned unsupported feature id is named (enumeration, not just the first).
            for id in expect["unsupported_feature_ids"].as_array().expect("unsupported ids") {
                let id = id.as_str().unwrap();
                assert!(
                    msg.contains(id),
                    "case {case:?}: refusal must enumerate {id:?}: {msg}"
                );
            }
            // Every pinned message substring (required-min / client-has phrasing).
            for needle in expect["message_contains"].as_array().expect("message_contains") {
                let needle = needle.as_str().unwrap();
                assert!(
                    msg.contains(needle),
                    "case {case:?}: refusal must contain {needle:?}: {msg}"
                );
            }

            // Live-wiring proof: a refused install stored NOTHING — a run on the
            // (never-installed) applet is rejected.
            assert!(
                !applet_runs(&mut core),
                "case {case:?}: a refused install must store nothing (no runnable applet)"
            );
        }
        other => panic!("case {case:?}: unknown decision {other:?}"),
    }
}

/// Whether the applet is installed + runnable: drive a real `runtime.run` and
/// report whether it succeeded. An uninstalled applet's run is a typed rejection,
/// so this is a live probe of "did the install actually store the applet?".
fn applet_runs(core: &mut WorkspaceCore) -> bool {
    core.handle(cmd(
        "runtime.run",
        serde_json::json!({ "input": { "mode": "boot" } }),
    ))
    .ok
}
