//! Phase 3 unified CLI integration tests (cli-plan/07 P3.5).
//!
//! Exercises the library helpers the `forge` binary uses: catalog list/describe,
//! run round-trip, inner-surface refusal, unknown-command rejection, and dry-run
//! schema validation.

use forge_cli::{
    actor_context, describe_catalog, find_command_descriptor, install, is_inner_command,
    open_core, parse_payload, run_command, DescribeFilter, RunOptions, WorkspaceOpenOptions,
    NOTES_LITE_MAIN_TS, NOTES_LITE_MANIFEST_JSON,
};
use forge_core::WorkspaceCore;
use forge_domain::CoreError;

fn describe_all(core: &mut WorkspaceCore, workspace_id: &str) -> serde_json::Value {
    describe_catalog(
        core,
        workspace_id,
        &DescribeFilter {
            tier: Some("debug".into()),
            include_inner: true,
            ..DescribeFilter::default()
        },
        actor_context(None, None),
    )
    .expect("system.describe")
}

#[test]
fn commands_json_matches_system_describe() {
    let mut core = open_core(&WorkspaceOpenOptions::in_memory()).unwrap();
    let catalog = describe_all(&mut core, "ws-cli");
    let names: Vec<String> = catalog["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"query.execute".to_string()));
    assert!(names.contains(&"system.describe".to_string()));
    // Outer commands are namespace.action; inner catalog may include short names (e.g. "log").
    assert!(
        names.iter().any(|name| name.contains('.')),
        "catalog should include dotted outer commands: {names:?}"
    );
}

#[test]
fn describe_returns_one_command_entry() {
    let mut core = open_core(&WorkspaceOpenOptions::in_memory()).unwrap();
    let catalog = describe_catalog(
        &mut core,
        "ws-cli",
        &DescribeFilter {
            tier: Some("debug".into()),
            names: Some(vec!["query.execute".into()]),
            include_inner: true,
            ..DescribeFilter::default()
        },
        actor_context(None, None),
    )
    .unwrap();
    let entry = find_command_descriptor(&catalog, "query.execute").unwrap();
    assert_eq!(entry["namespace"], "query");
    assert!(entry.get("payload_schema").is_some());
}

#[test]
fn run_query_execute_round_trips() {
    let opts = RunOptions {
        workspace: WorkspaceOpenOptions {
            workspace_id: "ws-test".into(),
            ..WorkspaceOpenOptions::in_memory()
        },
        ..RunOptions::default()
    };
    let outcome = run_command(
        "query.execute",
        serde_json::json!({ "collection": "notes" }),
        &opts,
    )
    .unwrap();
    assert!(outcome.response.ok, "{:?}", outcome.response.error);
    assert!(outcome.response.payload.get("rows").is_some());
}

#[test]
fn run_applet_install_via_demo_fixture() {
    let opts = RunOptions {
        workspace: WorkspaceOpenOptions {
            workspace_id: "ws-install".into(),
            ..WorkspaceOpenOptions::in_memory()
        },
        applet_id: Some("notes-lite".into()),
        ..RunOptions::default()
    };
    let manifest: serde_json::Value = serde_json::from_str(NOTES_LITE_MANIFEST_JSON).unwrap();
    let entrypoint = manifest["entrypoint"].as_str().unwrap();
    let outcome = run_command(
        "applet.install",
        serde_json::json!({
            "applet_id": "notes-lite",
            "manifest": manifest,
            "sources": { entrypoint: NOTES_LITE_MAIN_TS },
        }),
        &opts,
    )
    .unwrap();
    assert!(outcome.response.ok, "{:?}", outcome.response.error);
}

#[test]
fn run_inner_command_is_rejected() {
    let opts = RunOptions {
        workspace: WorkspaceOpenOptions::in_memory(),
        ..RunOptions::default()
    };
    let err = run_command("ctx.db.insert", serde_json::json!({}), &opts).unwrap_err();
    assert!(err.to_string().contains("inner host-call"), "{err}");
}

#[test]
fn run_unknown_command_returns_cr_a5_error() {
    let opts = RunOptions {
        workspace: WorkspaceOpenOptions::in_memory(),
        ..RunOptions::default()
    };
    let outcome = run_command(
        "definitely.not.a.command",
        serde_json::json!({}),
        &opts,
    )
    .unwrap();
    assert!(!outcome.response.ok);
    let err = outcome.response.error.unwrap();
    assert_eq!(err.code(), "ValidationError");
    assert!(err.to_string().contains("CR-A5"), "{err}");
}

#[test]
fn dry_run_rejects_invalid_payload() {
    let opts = RunOptions {
        dry_run: true,
        workspace: WorkspaceOpenOptions::in_memory(),
        ..RunOptions::default()
    };
    let err = run_command("query.execute", serde_json::json!({}), &opts).unwrap_err();
    assert!(err.to_string().contains("missing required property"), "{err}");
}

#[test]
fn dry_run_prints_envelope_without_executing() {
    let opts = RunOptions {
        dry_run: true,
        workspace: WorkspaceOpenOptions::in_memory(),
        ..RunOptions::default()
    };
    let outcome = run_command(
        "query.execute",
        serde_json::json!({ "collection": "notes" }),
        &opts,
    )
    .unwrap();
    assert!(outcome.response.ok);
    assert_eq!(outcome.response.payload["dry_run"], serde_json::json!(true));
    assert_eq!(outcome.envelope.name, "query.execute");
}

#[test]
fn parse_payload_reads_stdin_marker_and_objects() {
    let empty = parse_payload(None, None).unwrap();
    assert!(empty.is_object());
    let from_text = parse_payload(Some(r#"{"collection":"notes"}"#), None).unwrap();
    assert_eq!(from_text["collection"], "notes");
}

#[test]
fn is_inner_command_detects_ctx_prefix() {
    let mut core = open_core(&WorkspaceOpenOptions::in_memory()).unwrap();
    let catalog = describe_all(&mut core, "ws-cli");
    assert!(is_inner_command("ctx.db.insert", &catalog));
    assert!(!is_inner_command("query.execute", &catalog));
}

#[test]
fn run_emit_events_drains_core_events_after_success() {
    let manifest: serde_json::Value = serde_json::from_str(NOTES_LITE_MANIFEST_JSON).unwrap();
    let entrypoint = manifest["entrypoint"].as_str().unwrap();
    let opts = RunOptions {
        workspace: WorkspaceOpenOptions {
            workspace_id: "ws-events".into(),
            ..WorkspaceOpenOptions::in_memory()
        },
        applet_id: Some("notes-lite".into()),
        emit_events: true,
        ..RunOptions::default()
    };
    let outcome = run_command(
        "applet.install",
        serde_json::json!({
            "applet_id": "notes-lite",
            "manifest": manifest,
            "sources": { entrypoint: NOTES_LITE_MAIN_TS },
        }),
        &opts,
    )
    .unwrap();
    assert!(outcome.response.ok, "{:?}", outcome.response.error);
    assert!(
        outcome
            .events
            .iter()
            .any(|event| event.kind == "applet.installed"),
        "expected applet.installed event, got {:?}",
        outcome.events
    );
}

#[test]
fn install_helper_still_works_for_scenarios() {
    let mut core = WorkspaceCore::in_memory("ws-scenario").unwrap();
    install(&mut core, "notes-lite", NOTES_LITE_MANIFEST_JSON, NOTES_LITE_MAIN_TS)
        .expect("install through facade");
}

#[test]
fn unknown_command_error_is_validation_not_panic() {
    let opts = RunOptions {
        workspace: WorkspaceOpenOptions::in_memory(),
        ..RunOptions::default()
    };
    let outcome = run_command("nope.nope", serde_json::json!({}), &opts).unwrap();
    assert!(matches!(
        outcome.response.error,
        Some(CoreError::ValidationError(_))
    ));
}