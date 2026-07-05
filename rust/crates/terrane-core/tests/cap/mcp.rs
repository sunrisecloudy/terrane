//! Engine tests for the `mcp` capability: recorded effects, exact per-server
//! grants, redaction, replay identity, and app removal cleanup.

use tempfile::tempdir;
use terrane_cap_mcp_client::{called_event, CalledEvent, REDACTED};
use terrane_core::{
    Core, Effect, EffectRunner, Error, EventRecord, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

struct CannedMcp;

impl EffectRunner for CannedMcp {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::McpCall {
                app,
                connection,
                tool,
                args,
                args_redacted,
                ..
            } => {
                if args.contains("secret") {
                    assert!(!args_redacted.contains("secret"));
                }
                Ok(vec![called_event(CalledEvent {
                    app: app.clone(),
                    connection: connection.clone(),
                    tool: tool.clone(),
                    args_json_redacted: args_redacted.clone(),
                    result_kind: "inline".to_string(),
                    result: r#"[{"type":"text","text":"ok"}]"#.to_string(),
                    result_is_base64: false,
                    result_hash: "a".repeat(64),
                    result_size: 29,
                    is_error: false,
                    called_at: "1700000000.000Z".to_string(),
                })?])
            }
            other => Err(Error::Runtime(format!("unexpected effect: {other:?}"))),
        }
    }
}

#[test]
fn mcp_call_records_redacted_result_and_replays() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedMcp).unwrap();
    core.dispatch(req("app.add", &["work", "Work"])).unwrap();
    core.dispatch(req(
        "mcp.connect",
        &[
            "linear",
            r#"{"http":{"url":"http://127.0.0.1/mcp","headers":{"authorization":{"$secret":"linear.header"}}}}"#,
        ],
    ))
    .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "work", "mcp:linear"]))
        .unwrap();

    let records = core
        .dispatch(req(
            "mcp.call",
            &[
                "work",
                "linear",
                "issue_search",
                r#"{"query":"bug","token":"secret","sensitiveArgs":["/token"]}"#,
            ],
        ))
        .unwrap();

    assert_eq!(records.iter().filter(|r| r.kind == "mcp.called").count(), 1);
    let calls = &core.state().mcp.calls["work"];
    assert_eq!(calls.len(), 1);
    let call = calls.values().next().unwrap();
    assert_eq!(call.connection, "linear");
    assert_eq!(call.tool, "issue_search");
    assert!(call.args_json_redacted.contains(REDACTED));
    assert!(!call.args_json_redacted.contains("secret"));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn mcp_call_requires_exact_server_grant() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedMcp).unwrap();
    core.dispatch(req("app.add", &["work", "Work"])).unwrap();
    core.dispatch(req(
        "mcp.connect",
        &["linear", r#"{"http":{"url":"http://127.0.0.1/mcp"}}"#],
    ))
    .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "work", "mcp"]))
        .unwrap();

    let err = core
        .dispatch(req("mcp.call", &["work", "linear", "echo", "{}"]))
        .unwrap_err();
    match err {
        Error::InvalidInput(msg) => assert!(msg.contains("mcp:linear"), "{msg}"),
        other => panic!("expected grant error, got {other:?}"),
    }
}

#[test]
fn mcp_disconnect_and_app_removed_clean_folded_state() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), CannedMcp).unwrap();
    core.dispatch(req("app.add", &["work", "Work"])).unwrap();
    core.dispatch(req(
        "mcp.connect",
        &["linear", r#"{"http":{"url":"http://127.0.0.1/mcp"}}"#],
    ))
    .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "work", "mcp:linear"]))
        .unwrap();
    core.dispatch(req("mcp.call", &["work", "linear", "echo", "{}"]))
        .unwrap();
    assert!(core.state().mcp.connections.contains_key("linear"));
    assert!(core.state().mcp.calls.contains_key("work"));

    core.dispatch(req("mcp.disconnect", &["linear"])).unwrap();
    assert!(!core.state().mcp.connections.contains_key("linear"));
    assert!(core.state().mcp.calls.contains_key("work"));
    core.dispatch(req("app.remove", &["work"])).unwrap();
    assert!(!core.state().mcp.calls.contains_key("work"));
    assert!(core.replay_matches().unwrap());
}
