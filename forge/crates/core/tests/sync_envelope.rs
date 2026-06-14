//! Data-driven conformance over the SS-7 envelope-metadata well-formedness
//! vectors (`forge/fixtures/sync-envelope/`, review 092 #2).
//!
//! These vectors are ORTHOGONAL to RBAC: every case assumes a TRUSTED wildcard
//! owner, so the only thing that decides the outcome is whether the incoming
//! op's envelope metadata is WELL-FORMED. A well-formed envelope passes the
//! owner grant and is ALLOWED; a malformed one (missing collection / record id /
//! schema id / schema version, or an inconsistent resource_type-op pairing) is
//! DENIED fail-closed BEFORE any grant check, with the audit reason naming the
//! structural defect (`forge/spec/sync-rbac.md` line 90, SS-7 resource gate).
//!
//! The `ran == manifest.count` guard means a missing/renamed vector FAILS the
//! test rather than silently skipping, so the corpus stays load-bearing.

use forge_core::{authorize_remote_op, RemoteOp, RemoteOpEnvelope, ResourceType, TrustedMembership};
use forge_domain::Role;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/core
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sync-envelope")
        .canonicalize()
        .expect("sync-envelope fixtures dir exists")
}

/// The trusted authority for EVERY vector: a wildcard owner with schema_write, so
/// the grant/role checks always pass and only envelope well-formedness can deny.
fn wildcard_owner() -> TrustedMembership {
    TrustedMembership {
        actor_id: "actor-owner".into(),
        role: Role::Owner,
        db_read: vec!["*".into()],
        db_write: vec!["*".into()],
        schema_write: true,
    }
}

fn parse_op(s: &str) -> RemoteOp {
    match s {
        "insert" => RemoteOp::Insert,
        "patch" => RemoteOp::Patch,
        "delete" => RemoteOp::Delete,
        "schema_change" => RemoteOp::SchemaChange,
        "read" => RemoteOp::Read,
        other => panic!("unknown op {other:?}"),
    }
}

fn parse_envelope(env: &Value) -> RemoteOpEnvelope {
    let resource_type = match env["resource_type"].as_str().expect("resource_type") {
        "record" => ResourceType::Record,
        "schema" => ResourceType::Schema,
        other => panic!("unknown resource_type {other:?}"),
    };
    RemoteOpEnvelope {
        resource_type,
        op: parse_op(env["op"].as_str().expect("op")),
        collection: env.get("collection").and_then(|v| v.as_str()).map(String::from),
        // Fixtures spell a single `record_id`; the in-code envelope carries a list
        // (`review 093`), so wrap a present id into a one-element list (absent =>
        // empty list, which the metadata gate denies for a record write).
        record_ids: env
            .get("record_id")
            .and_then(|v| v.as_str())
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
        schema_id: env.get("schema_id").and_then(|v| v.as_str()).map(String::from),
        schema_version: env.get("schema_version").and_then(|v| v.as_u64()),
        // Not a migration unless a vector explicitly flags it; these envelope vectors
        // model plain record / schema ops, so default `false` (review 143).
        is_migration: env.get("is_migration").and_then(|v| v.as_bool()).unwrap_or(false),
    }
}

#[derive(serde::Deserialize)]
struct Manifest {
    count: usize,
}

#[test]
fn every_sync_envelope_vector_matches_expected_wellformedness() {
    let dir = fixtures_dir();
    let manifest: Manifest = serde_json::from_str(
        &std::fs::read_to_string(dir.join("manifest.json")).expect("read envelope manifest"),
    )
    .expect("parse envelope manifest");

    let trusted = wildcard_owner();
    let mut ran = 0usize;

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read sync-envelope dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|p| p.file_name().and_then(|n| n.to_str()) != Some("manifest.json"))
        .collect();
    entries.sort();

    for path in entries {
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let fx: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        let case = fx["case"].as_str().unwrap_or("<no-case>").to_string();

        let env = parse_envelope(&fx["envelope"]);
        // The trusted owner is the SAME for every vector; only the envelope varies,
        // so an allow proves well-formedness and a deny proves a structural defect.
        let decision = authorize_remote_op(&trusted, None, &env);

        let expect = &fx["expect"];
        let want_well_formed = expect["well_formed"].as_bool().expect("expect.well_formed");
        // A well-formed envelope is allowed by the wildcard owner; a malformed one is
        // denied — so well-formedness is exactly the allow/deny outcome here.
        assert_eq!(
            decision.is_allow(),
            want_well_formed,
            "[{case}] well-formedness mismatch: got decision {decision:?}"
        );

        match expect["decision"].as_str().expect("expect.decision") {
            "allowed" => assert!(decision.is_allow(), "[{case}] expected allow"),
            "permission_denied" => {
                assert!(!decision.is_allow(), "[{case}] expected deny");
                // A malformed envelope names the structural defect in BOTH the decision
                // reason and the persisted audit row.
                let reason_contains = expect["reason_contains"]
                    .as_str()
                    .expect("a denied vector pins reason_contains");
                assert!(
                    decision.reason().contains(reason_contains)
                        && decision.audit().reason.contains(reason_contains),
                    "[{case}] reason {:?} does not contain {reason_contains:?}",
                    decision.reason()
                );
                assert_eq!(decision.audit().decision, "deny", "[{case}] audit decision");
            }
            other => panic!("[{case}] unknown expect.decision {other:?}"),
        }

        ran += 1;
    }

    assert_eq!(
        ran, manifest.count,
        "ran {ran} sync-envelope vectors but the manifest declares {}",
        manifest.count
    );
}
