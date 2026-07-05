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
fn blob_put_records_metadata_only_and_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, BlobRunner).unwrap();
    core.dispatch(req("app.add", &["gallery", "Gallery"]))
        .unwrap();

    let records = core
        .dispatch(req(
            "blob.put",
            &["gallery", "images/a.txt", "text/plain", "aGVsbG8="],
        ))
        .unwrap();

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "blob.stored");
    assert!(!records[0]
        .payload
        .windows(b"hello".len())
        .any(|w| w == b"hello"));
    let meta = &core.state().blob.blobs["gallery"]["images/a.txt"];
    assert_eq!(
        meta.hash,
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
    );
    assert_eq!(meta.size, 5);
    assert_eq!(core.state().blob.refs[&meta.hash], 1);
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().blob, core.state().blob);
}

#[test]
fn blob_rm_and_app_removed_update_refcounts() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, BlobRunner).unwrap();
    core.dispatch(req("app.add", &["gallery", "Gallery"]))
        .unwrap();
    core.dispatch(req(
        "blob.put",
        &["gallery", "a.txt", "text/plain", "c2FtZQ=="],
    ))
    .unwrap();
    core.dispatch(req(
        "blob.put",
        &["gallery", "b.txt", "text/plain", "c2FtZQ=="],
    ))
    .unwrap();
    let hash = core.state().blob.blobs["gallery"]["a.txt"].hash.clone();
    assert_eq!(core.state().blob.refs[&hash], 2);

    core.dispatch(req("blob.rm", &["gallery", "a.txt"]))
        .unwrap();
    assert_eq!(core.state().blob.refs[&hash], 1);
    assert!(!core.state().blob.blobs["gallery"].contains_key("a.txt"));

    core.dispatch(req("app.remove", &["gallery"])).unwrap();
    assert_eq!(core.state().blob.refs[&hash], 0);
    assert!(core.state().blob.blobs.is_empty());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn blob_link_records_metadata_without_cas_presence_check() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["gallery", "Gallery"]))
        .unwrap();

    core.dispatch(req(
        "blob.link",
        &[
            "gallery",
            "remote.bin",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            "12",
            "application/octet-stream",
        ],
    ))
    .unwrap();

    assert_eq!(core.state().blob.blobs["gallery"]["remote.bin"].size, 12);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn blob_validation_errors_are_typed() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, BlobRunner).unwrap();
    core.dispatch(req("app.add", &["gallery", "Gallery"]))
        .unwrap();

    assert_eq!(
        core.dispatch(req("blob.put", &["ghost", "a", "text/plain", "aGVsbG8="])),
        Err(Error::AppNotFound("ghost".into()))
    );
    assert!(matches!(
        core.dispatch(req("blob.put", &["gallery", "", "text/plain", "aGVsbG8="])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "blob.put",
            &["gallery", "a", "text/plain", "not base64"]
        )),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "blob.link",
            &["gallery", "a", "ABC", "1", "text/plain"]
        )),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        core.dispatch(req("blob.rm", &["gallery", "missing"])),
        Err(Error::KeyNotFound("gallery".into(), "missing".into()))
    );
}
