//! Engine tests for Codex app generation requests.

use tempfile::tempdir;
use terrane_core::{Core, NoEffects};

use crate::helpers::{req, FakeEdge};

#[test]
fn codex_generation_records_and_replays_builder_draft_files() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, FakeEdge).unwrap();

    let records = core
        .dispatch(req(
            "codex.generate-app",
            &["demo", "demo", "Demo", "make a tiny greeting app"],
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
fn codex_generation_validates_request_before_effect() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), FakeEdge).unwrap();

    assert!(core
        .dispatch(req(
            "codex.generate-app",
            &["bad/path", "demo", "Demo", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("unsafe"));

    assert!(core
        .dispatch(req("codex.generate-app", &["demo", "demo", "Demo", ""]))
        .unwrap_err()
        .to_string()
        .contains("prompt"));
}

#[test]
fn pure_core_rejects_codex_effect_without_runner() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "codex.generate-app",
            &["demo", "demo", "Demo", "make app"],
        ))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));
}
