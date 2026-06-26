//! Integration tests for `system.trace` — run observability through the facade.

use forge_core::WorkspaceCore;
use forge_domain::{ActorContext, AppletId, CoreCommand, RequestId, Role, WorkspaceId};
use std::path::{Path, PathBuf};

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn command(name: &str, applet_id: Option<&str>, payload: serde_json::Value) -> CoreCommand {
    CoreCommand {
        request_id: RequestId::new("req"),
        actor: ActorContext {
            actor: "test".into(),
            role: Role::Owner,
        },
        workspace_id: WorkspaceId::new("ws"),
        applet_id: applet_id.map(AppletId::new),
        name: name.into(),
        payload,
    }
}

fn install_notes_lite(core: &mut WorkspaceCore) {
    let dir = examples_dir().join("notes-lite");
    let manifest_json: serde_json::Value =
        serde_json::from_str(&read(dir.join("manifest.json"))).unwrap();
    let main_ts = read(dir.join("src/main.ts"));

    let resp = core.handle(command(
        "applet.install",
        Some("notes-lite"),
        serde_json::json!({
            "manifest": manifest_json,
            "sources": { "src/main.ts": main_ts }
        }),
    ));
    assert!(resp.ok, "notes-lite install must succeed: {:?}", resp.error);
}

#[test]
fn system_trace_returns_recorded_calls_for_run() {
    let mut core = WorkspaceCore::in_memory("ws").unwrap();
    install_notes_lite(&mut core);

    let run = core.handle(command(
        "runtime.run",
        Some("notes-lite"),
        serde_json::json!({
            "input": { "title": "Trace me" },
            "random_seed": 7,
            "time_start": 500
        }),
    ));
    assert!(run.ok, "runtime.run must succeed: {:?}", run.error);
    let run_id = run.payload["run_id"]
        .as_str()
        .expect("run_id")
        .to_string();

    let trace = core.handle(command(
        "system.trace",
        None,
        serde_json::json!({ "run_id": run_id }),
    ));
    assert!(trace.ok, "system.trace must succeed: {:?}", trace.error);
    assert_eq!(trace.payload["run_id"], serde_json::json!(run_id));
    assert_eq!(trace.payload["applet_id"], serde_json::json!("notes-lite"));

    let calls = trace.payload["calls"]
        .as_array()
        .expect("calls array");
    assert!(!calls.is_empty(), "run must record at least one host call");

    for call in calls {
        if call["method"].as_str() == Some("net.fetch") {
            for key in ["body", "request_body", "response_body"] {
                if let Some(value) = call["args"].get(key) {
                    assert_eq!(
                        value["redacted"],
                        serde_json::json!(true),
                        "net.fetch args.{key} must be redacted"
                    );
                }
                if let Some(value) = call["response"].get(key) {
                    assert_eq!(
                        value["redacted"],
                        serde_json::json!(true),
                        "net.fetch response.{key} must be redacted"
                    );
                }
            }
        }
    }
}

#[test]
fn system_trace_requires_run_id() {
    let mut core = WorkspaceCore::in_memory("ws").unwrap();
    let resp = core.handle(command("system.trace", None, serde_json::json!({})));
    assert!(!resp.ok, "missing run_id must fail");
    assert_eq!(resp.error.unwrap().code(), "ValidationError");
}