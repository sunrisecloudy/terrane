//! e2e for `org` — drive the real host edge through the org capability and prove
//! replay identity holds for create / invite / join / role.set / leave over the
//! real secret store.

use sha2::{Digest, Sha256};
use tempfile::tempdir;
use terrane_host::{open_at_home, dispatch_on_core};
use terrane_cap_interface::QueryValue;

/// Build `Vec<String>` from `&[&str]` to satisfy `dispatch_on_core`/`query`
/// argument signatures without `&[x.clone()]` slice patterns.
fn s(values: &[&str]) -> Vec<String> {
    values.iter().copied().map(String::from).collect()
}

fn token_hash(token: &str) -> String {
    let mut out = String::with_capacity(64);
    for byte in Sha256::digest(token.as_bytes()) {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[test]
fn org_create_invite_join_role_set_leave_replays_through_real_edge() {
    let dir = tempdir().unwrap();
    let mut core = open_at_home(dir.path()).unwrap();

    let founder_id = match core.query("person", "whoami", &[]).unwrap() {
        QueryValue::Json(json) if json.contains("person_id") => {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            value["person_id"].as_str().unwrap().to_string()
        }
        other => panic!("person.whoami returned unexpected value: {other:?}"),
    };
    assert!(!founder_id.is_empty());

    let create_records = dispatch_on_core(&mut core, "org.create", &s(&[founder_id.as_str()])).unwrap();
    assert_eq!(create_records.records.len(), 2);
    assert_eq!(create_records.records[0].kind, "org.created");
    assert_eq!(create_records.records[1].kind, "org.member.granted");

    let org_id = core.state().org.orgs.keys().next().cloned().unwrap();
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].role,
        "owner"
    );
    assert!(core.state().org.members[&(org_id.clone(), founder_id.clone())].active);

    // info / members queries return the org + founder.
    let QueryValue::Json(info) = core.query("org", "info", &[]).unwrap() else {
        panic!("org.info should return JSON");
    };
    assert!(info.contains(&org_id));
    let members_args = s(&[org_id.as_str()]);
    let QueryValue::Json(members) = core.query("org", "members", &members_args).unwrap() else {
        panic!("org.members should return JSON");
    };
    assert!(members.contains(&founder_id));

    // Open an admin invite; the host helper mints the token, hashes it, records
    // org.invited. We mint the token here so we can redeem it next.
    let invite_token = terrane_host::edge::mint_invite_token().unwrap();
    let hash = token_hash(&invite_token);
    dispatch_on_core(
        &mut core,
        "org.invite",
        &s(&[org_id.as_str(), "admin", hash.as_str(), "promote me"]),
    )
    .unwrap();
    assert!(core.state().org.invites[&(org_id.clone(), hash.clone())].open);

    // The founder redeems the invite; the join self-signs with the founder's
    // person key at the edge and records both member.granted and invite.redeemed.
    let join_records = dispatch_on_core(
        &mut core,
        "org.join",
        &s(&[org_id.as_str(), hash.as_str(), founder_id.as_str()]),
    )
    .unwrap();
    assert_eq!(join_records.records.len(), 2);
    assert_eq!(join_records.records[0].kind, "org.member.granted");
    assert_eq!(join_records.records[1].kind, "org.invite.redeemed");
    assert!(!core.state().org.invites[&(org_id.clone(), hash)].open);
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].role,
        "admin"
    );
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].signer,
        founder_id
    );

    // The admin restores the owner role via role.set, signing with their person key.
    dispatch_on_core(
        &mut core,
        "org.role.set",
        &s(&[org_id.as_str(), founder_id.as_str(), "owner", founder_id.as_str()]),
    )
    .unwrap();
    assert_eq!(
        core.state().org.members[&(org_id.clone(), founder_id.clone())].role,
        "owner"
    );

    // Leave marks the membership inactive.
    dispatch_on_core(&mut core, "org.leave", &s(&[org_id.as_str(), founder_id.as_str()])).unwrap();
    let last_active = core
        .state()
        .org
        .members
        .values()
        .next()
        .map(|member| member.active)
        .unwrap_or(true);
    assert!(!last_active);

    // Replay identity: re-reading the log must rebuild byte-for-byte equal state.
    assert!(core.replay_matches().unwrap());
}

#[test]
fn org_create_is_idempotent_when_called_twice_for_the_same_founder() {
    let dir = tempdir().unwrap();
    let mut core = open_at_home(dir.path()).unwrap();
    let founder_id = match core.query("person", "whoami", &[]).unwrap() {
        QueryValue::Json(json) if json.contains("person_id") => {
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();
            value["person_id"].as_str().unwrap().to_string()
        }
        other => panic!("person.whoami returned unexpected value: {other:?}"),
    };

    let first = dispatch_on_core(&mut core, "org.create", &s(&[founder_id.as_str()])).unwrap();
    let count_before = core.state().org.orgs.len();
    let second = dispatch_on_core(&mut core, "org.create", &s(&[founder_id.as_str()])).unwrap();
    // The cap declaration treats "founder already owns an org" as a no-op
    // (empty commit), so no new events are recorded on the second call.
    assert!(first.records.iter().any(|record| record.kind == "org.created"));
    assert!(second.records.iter().all(|record| record.kind != "org.created"));
    assert_eq!(core.state().org.orgs.len(), count_before);
}