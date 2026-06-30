//! Engine tests for the `auth` capability: grant/revoke state machine,
//! idempotency, the runtime read used by the resource gate, and replay
//! identity. Per the project rule these live here, not inline in
//! `terrane-cap-auth/src/lib.rs`.

use tempfile::tempdir;
use terrane_cap_auth::{agent_subject, auth_agents, namespace_granted, permission_request};
use terrane_core::{Core, ExecutionPrincipal, LOCAL_OWNER_SUBJECT};

use crate::helpers::req;

#[test]
fn grant_makes_namespace_granted_and_revoke_narrows() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    let principal = ExecutionPrincipal::local_owner();

    assert!(!namespace_granted(core.state(), &principal, "demo", "kv").unwrap());

    let events = core
        .dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    assert_eq!(events.len(), 1, "first grant records one fact");
    assert!(namespace_granted(core.state(), &principal, "demo", "kv").unwrap());

    let events = core
        .dispatch(req("auth.revoke", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    assert_eq!(events.len(), 1, "revoke of a live grant records one fact");
    assert!(!namespace_granted(core.state(), &principal, "demo", "kv").unwrap());

    assert!(
        core.replay_matches().unwrap(),
        "auth events replay identically"
    );
}

#[test]
fn grant_and_revoke_are_idempotent() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    let again = core
        .dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    assert!(again.is_empty(), "re-granting records no new fact");

    core.dispatch(req("auth.revoke", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    let again = core
        .dispatch(req("auth.revoke", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    assert!(
        again.is_empty(),
        "re-revoking a missing grant records no new fact"
    );
}

#[test]
fn unrelated_namespace_is_not_granted() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    let principal = ExecutionPrincipal::local_owner();

    // A grant for `kv` does not grant `crdt`.
    assert!(!namespace_granted(core.state(), &principal, "demo", "crdt").unwrap());
}

#[test]
fn grant_requires_existing_app() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(core
        .dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "ghost", "kv"]))
        .is_err());
}

#[test]
fn grants_are_cleaned_when_app_is_removed() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "demo", "kv"]))
        .unwrap();
    core.dispatch(req("app.remove", &["demo"])).unwrap();
    let principal = ExecutionPrincipal::local_owner();

    assert!(
        !namespace_granted(core.state(), &principal, "demo", "kv").unwrap(),
        "removing an app must drop its grants"
    );
}

#[test]
fn permission_request_records_pending_and_replays() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let events = core
        .dispatch(req(
            "auth.permission.request",
            &[
                "req-demo-kv",
                LOCAL_OWNER_SUBJECT,
                "demo",
                "invoke",
                "mcp",
                "kv",
            ],
        ))
        .unwrap();
    assert_eq!(events.len(), 1);
    let request = permission_request(core.state(), "req-demo-kv")
        .unwrap()
        .expect("pending request");
    assert_eq!(request.status, "pending");
    assert_eq!(request.resources[0].namespace, "kv");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn approve_request_records_grants_and_status() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "auth.permission.request",
        &[
            "req-demo-kv",
            LOCAL_OWNER_SUBJECT,
            "demo",
            "invoke",
            "web",
            "kv",
        ],
    ))
    .unwrap();

    let events = core
        .dispatch(req("auth.permission.approve", &["req-demo-kv", "ok"]))
        .unwrap();
    assert_eq!(events.len(), 2, "approve records grant plus decision");
    let principal = ExecutionPrincipal::local_owner();
    assert!(namespace_granted(core.state(), &principal, "demo", "kv").unwrap());
    let request = permission_request(core.state(), "req-demo-kv")
        .unwrap()
        .unwrap();
    assert_eq!(request.status, "approved");
    assert_eq!(request.decision_reason, "ok");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn deny_request_leaves_runtime_access_absent() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "auth.permission.request",
        &[
            "req-demo-kv",
            LOCAL_OWNER_SUBJECT,
            "demo",
            "invoke",
            "web",
            "kv",
        ],
    ))
    .unwrap();
    core.dispatch(req("auth.permission.deny", &["req-demo-kv", "no"]))
        .unwrap();

    let principal = ExecutionPrincipal::local_owner();
    assert!(!namespace_granted(core.state(), &principal, "demo", "kv").unwrap());
    let request = permission_request(core.state(), "req-demo-kv")
        .unwrap()
        .unwrap();
    assert_eq!(request.status, "denied");
}

#[test]
fn cancel_request_leaves_runtime_access_absent() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "auth.permission.request",
        &[
            "req-demo-kv",
            LOCAL_OWNER_SUBJECT,
            "demo",
            "invoke",
            "mcp",
            "kv",
        ],
    ))
    .unwrap();
    core.dispatch(req("auth.permission.cancel", &["req-demo-kv", "stale"]))
        .unwrap();

    let principal = ExecutionPrincipal::local_owner();
    assert!(!namespace_granted(core.state(), &principal, "demo", "kv").unwrap());
    let request = permission_request(core.state(), "req-demo-kv")
        .unwrap()
        .unwrap();
    assert_eq!(request.status, "cancelled");
}

#[test]
fn app_removed_cleans_permission_requests() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "auth.permission.request",
        &[
            "req-demo-kv",
            LOCAL_OWNER_SUBJECT,
            "demo",
            "invoke",
            "web",
            "kv",
        ],
    ))
    .unwrap();
    core.dispatch(req("app.remove", &["demo"])).unwrap();

    assert!(permission_request(core.state(), "req-demo-kv")
        .unwrap()
        .is_none());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn build_permission_request_records_read_only_verbs() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    core.dispatch(req(
        "auth.permission.request",
        &[
            "req-demo-build",
            LOCAL_OWNER_SUBJECT,
            "demo",
            "compile",
            "web",
            "build",
        ],
    ))
    .unwrap();

    let request = permission_request(core.state(), "req-demo-build")
        .unwrap()
        .unwrap();
    assert_eq!(request.resources[0].namespace, "build");
    assert_eq!(request.resources[0].verbs, vec!["read"]);
}

#[test]
fn agent_register_delegate_and_revoke_replay() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    let agent = agent_subject(LOCAL_OWNER_SUBJECT, "codex-local");

    let events = core
        .dispatch(req(
            "auth.agent.register",
            &[&agent, "Codex Local", LOCAL_OWNER_SUBJECT],
        ))
        .unwrap();
    assert_eq!(events.len(), 1);
    let agents = auth_agents(core.state()).unwrap();
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0].agent, agent);
    assert_eq!(agents[0].status, "active");
    assert!(agents[0].can_request_permissions);
    assert!(!agents[0].can_grant_permissions);

    let duplicate = core
        .dispatch(req(
            "auth.agent.register",
            &[&agent, "Codex Local", LOCAL_OWNER_SUBJECT],
        ))
        .unwrap();
    assert!(duplicate.is_empty(), "register is idempotent");

    let events = core
        .dispatch(req(
            "auth.agent.delegate",
            &[&agent, "operator", "false", "true", "false"],
        ))
        .unwrap();
    assert_eq!(events.len(), 1);
    let agents = auth_agents(core.state()).unwrap();
    assert_eq!(agents[0].max_role, "operator");
    assert!(!agents[0].can_install_apps);
    assert!(agents[0].can_request_permissions);

    let events = core.dispatch(req("auth.agent.revoke", &[&agent])).unwrap();
    assert_eq!(events.len(), 1);
    let agents = auth_agents(core.state()).unwrap();
    assert_eq!(agents[0].status, "revoked");
    assert!(core.replay_matches().unwrap());
}

#[test]
fn agent_grants_use_agent_subject() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    let agent = agent_subject(LOCAL_OWNER_SUBJECT, "codex-local");
    core.dispatch(req(
        "auth.agent.register",
        &[&agent, "Codex Local", LOCAL_OWNER_SUBJECT],
    ))
    .unwrap();
    core.dispatch(req("auth.grant", &[&agent, "demo", "kv"]))
        .unwrap();

    let principal = ExecutionPrincipal {
        org: "local".to_string(),
        subject: agent,
        source: "local".to_string(),
    };
    assert!(namespace_granted(core.state(), &principal, "demo", "kv").unwrap());
}

#[test]
fn revoked_agent_loses_grants_and_cannot_receive_new_grants() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    let agent = agent_subject(LOCAL_OWNER_SUBJECT, "codex-local");
    let principal = ExecutionPrincipal {
        org: "local".to_string(),
        subject: agent.clone(),
        source: "local".to_string(),
    };

    assert!(
        core.dispatch(req("auth.grant", &[&agent, "demo", "kv"]))
            .is_err(),
        "unregistered agents cannot receive grants"
    );

    core.dispatch(req(
        "auth.agent.register",
        &[&agent, "Codex Local", LOCAL_OWNER_SUBJECT],
    ))
    .unwrap();
    core.dispatch(req("auth.grant", &[&agent, "demo", "kv"]))
        .unwrap();
    assert!(namespace_granted(core.state(), &principal, "demo", "kv").unwrap());

    core.dispatch(req("auth.agent.revoke", &[&agent])).unwrap();
    assert!(
        !namespace_granted(core.state(), &principal, "demo", "kv").unwrap(),
        "revoked agents must not keep runtime access through old grants"
    );
    assert!(
        core.dispatch(req("auth.grant", &[&agent, "demo", "kv"]))
            .is_err(),
        "revoked agents cannot receive fresh grants"
    );
    assert!(core.replay_matches().unwrap());
}
