use tempfile::tempdir;
use terrane_core::{Core, Effect, EffectRunner, Error, EventRecord, Result, State};

use crate::helpers::req;

#[derive(Clone, Copy)]
struct BlobRunner;

impl EffectRunner for BlobRunner {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::BlobStore {
                app,
                name,
                mime,
                hash,
                bytes,
            } => Ok(vec![terrane_cap_blob::stored_event(
                app,
                name,
                hash,
                u64::try_from(bytes.len())
                    .map_err(|_| Error::Storage("blob byte length overflow".into()))?,
                mime,
            )?]),
            other => Err(Error::InvalidInput(format!("unexpected effect: {other:?}"))),
        }
    }
}

#[test]
fn compaction_reopens_to_same_state_and_retains_tail_identity() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, BlobRunner).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req("kv.set", &["demo", "a", "one"])).unwrap();
    core.dispatch(req("blob.put", &["demo", "asset.txt", "text/plain", "aGVsbG8="]))
        .unwrap();
    core.dispatch(req("kv.set", &["demo", "b", "two"])).unwrap();
    let before = core.state().clone();
    let original = core.log_records().unwrap();
    drop(core);

    let report = terrane_core::compact_log(
        &log,
        terrane_core::CompactionOptions {
            retain: 1,
            prune_archive: false,
        },
    )
    .unwrap();

    assert_eq!(report.archived_records, original.len());
    assert_eq!(report.retained_records, 1);
    assert!(dir.path().join("snapshot.bin").exists());
    assert!(dir.path().join("log.bin.archive").exists());
    assert_eq!(terrane_core::read_log(&log).unwrap(), original[original.len() - 1..]);
    let reopened = Core::open_with(&log, BlobRunner).unwrap();
    assert_eq!(reopened.state(), &before);
    assert!(reopened.replay_matches().unwrap());
    let hash = &before.blob.blobs["demo"]["asset.txt"].hash;
    assert_eq!(reopened.state().blob.refs[hash], 1);
}

#[test]
fn compaction_with_zero_retain_replays_from_snapshot_only() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req("kv.set", &["demo", "a", "one"])).unwrap();
    let before = core.state().clone();
    drop(core);

    terrane_core::compact_log(
        &log,
        terrane_core::CompactionOptions {
            retain: 0,
            prune_archive: false,
        },
    )
    .unwrap();

    assert!(terrane_core::read_log(&log).unwrap().is_empty());
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), &before);
    assert!(reopened.replay_matches().unwrap());
}

#[test]
fn unknown_snapshot_section_is_a_storage_error() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    drop(core);
    let log_bytes = std::fs::read(&log).unwrap();
    terrane_core::snapshot::write_snapshot(
        &dir.path().join("snapshot.bin"),
        1,
        terrane_core::snapshot::hash_bytes(&log_bytes),
        vec![terrane_core::snapshot::SnapshotSection {
            namespace: "newer-cap".into(),
            payload: vec![1, 2, 3],
        }],
    )
    .unwrap();
    std::fs::write(&log, b"TRNLOG\x01\n").unwrap();

    let err = match Core::open(&log) {
        Ok(_) => panic!("open should reject unknown snapshot namespace"),
        Err(err) => err,
    };
    assert!(matches!(err, Error::Storage(message) if message.contains("newer-cap")));
}

#[test]
fn leftover_tmp_files_are_ignored_on_open() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    let before = core.state().clone();
    drop(core);
    std::fs::write(dir.path().join("snapshot.bin.tmp"), b"not a snapshot").unwrap();
    std::fs::write(dir.path().join("log.bin.tmp"), b"not a log").unwrap();

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state(), &before);
    assert!(reopened.replay_matches().unwrap());
}

#[test]
fn crdt_snapshot_preserves_version_vector_delta_export() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req("crdt.mapSet", &["notes", "prefs", "theme", "dark"]))
        .unwrap();
    let peer_vv = terrane_cap_crdt::crdt_vv(core.state(), "notes");
    core.dispatch(req("crdt.mapSet", &["notes", "prefs", "lang", "en"]))
        .unwrap();
    let before_delta =
        terrane_cap_crdt::crdt_export_from_vv(core.state(), "notes", &peer_vv).unwrap();
    drop(core);

    terrane_core::compact_log(
        &log,
        terrane_core::CompactionOptions {
            retain: 0,
            prune_archive: false,
        },
    )
    .unwrap();
    let reopened = Core::open(&log).unwrap();
    let after_delta =
        terrane_cap_crdt::crdt_export_from_vv(reopened.state(), "notes", &peer_vv).unwrap();
    assert_eq!(after_delta, before_delta);
    assert!(reopened.replay_matches().unwrap());
}
