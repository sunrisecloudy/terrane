//! Engine tests for the `builder` capability.

use tempfile::tempdir;
use terrane_core::{Core, NoEffects};

use crate::helpers::{req, FakeEdge};

#[test]
fn builder_generation_records_and_replays_draft_files() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, FakeEdge).unwrap();

    let records = core
        .dispatch(req(
            "builder.generate",
            &["demo", "demo", "Demo", "codex", "make a tiny greeting app"],
        ))
        .unwrap();
    assert_eq!(
        records.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(),
        vec!["builder.requested", "builder.generated"]
    );

    let draft = &core.state().builder.drafts["demo"];
    assert_eq!(draft.app_id, "demo");
    assert_eq!(draft.name, "Demo");
    assert_eq!(draft.agent, "codex");
    assert!(draft.error.is_none());
    assert!(draft.files.iter().any(|f| f.path == "manifest.json"));

    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().builder.drafts, core.state().builder.drafts);
    assert!(reopened.replay_matches().unwrap());
}

#[test]
fn builder_generation_validates_request_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), FakeEdge).unwrap();

    assert!(core
        .dispatch(req(
            "builder.generate",
            &["bad/path", "demo", "Demo", "codex", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("unsafe"));

    assert!(core
        .dispatch(req(
            "builder.generate",
            &["demo", "demo", "Demo", "claude", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("unsupported"));

    assert!(core
        .dispatch(req(
            "builder.generate",
            &["demo", "demo", "Demo", "codex", ""],
        ))
        .unwrap_err()
        .to_string()
        .contains("prompt"));
}

#[test]
fn pure_core_rejects_builder_effect_without_runner() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "builder.generate",
            &["demo", "demo", "Demo", "codex", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
}
