//! Engine tests for the `auth` capability: grant/revoke state machine,
//! idempotency, the runtime read used by the resource gate, and replay
//! identity. Per the project rule these live here, not inline in
//! `terrane-cap-auth/src/lib.rs`.

use tempfile::tempdir;
use terrane_cap_auth::namespace_granted;
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
