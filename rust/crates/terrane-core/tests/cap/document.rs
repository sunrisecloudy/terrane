//! Engine tests for the `document` capability.

use serde_json::json;
use tempfile::tempdir;
use terrane_core::{Core, Error};

use crate::helpers::req;

#[test]
fn document_create_patch_append_delete_replays_identically() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    let created = core
        .dispatch(req(
            "document.create",
            &[
                "notes",
                "daily-1",
                "Daily",
                "# Daily\n",
                r#"{"tags":["work"],"draft":true}"#,
            ],
        ))
        .unwrap();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].kind, "document.created");

    core.dispatch(req(
        "document.patch",
        &[
            "notes",
            "daily-1",
            r#"{"title":"Daily updated","metadata":{"draft":null,"status":"ready"}}"#,
        ],
    ))
    .unwrap();
    core.dispatch(req(
        "document.append",
        &["notes", "daily-1", "\nDone."],
    ))
    .unwrap();

    let doc = &core.state().document.docs["notes"]["daily-1"];
    assert_eq!(doc.title, "Daily updated");
    assert_eq!(doc.body, "# Daily\n\nDone.");
    let metadata: serde_json::Value = serde_json::from_str(&doc.metadata_json).unwrap();
    assert_eq!(
        metadata,
        json!({"status":"ready","tags":["work"]})
    );
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().document,
        core.state().document
    );

    let deleted = core
        .dispatch(req("document.delete", &["notes", "daily-1"]))
        .unwrap();
    assert_eq!(deleted.len(), 1);
    assert_eq!(deleted[0].kind, "document.deleted");
    assert!(core.state().document.docs.is_empty());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn document_create_replaces_and_delete_missing_is_noop() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    core.dispatch(req(
        "document.create",
        &["notes", "same", "First", "one", r#"{"v":1}"#],
    ))
    .unwrap();
    core.dispatch(req(
        "document.create",
        &["notes", "same", "Second", "two", r#"{"v":2}"#],
    ))
    .unwrap();

    let doc = &core.state().document.docs["notes"]["same"];
    assert_eq!(doc.title, "Second");
    assert_eq!(doc.body, "two");
    assert_eq!(doc.metadata_json, r#"{"v":2}"#);

    let events = core
        .dispatch(req("document.delete", &["notes", "missing"]))
        .unwrap();
    assert!(events.is_empty());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn document_validation_errors_are_typed() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    assert!(matches!(
        core.dispatch(req(
            "document.create",
            &["notes", "-bad", "Bad", "", "{}"]
        )),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "document.create",
            &["notes", "bad-meta", "Bad", "", "[]"]
        )),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req("document.patch", &["notes", "missing", "{}"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "document.create",
            &["notes", "too-big", "Big", &"x".repeat(1_048_577), "{}"]
        )),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        core.dispatch(req("document.create", &["ghost", "id", "Title", "", "{}"])),
        Err(Error::AppNotFound("ghost".into()))
    );
}

#[test]
fn document_quota_is_enforced() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();

    for i in 0..10_000 {
        let id = format!("doc-{i}");
        core.dispatch(req(
            "document.create",
            &["notes", id.as_str(), "Title", "", "{}"],
        ))
        .unwrap();
    }
    assert!(matches!(
        core.dispatch(req(
            "document.create",
            &["notes", "doc-over", "Title", "", "{}"]
        )),
        Err(Error::InvalidInput(_))
    ));
    core.dispatch(req(
        "document.create",
        &["notes", "doc-1", "Replace", "ok", "{}"],
    ))
    .unwrap();
    assert_eq!(core.state().document.docs["notes"]["doc-1"].body, "ok");
}

#[test]
fn removing_the_app_drops_its_documents() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req(
        "document.create",
        &["notes", "daily", "Daily", "body", "{}"],
    ))
    .unwrap();

    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(core.state().document.docs.is_empty());
    assert!(core.replay_matches().unwrap());
}
