//! Engine tests for replayable builder draft state.

use tempfile::tempdir;
use terrane_core::cap::builder::{failed_event, generated_event, requested_event, BuilderFile};
use terrane_core::{fold_records_in_memory, Core, NoEffects, State};

use crate::helpers::req;

#[test]
fn builder_records_fold_into_draft_state() {
    let mut state = State::default();
    let records = vec![
        requested_event("draft-1", "demo", "Demo", "make a tiny app", "codex").unwrap(),
        generated_event(
            "draft-1",
            vec![BuilderFile {
                path: "manifest.json".into(),
                content: r#"{"id":"demo","name":"Demo","version":"0.1.0","backend":"main.js","ui":"index.html","resources":[]}"#.into(),
            }],
        )
        .unwrap(),
    ];

    fold_records_in_memory(&mut state, &records).unwrap();

    let draft = &state.builder.drafts["draft-1"];
    assert_eq!(draft.app_id, "demo");
    assert_eq!(draft.name, "Demo");
    assert_eq!(draft.harness, "codex");
    assert!(draft.error.is_none());
    assert!(draft.files.iter().any(|f| f.path == "manifest.json"));
}

#[test]
fn builder_failed_event_clears_files_and_records_error() {
    let mut state = State::default();
    let records = vec![
        requested_event("draft-1", "demo", "Demo", "make a tiny app", "codex").unwrap(),
        generated_event(
            "draft-1",
            vec![BuilderFile {
                path: "manifest.json".into(),
                content: "{}".into(),
            }],
        )
        .unwrap(),
        failed_event("draft-1", "harness exited 1").unwrap(),
    ];

    fold_records_in_memory(&mut state, &records).unwrap();

    let draft = &state.builder.drafts["draft-1"];
    assert!(draft.files.is_empty());
    assert_eq!(draft.error.as_deref(), Some("harness exited 1"));
}

#[test]
fn builder_namespace_has_no_generation_commands() {
    let dir = tempdir().unwrap();
    let mut core = Core::<NoEffects>::open(dir.path().join("log.bin")).unwrap();

    assert!(core
        .dispatch(req(
            "builder.generate-app",
            &["demo", "demo", "Demo", "make app"]
        ))
        .unwrap_err()
        .to_string()
        .contains("unknown command"));
}
