//! Backgrounded harness generation: a host runs the effect on a worker,
//! stages the records via [`HarnessStaging`], and commits them through the
//! ordinary `harness.generate-app` dispatch. The log must replay identically.

use tempfile::tempdir;
use terrane_cap_builder::{generated_event, requested_event, BuilderFile};
use terrane_core::Core;
use terrane_host::{
    builder_draft_json, dispatch_on_core, open_at_home_with_staging, HarnessStaging,
};

fn staged_records(draft_id: &str) -> Vec<terrane_host::EventRecord> {
    let manifest = r#"{"id":"bg-demo","name":"BG Demo","version":"0.1.0","runtime":"js","backend":"main.js","ui":"index.html","resources":[]}"#;
    vec![
        requested_event(draft_id, "bg-demo", "BG Demo", "make a demo", "codex").unwrap(),
        generated_event(
            draft_id,
            vec![
                BuilderFile {
                    path: "manifest.json".into(),
                    content: manifest.into(),
                },
                BuilderFile {
                    path: "main.js".into(),
                    content: "function handle(input) { return \"ok\"; }".into(),
                },
                BuilderFile {
                    path: "index.html".into(),
                    content: "<!doctype html><html><body>bg demo</body></html>".into(),
                },
            ],
        )
        .unwrap(),
    ]
}

#[test]
fn staged_generation_commits_through_dispatch_and_replays() {
    let dir = tempdir().unwrap();
    let staging = HarnessStaging::default();
    let mut core = open_at_home_with_staging(dir.path(), staging.clone()).unwrap();

    staging.stage_generated("bg-demo", staged_records("bg-demo"));
    dispatch_on_core(
        &mut core,
        "harness.generate-app",
        &[
            "--harness".into(),
            "codex".into(),
            "bg-demo".into(),
            "bg-demo".into(),
            "BG Demo".into(),
            "make a demo".into(),
        ],
    )
    .expect("staged records commit without running a harness CLI");

    let draft = builder_draft_json(&core, "bg-demo").expect("draft recorded in state");
    assert!(draft.contains("manifest.json"), "draft: {draft}");
    assert!(draft.contains("bg-demo"), "draft: {draft}");

    // The staged slot is consumed: nothing left for a second dispatch.
    drop(core);
    let reopened = Core::open(dir.path().join("log.bin")).unwrap();
    assert!(
        reopened.state().builder.drafts.contains_key("bg-demo"),
        "draft survives reopen"
    );
    assert!(
        reopened.replay_matches().unwrap(),
        "staged commit must preserve replay identity"
    );
}
