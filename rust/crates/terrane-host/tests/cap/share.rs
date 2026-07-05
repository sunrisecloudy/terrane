//! Host e2e checks for share invite composition and sync rights enforcement.

use tempfile::tempdir;
use terrane_core::Request;
use terrane_host::{open_at_home, share, sync};

#[test]
fn share_cli_invite_redeem_revoke_mirrors_auth_and_replays() {
    let dir = tempdir().unwrap();
    let mut core = open_at_home(dir.path()).unwrap();
    core.dispatch(Request::trusted_host(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();

    let invite = share::invite(&mut core, "notes", "write", "hello").unwrap();
    assert_eq!(invite.token.len(), terrane_cap_share::INVITE_TOKEN_BYTES * 2);
    assert!(!share::invites(&core, "notes").unwrap().contains(&invite.token));

    share::redeem(&mut core, "notes", &invite.token, "replica:abc").unwrap();
    assert!(share::list(&core, "notes").unwrap().contains("replica:abc"));
    assert!(terrane_cap_auth::namespace_granted(
        core.state(),
        &share::principal_for_grantee("replica:abc").unwrap(),
        "notes",
        "kv",
    )
    .unwrap());

    share::revoke(&mut core, "notes", "replica:abc").unwrap();
    assert!(!terrane_cap_auth::namespace_granted(
        core.state(),
        &share::principal_for_grantee("replica:abc").unwrap(),
        "notes",
        "kv",
    )
    .unwrap());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn sync_share_rights_are_read_pull_only_and_write_bidirectional() {
    let a_dir = tempdir().unwrap();
    let b_dir = tempdir().unwrap();
    let mut a = open_at_home(a_dir.path()).unwrap();
    let mut b = open_at_home(b_dir.path()).unwrap();

    for core in [&mut a, &mut b] {
        core.dispatch(Request::trusted_host(
            "app.add",
            vec!["notes".into(), "Notes".into()],
        ))
        .unwrap();
    }

    let b_peer = sync::local_peer_hex(&b).unwrap();
    let a_peer = sync::local_peer_hex(&a).unwrap();
    let b_grantee = share::grantee_for_peer(&b_peer).unwrap();
    sync::pair_peer(&mut a, &b_peer, "B").unwrap();
    sync::pair_peer(&mut b, &a_peer, "A").unwrap();

    a.dispatch(Request::trusted_host(
        "kv.set",
        vec!["notes".into(), "theme".into(), "dark".into()],
    ))
    .unwrap();
    b.dispatch(Request::trusted_host(
        "kv.set",
        vec!["notes".into(), "lang".into(), "en".into()],
    ))
    .unwrap();

    assert!(sync::event_batch_since_for_grantee(&a, "notes", &b_grantee, 0)
        .unwrap_err()
        .contains("missing read"));

    let read_invite = share::invite(&mut a, "notes", "read", "").unwrap();
    share::redeem(&mut a, "notes", &read_invite.token, &b_grantee).unwrap();
    let batch = sync::event_batch_since_for_grantee(&a, "notes", &b_grantee, 0).unwrap();
    sync::apply_event_batch(&mut b, "notes", &batch).unwrap();
    assert_eq!(b.state().kv.data["notes"]["theme"], "dark");

    let b_batch = sync::event_batch_since(&b, "notes", 0).unwrap();
    assert!(sync::apply_event_batch_for_grantee(&mut a, "notes", &b_grantee, &b_batch)
        .unwrap_err()
        .contains("missing write"));

    share::revoke(&mut a, "notes", &b_grantee).unwrap();
    let write_invite = share::invite(&mut a, "notes", "write", "").unwrap();
    share::redeem(&mut a, "notes", &write_invite.token, &b_grantee).unwrap();
    sync::apply_event_batch_for_grantee(&mut a, "notes", &b_grantee, &b_batch).unwrap();
    assert_eq!(a.state().kv.data["notes"]["lang"], "en");

    share::revoke(&mut a, "notes", &b_grantee).unwrap();
    assert!(sync::event_batch_since_for_grantee(&a, "notes", &b_grantee, 0)
        .unwrap_err()
        .contains("missing read"));
}
