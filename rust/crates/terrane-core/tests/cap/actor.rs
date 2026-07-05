//! Actor provenance tests for the event envelope and log migration.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use borsh::{BorshDeserialize, BorshSerialize};
use tempfile::tempdir;
use terrane_cap_net::fetched_event;
use terrane_core::{
    migrate_log, read_log, Core, Effect, EffectRunner, Error, EventRecord, ExecutionPrincipal,
    Request, Result, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

#[derive(BorshSerialize, BorshDeserialize)]
struct LegacyEventRecord {
    kind: String,
    payload: Vec<u8>,
}

struct BogusActorRunner;

impl EffectRunner for BogusActorRunner {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                let mut record = fetched_event(app, url, 200, "ok".to_string())?;
                record.actor = "agent:forged:evil".to_string();
                Ok(vec![record])
            }
            other => Err(Error::InvalidInput(format!("unexpected effect: {other:?}"))),
        }
    }
}

fn principal(subject: &str) -> ExecutionPrincipal {
    ExecutionPrincipal {
        org: "local".to_string(),
        subject: subject.to_string(),
        source: "test".to_string(),
    }
}

fn trusted(name: &str, args: &[&str], subject: &str) -> Request {
    Request::trusted_host(name, args.iter().map(|arg| (*arg).to_string()).collect())
        .with_principal(principal(subject))
}

fn write_legacy_log(path: &Path, records: &[EventRecord]) {
    let mut file = fs::File::create(path).unwrap();
    for record in records {
        let legacy = LegacyEventRecord {
            kind: record.kind.clone(),
            payload: record.payload.clone(),
        };
        let bytes = borsh::to_vec(&legacy).unwrap();
        let len = u32::try_from(bytes.len()).unwrap();
        file.write_all(&len.to_le_bytes()).unwrap();
        file.write_all(&bytes).unwrap();
    }
}

#[test]
fn commit_stamps_actor_for_user_agent_and_app_principals() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    let user = core
        .dispatch(trusted(
            "app.add",
            &["user-app", "User App"],
            "user:local-owner",
        ))
        .unwrap();
    let agent = core
        .dispatch(trusted(
            "app.add",
            &["agent-app", "Agent App"],
            "agent:user-1:agent-7",
        ))
        .unwrap();
    let app = core
        .dispatch(trusted(
            "app.add",
            &["caller-app", "Caller App"],
            "app:calendar",
        ))
        .unwrap();

    assert_eq!(user[0].actor, "user:local-owner");
    assert_eq!(agent[0].actor, "agent:user-1:agent-7");
    assert_eq!(app[0].actor, "app:calendar");

    let stored = read_log(&log).unwrap();
    assert_eq!(stored[0].actor, "user:local-owner");
    assert_eq!(stored[1].actor, "agent:user-1:agent-7");
    assert_eq!(stored[2].actor, "app:calendar");
}

#[test]
fn capability_supplied_actor_is_overwritten_at_commit() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, BogusActorRunner).unwrap();
    core.dispatch(req("app.add", &["web", "Web"])).unwrap();

    let records = core
        .dispatch(trusted(
            "net.fetch",
            &["web", "https://example.test"],
            "agent:owner:truth",
        ))
        .unwrap();

    assert_eq!(records[0].kind, "net.fetched");
    assert_eq!(records[0].actor, "agent:owner:truth");
    assert_eq!(read_log(&log).unwrap()[1].actor, "agent:owner:truth");
}

#[test]
fn broadcast_fold_cascade_keeps_the_triggering_actor() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("kv.set", &["notes", "theme", "dark"]))
        .unwrap();

    let records = core
        .dispatch(trusted("app.remove", &["notes"], "agent:owner:cleanup"))
        .unwrap();

    assert_eq!(records[0].kind, "app.removed");
    assert_eq!(records[0].actor, "agent:owner:cleanup");
    assert!(core.state().kv.data.is_empty());
    assert!(Core::open(&log).unwrap().state().kv.data.is_empty());
}

#[test]
fn migrate_log_round_trips_legacy_records_and_keeps_backup() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut old = terrane_cap_app::added_event("notes", "Notes", None, "js").unwrap();
    old.actor = "ignored-before-migration".to_string();
    write_legacy_log(&log, &[old]);

    let count = migrate_log(&log).unwrap();

    assert_eq!(count, 1);
    assert!(dir.path().join("log.bin.pre-actor").is_file());
    let records = read_log(&log).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].actor, LOCAL_OWNER_SUBJECT);
    let core = Core::open(&log).unwrap();
    assert!(core.state().app.apps.contains_key("notes"));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn old_format_log_open_is_refused_with_migrate_log_guidance() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let record = terrane_cap_app::added_event("notes", "Notes", None, "js").unwrap();
    write_legacy_log(&log, &[record]);

    let err = match Core::open(&log) {
        Ok(_) => panic!("old-format log should be refused"),
        Err(err) => err,
    };

    match err {
        Error::Storage(msg) => assert!(msg.contains("migrate-log"), "{msg}"),
        other => panic!("expected storage error, got {other:?}"),
    }
}

#[test]
fn describe_output_includes_the_actor_prefix() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(trusted("app.add", &["notes", "Notes"], "agent:owner:audit"))
        .unwrap();

    let lines = core.log_lines().unwrap();

    assert!(lines[0].starts_with("agent:owner:audit "), "{:?}", lines);
    assert!(lines[0].contains("app.added"), "{:?}", lines);
}
