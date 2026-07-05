//! Engine tests for the `app` capability (and core dispatch/routing).

use tempfile::tempdir;
use terrane_core::{Core, Effect, EffectRunner, EventRecord, Result};
use terrane_core::Error;

use crate::helpers::req;

struct UpgradeBatch {
    records: Vec<EventRecord>,
}

impl EffectRunner for UpgradeBatch {
    fn run(&self, effect: &Effect, _state: &terrane_core::State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::UpgradeAppBundle { id, source } => {
                assert_eq!(id, "notes");
                assert_eq!(source, "/tmp/notes-v2");
                Ok(self.records.clone())
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

#[test]
fn dispatches_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");

    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("app.add", &["tasks", "Task", "Workbench"]))
        .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();

    assert!(core.replay_matches().unwrap());
    assert_eq!(core.state().app.apps.len(), 1);
    assert!(core.state().app.apps.contains_key("tasks"));

    // A brand-new Core opened on the same log rebuilds the same world.
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn source_round_trips_through_the_log() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &["notes", "Notes", "--source", "apps/notes"],
    ))
    .unwrap();
    let reopened = Core::open(&log).unwrap();
    assert_eq!(
        reopened.state().app.apps["notes"].source.as_deref(),
        Some("apps/notes")
    );
}

#[test]
fn upgrade_effect_batch_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let batch = vec![
        terrane_cap_migration::applied_event(
            "notes",
            1,
            2,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        )
        .unwrap(),
        terrane_cap_app::upgraded_event(
            "notes",
            terrane_cap_app::DEFAULT_VERSION,
            "1.1.0",
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
        )
        .unwrap(),
        terrane_cap_kv::set_event("notes", "__terrane/app-bundle/main.js", "v2").unwrap(),
        terrane_cap_kv::delete_event("notes", "__terrane/app-bundle/old.js").unwrap(),
    ];
    let mut core = Core::open_with(&log, UpgradeBatch { records: batch }).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"]))
        .unwrap();
    core.dispatch(req("app.upgrade", &["notes", "/tmp/notes-v2"]))
        .unwrap();

    assert_eq!(core.state().app.apps["notes"].version, "1.1.0");
    assert_eq!(
        core.state().migration.apps["notes"].version,
        2,
        "migration event should fold before replay"
    );
    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn link_registrations_fold_and_replay() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &[
            "viewer",
            "Viewer",
            "--source",
            "apps/viewer",
            "--file-types",
            "txt:text/plain",
        ],
    ))
    .unwrap();

    let links = &core.state().app.apps["viewer"].links;
    assert!(links
        .iter()
        .any(|link| link.kind == "scheme-route" && link.spec == "terrane://open/viewer"));
    assert!(links
        .iter()
        .any(|link| link.kind == "filetype" && link.spec == "txt:text/plain"));
    assert!(core.replay_matches().unwrap());

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), core.state());
}

#[test]
fn rejects_duplicate_missing_and_unknown() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    assert_eq!(
        core.dispatch(req("app.add", &["notes", "Again"])),
        Err(Error::AppExists("notes".into()))
    );
    assert_eq!(
        core.dispatch(req("app.remove", &["ghost"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    // Unknown namespace and unknown verb are both rejected.
    assert!(matches!(
        core.dispatch(req("bogus.thing", &[])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.frobnicate", &["x"])),
        Err(Error::InvalidInput(_))
    ));

    assert_eq!(core.state().app.apps.len(), 1);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn rejects_empty_fields() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(matches!(
        core.dispatch(req("app.add", &["", "x"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.add", &["x"])),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn public_link_delivery_requires_trusted_host_authority() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let err = core
        .dispatch(terrane_core::Request::new(
            "app.link.deliver",
            vec!["demo".into(), "link".into(), "{}".into()],
        ))
        .unwrap_err()
        .to_string();
    assert!(err.contains("trusted host authority"), "{err}");
}

#[test]
fn rejects_reserved_or_unsafe_app_ids() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();

    assert!(matches!(
        core.dispatch(req("app.add", &["__terrane/auth", "Auth Shadow"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("app.add", &["bad/path", "Bad Path"])),
        Err(Error::InvalidInput(_))
    ));
    core.dispatch(req("app.add", &["safe_id-1", "Safe"]))
        .unwrap();
    assert!(core.state().app.apps.contains_key("safe_id-1"));
}
