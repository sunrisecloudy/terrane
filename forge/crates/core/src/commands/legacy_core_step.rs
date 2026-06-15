//! `legacy.core_step` — temporary v0.4 `core.step` compatibility over the v1
//! [`CoreCommand`](forge_domain::CoreCommand) ABI.
//!
//! This command exists only for the legacy-removal cutover: native hosts can stop
//! loading `libzig_core` and instead route the still-live generated-app
//! `core.step` bridge method through `forge_core_handle_command`. The returned
//! payload preserves the old generated-app-visible shape
//! `{ ok, stateVersion, actions }` so host ABI migration does not also require a
//! generated-app/runtime rewrite in the same slice.

use forge_domain::{CoreCommand, Result};

use super::super::persistence::META_NS;
use super::super::WorkspaceCore;

const LEGACY_CORE_STEP_COUNTER_KEY: &str = "legacy_core_step_counter";

impl WorkspaceCore {
    /// Handle one legacy generated-app `core.step` event.
    pub(in crate::workspace) fn cmd_legacy_core_step(
        &mut self,
        cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        let event = match validate_event(cmd.payload.get("event")) {
            Ok(event) => event,
            Err(payload) => return Ok(payload),
        };

        let state_version = self
            .store_mut()
            .next_counter(META_NS, LEGACY_CORE_STEP_COUNTER_KEY)?;
        Ok(serde_json::json!({
            "ok": true,
            "stateVersion": state_version,
            "actions": actions_for_event(event),
        }))
    }
}

fn validate_event(
    value: Option<&serde_json::Value>,
) -> std::result::Result<&serde_json::Value, serde_json::Value> {
    let Some(event) = value else {
        return Err(error_payload(
            "invalid_event",
            "core.step input requires event",
        ));
    };
    let Some(object) = event.as_object() else {
        return Err(error_payload("invalid_event", "event must be an object"));
    };
    match object.get("type") {
        Some(serde_json::Value::String(_)) => Ok(event),
        Some(_) => Err(error_payload(
            "invalid_event",
            "event.type must be a string",
        )),
        None => Err(error_payload("invalid_event", "event.type is required")),
    }
}

fn actions_for_event(event: &serde_json::Value) -> serde_json::Value {
    let event_type = event
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or_default();
    match event_type {
        "CreateTask" => serde_json::json!([
            {
                "type": "Toast",
                "message": format!("Task accepted: {}", payload_string(event, "title").unwrap_or("task")),
                "level": "success",
            },
            { "type": "Log", "message": "CreateTask handled" },
        ]),
        "UpdateTask" => serde_json::json!([
            { "type": "Log", "message": "UpdateTask handled" },
        ]),
        "TransformText" => serde_json::json!([
            {
                "type": "TransformText",
                "text": transform_text(
                    payload_string(event, "text").unwrap_or_default(),
                    payload_string(event, "mode").unwrap_or("uppercase"),
                ),
            },
        ]),
        "ImportFile" => serde_json::json!([
            { "type": "Log", "message": "ImportFile handled" },
        ]),
        "NetworkSnapshotReceived" => serde_json::json!([
            { "type": "RenderHint", "hint": "network-snapshot-received" },
        ]),
        "ReplayEvents" => serde_json::json!([
            { "type": "Log", "message": "ReplayEvents handled" },
        ]),
        other => serde_json::json!([
            { "type": "Log", "message": format!("Unhandled event: {other}") },
        ]),
    }
}

fn payload_string<'a>(event: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    event
        .get("payload")
        .and_then(|payload| payload.as_object())
        .and_then(|payload| payload.get(field))
        .and_then(|value| value.as_str())
}

fn transform_text(text: &str, mode: &str) -> String {
    match mode {
        "lowercase" => text.to_lowercase(),
        "reverse-lines" => text.lines().rev().collect::<Vec<_>>().join("\n"),
        "word-count" => {
            let words = text.split_whitespace().count();
            let lines = if text.is_empty() {
                0
            } else {
                text.matches('\n').count() + 1
            };
            format!("Words: {words}\nLines: {lines}\nCharacters: {}", text.len())
        }
        _ => text.to_uppercase(),
    }
}

fn error_payload(code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "ok": false,
        "error": {
            "code": code,
            "message": message,
        },
        "actions": [],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{ActorContext, RequestId, WorkspaceId};

    fn command(payload: serde_json::Value) -> CoreCommand {
        CoreCommand {
            request_id: RequestId::new("legacy-core-step-test"),
            actor: ActorContext::owner("tester"),
            workspace_id: WorkspaceId::new("ws"),
            applet_id: None,
            name: "legacy.core_step".into(),
            payload,
        }
    }

    #[test]
    fn create_task_matches_legacy_core_shape() {
        let mut core = WorkspaceCore::in_memory("ws").unwrap();
        let response = core.handle(command(serde_json::json!({
            "event": {
                "type": "CreateTask",
                "payload": { "title": "Ship parity" }
            }
        })));
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.payload["ok"], serde_json::json!(true));
        assert_eq!(response.payload["stateVersion"], serde_json::json!(1));
        assert_eq!(
            response.payload["actions"],
            serde_json::json!([
                { "type": "Toast", "message": "Task accepted: Ship parity", "level": "success" },
                { "type": "Log", "message": "CreateTask handled" }
            ])
        );
    }

    #[test]
    fn transform_text_and_counter_are_durable_in_workspace() {
        let mut core = WorkspaceCore::in_memory("ws").unwrap();
        let first = core.handle(command(serde_json::json!({
            "event": {
                "type": "TransformText",
                "payload": { "text": "Hello", "mode": "lowercase" }
            }
        })));
        let second = core.handle(command(serde_json::json!({
            "event": { "type": "ProbeEvent" }
        })));
        assert!(first.ok, "{:?}", first.error);
        assert_eq!(first.payload["stateVersion"], serde_json::json!(1));
        assert_eq!(
            first.payload["actions"],
            serde_json::json!([{ "type": "TransformText", "text": "hello" }])
        );
        assert!(second.ok, "{:?}", second.error);
        assert_eq!(second.payload["stateVersion"], serde_json::json!(2));
        assert_eq!(
            second.payload["actions"],
            serde_json::json!([{ "type": "Log", "message": "Unhandled event: ProbeEvent" }])
        );
    }

    #[test]
    fn invalid_event_returns_legacy_payload_error_not_command_error() {
        let mut core = WorkspaceCore::in_memory("ws").unwrap();
        let response = core.handle(command(serde_json::json!({})));
        assert!(response.ok, "{:?}", response.error);
        assert_eq!(response.payload["ok"], serde_json::json!(false));
        assert_eq!(
            response.payload["error"],
            serde_json::json!({
                "code": "invalid_event",
                "message": "core.step input requires event"
            })
        );
    }
}
