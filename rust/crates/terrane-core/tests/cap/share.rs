//! Engine tests for the `share` capability: invite/redeem/revoke folds,
//! hash-only descriptions, app cleanup, validation, and replay identity.

use tempfile::tempdir;
use terrane_core::{Core, QueryValue};

use crate::helpers::req;

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn invite_redeem_revoke_are_replayable_and_hash_only() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    let invited = core
        .dispatch(req("share.invite", &["notes", "write", "friend", HASH]))
        .unwrap();
    assert_eq!(invited[0].kind, "share.invited");
    let share_line = core
        .log_lines()
        .unwrap()
        .into_iter()
        .find(|line| line.contains("share.invited"))
        .unwrap();
    assert_eq!(share_line, "user:local-owner share.invited notes write");
    assert!(!share_line.contains(HASH));

    core.dispatch(req("share.redeem", &["notes", HASH, "replica:abc"]))
        .unwrap();
    let shares = core.query("share", "list", &["notes".into()]).unwrap();
    assert!(matches!(shares, QueryValue::Json(json) if json.contains("\"grantee\":\"replica:abc\"") && json.contains("\"rights\":\"write\"")));

    let err = core
        .dispatch(req("share.redeem", &["notes", HASH, "replica:def"]))
        .unwrap_err();
    assert!(
        err.to_string().contains("already redeemed"),
        "unexpected error: {err}"
    );

    core.dispatch(req("share.revoke", &["notes", "replica:abc"]))
        .unwrap();
    assert_eq!(
        core.query("share", "list", &["notes".into()]).unwrap(),
        QueryValue::Json("[]".to_string())
    );
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log)
            .unwrap()
            .query("share", "list", &["notes".into()])
            .unwrap(),
        QueryValue::Json("[]".to_string())
    );
}

#[test]
fn validates_inputs_and_clears_app_on_remove() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    assert!(core
        .dispatch(req("share.invite", &["missing", "read", "", HASH]))
        .unwrap_err()
        .to_string()
        .contains("app not found"));
    assert!(core
        .dispatch(req("share.invite", &["notes", "admin", "", HASH]))
        .unwrap_err()
        .to_string()
        .contains("rights"));
    assert!(core
        .dispatch(req("share.invite", &["notes", "read", "", "bad"]))
        .unwrap_err()
        .to_string()
        .contains("token_hash"));
    assert!(core
        .dispatch(req("share.redeem", &["notes", HASH, "replica:not-hex"]))
        .unwrap_err()
        .to_string()
        .contains("replica grantee"));

    core.dispatch(req("share.invite", &["notes", "read", "", HASH]))
        .unwrap();
    core.dispatch(req("share.redeem", &["notes", HASH, "user:friend"]))
        .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert_eq!(
        core.query("share", "list", &["notes".into()]).unwrap(),
        QueryValue::Json("[]".to_string())
    );
    assert_eq!(
        core.query("share", "invites", &["notes".into()]).unwrap(),
        QueryValue::Json("[]".to_string())
    );
}
