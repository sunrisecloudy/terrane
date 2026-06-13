//! Data-driven conformance over the SS-7 sync-RBAC vectors
//! (`forge/fixtures/sync-rbac/`, manifest `count = 13`).
//!
//! Each fixture pins one incoming remote op plus the receiver's expected
//! decision and audit record. The test parses `trusted_peer` / `incoming_claim`
//! / `incoming` into the pure-decision types, calls
//! [`forge_core::authorize_remote_op`], and asserts the decision and audit match
//! `expect` (`forge/spec/sync-rbac.md`). The fixtures are load-bearing: a wrong
//! role-matrix mapping, a mishandled wildcard grant, or a missed self-escalation
//! fails here. The `ran == 10` guard means a missing/misnamed fixture FAILS the
//! test rather than silently skipping.

use forge_core::{
    authorize_remote_op, IncomingClaim, RemoteOp, RemoteOpEnvelope, ResourceType,
    SyncAuthDecision, TrustedMembership,
};
use forge_domain::Role;
use serde_json::Value;
use std::path::{Path, PathBuf};

fn fixtures_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR = forge/crates/core
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/sync-rbac")
        .canonicalize()
        .expect("sync-rbac fixtures dir exists")
}

fn parse_role(s: &str) -> Role {
    match s {
        "owner" => Role::Owner,
        "maintainer" => Role::Maintainer,
        "editor" => Role::Editor,
        "runner" => Role::Runner,
        "viewer" => Role::Viewer,
        "auditor" => Role::Auditor,
        "reviewer" => Role::Reviewer,
        other => panic!("unknown role {other:?}"),
    }
}

fn parse_str_list(v: &Value) -> Vec<String> {
    v.as_array()
        .expect("grant list is an array")
        .iter()
        .map(|e| e.as_str().expect("grant entry is a string").to_string())
        .collect()
}

fn parse_membership(v: &Value) -> TrustedMembership {
    TrustedMembership {
        actor_id: v["actor_id"].as_str().expect("actor_id").to_string(),
        role: parse_role(v["role"].as_str().expect("role")),
        db_read: parse_str_list(&v["db_read"]),
        db_write: parse_str_list(&v["db_write"]),
        schema_write: v["schema_write"].as_bool().expect("schema_write"),
    }
}

fn parse_claim(v: &Value) -> IncomingClaim {
    IncomingClaim {
        actor_id: v["actor_id"].as_str().expect("actor_id").to_string(),
        role: parse_role(v["role"].as_str().expect("role")),
        db_read: parse_str_list(&v["db_read"]),
        db_write: parse_str_list(&v["db_write"]),
        schema_write: v["schema_write"].as_bool().expect("schema_write"),
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

fn parse_envelope(incoming: &Value) -> RemoteOpEnvelope {
    let meta = &incoming["metadata"];
    let resource_type = match meta["resource_type"].as_str().expect("resource_type") {
        "record" => ResourceType::Record,
        "schema" => ResourceType::Schema,
        other => panic!("unknown resource_type {other:?}"),
    };
    let schema_version = meta
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .or_else(|| meta.get("to_schema_version").and_then(|v| v.as_u64()));
    RemoteOpEnvelope {
        resource_type,
        op: parse_op(meta["op"].as_str().expect("op")),
        collection: meta.get("collection").and_then(|v| v.as_str()).map(String::from),
        record_id: meta.get("record_id").and_then(|v| v.as_str()).map(String::from),
        schema_id: meta.get("schema_id").and_then(|v| v.as_str()).map(String::from),
        schema_version,
    }
}

/// All 13 vectors (manifest.json is excluded). The harness loads every file in
/// the directory and asserts the count, so a renamed/dropped fixture fails.
#[test]
fn sync_rbac_vectors_match_expected_decision_and_audit() {
    let dir = fixtures_dir();
    let mut ran = 0usize;

    let mut entries: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read sync-rbac dir")
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

        let trusted = parse_membership(&fx["trusted_peer"]);
        let claim = fx.get("incoming_claim").map(parse_claim);
        let env = parse_envelope(&fx["incoming"]);

        let decision = authorize_remote_op(&trusted, claim.as_ref(), &env);

        // expect.decision: "applied" -> allow, "rejected"/permission_denied -> deny.
        let expect = &fx["expect"];
        let want_allow = match expect["decision"].as_str().expect("expect.decision") {
            "applied" => true,
            "rejected" | "permission_denied" => false,
            other => panic!("[{case}] unknown expect.decision {other:?}"),
        };
        assert_eq!(
            decision.is_allow(),
            want_allow,
            "[{case}] decision mismatch: got {decision:?}"
        );

        // expect.audit.{action, decision, reason_contains}.
        let audit = decision.audit();
        let want_action = expect["audit"]["action"].as_str().expect("audit.action");
        assert_eq!(audit.action, want_action, "[{case}] audit.action");

        let want_audit_decision =
            expect["audit"]["decision"].as_str().expect("audit.decision");
        assert_eq!(
            audit.decision, want_audit_decision,
            "[{case}] audit.decision"
        );
        // The decision-level allow/deny and audit decision agree.
        let expected_audit_decision = if want_allow { "allow" } else { "deny" };
        assert_eq!(
            audit.decision, expected_audit_decision,
            "[{case}] audit decision matches applied/rejected"
        );

        let reason_contains =
            expect["audit"]["reason_contains"].as_str().expect("reason_contains");
        assert!(
            decision.reason().contains(reason_contains)
                && audit.reason.contains(reason_contains),
            "[{case}] reason {:?} does not contain {reason_contains:?}",
            decision.reason()
        );

        // Cross-check that allow/deny carries the right variant for clarity.
        match (&decision, want_allow) {
            (SyncAuthDecision::Allow { .. }, true) => {}
            (SyncAuthDecision::Deny { .. }, false) => {}
            _ => panic!("[{case}] decision variant mismatch"),
        }

        ran += 1;
    }

    assert_eq!(ran, 13, "expected exactly 13 sync-rbac vectors, ran {ran}");
}
