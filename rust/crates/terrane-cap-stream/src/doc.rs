use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc,
};

use crate::{
    INLINE_TEXT_LIMIT, MAX_MESSAGE_SIZE, MAX_NAME_LEN, MAX_OPEN_STREAMS_PER_APP,
    RATE_LIMIT_PER_SECOND, RATE_LIMIT_WINDOW_SECONDS,
};

fn stream_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut list = resource_method(
        "list",
        "read",
        &[],
        "Return this app's folded stream desired-state as JSON.",
    );
    list.returns = "[{name,kind,verb,lastSeq,status}]".to_string();
    vec![list]
}

pub fn stream_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "stream".to_string(),
        title: "Recorded Streams".to_string(),
        summary: "Desired outbound SSE/WebSocket subscriptions with recorded inbound messages."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "stream.open".to_string(),
                "stream.close".to_string(),
                "stream.message".to_string(),
                "stream.reopened".to_string(),
                "stream.close-host".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "stream.opened".to_string(),
                "stream.message".to_string(),
                "stream.reopened".to_string(),
                "stream.closed".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: stream_resource_methods(),
        },
        commands: stream_commands(),
        queries: Vec::new(),
        events: stream_events(),
        resources: vec![ResourceDoc {
            namespace: "stream".to_string(),
            summary: "Read folded stream desired-state for the current app.".to_string(),
            methods: stream_resource_methods(),
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Open a recorded stream".to_string(),
            summary: "Record desired state; a long-running host connects at the edge.".to_string(),
            language: "cli".to_string(),
            code: r#"terrane stream open prices btc onTick '{"kind":"sse","url":"https://example.test/ticks","headers":{"Authorization":"Bearer x"}}"#.to_string(),
            expected: "records stream.opened with the Authorization value redacted".to_string(),
        }],
        constraints: vec![
            "stream.open is pure desired-state; replay never opens a socket.".to_string(),
            "stream.message and stream.reopened are trusted-host-only ingest commands.".to_string(),
            "The edge records every observed message as a fact; replay folds recorded chunks and never re-streams.".to_string(),
            "Sensitive headers are redacted identically to net-v2, and {$secret:name} markers are recorded as markers, never resolved secrets.".to_string(),
            "Large or binary messages are represented by blob refs such as __stream__/<app>/<name>/<seq>; event payloads stay compact.".to_string(),
            "The CLI can open/close/list and ingest individual messages, but it is not a long-running socket reconciler.".to_string(),
            "High-frequency streams grow the event log at message rate until compaction lands.".to_string(),
        ],
        limits: vec![
            limit("openStreamsPerApp", &MAX_OPEN_STREAMS_PER_APP.to_string(), "Bound live socket count per app."),
            limit("name", &format!("1..={MAX_NAME_LEN} chars [a-z0-9-_]"), "Stable app-local stream id."),
            limit("inlineTextBytes", &INLINE_TEXT_LIMIT.to_string(), "Text messages at or below this size stay inline."),
            limit("messageBytes", &MAX_MESSAGE_SIZE.to_string(), "Larger messages close the stream at the edge instead of truncating facts."),
            limit("rate", &format!("{RATE_LIMIT_PER_SECOND}/s sustained over {RATE_LIMIT_WINDOW_SECONDS}s"), "A stream that exceeds this is closed by the edge; facts are not silently dropped."),
        ],
        compatibility: vec![
            "Replay identity holds because stream.opened/message/reopened/closed are ordinary events.".to_string(),
            "app.removed removes folded stream metadata for that app; the edge disconnects when it reconciles desired state.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Edge reconciliation".to_string(),
                body: "stream_edge records delivered bytes and invokes the configured backend verb; full SSE/WS daemon loops are host-owned and outside deterministic replay.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn stream_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "stream.open",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "App-local stream name.", "stream_name"),
                param("verb", "Backend verb to run after each recorded message.", "token"),
                param("request_json", "JSON with kind, url, headers, sensitiveHeaders.", "json"),
            ],
            "commit",
            "Validate and record desired outbound stream state.",
        )
        .with_errors(&["app not found", "invalid name", "invalid request JSON", "open stream limit"])
        .with_emits(&["stream.opened"]),
        command_doc(
            "stream.close",
            &[param("app", "Existing app id.", "app_id"), param("name", "Stream name.", "stream_name")],
            "commit",
            "Close desired stream state at app request.",
        )
        .with_errors(&["unknown stream"])
        .with_emits(&["stream.closed"]),
        command_doc(
            "stream.message",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("seq", "Per-stream monotonic sequence.", "u64"),
                param("data_kind", "inline or blob.", "string"),
                param("data", "Inline data or blob name.", "string"),
                param("data_is_base64", "true when inline data is base64.", "bool"),
                param("data_hash", "SHA-256 of message bytes.", "sha256"),
                param("data_size", "Message byte size.", "u64"),
                param("received_at", "Host timestamp.", "string"),
            ],
            "commit",
            "Trusted host ingest of one observed stream message.",
        )
        .with_errors(&["requires trusted host authority", "seq regression", "message too large"])
        .with_emits(&["stream.message"]),
        command_doc(
            "stream.reopened",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("seq_before", "Last known seq before reconnect.", "u64"),
                param("attempt", "Reconnect attempt number.", "u64"),
            ],
            "commit",
            "Trusted host marker that a reconnect succeeded and a gap may exist.",
        )
        .with_errors(&["requires trusted host authority", "unknown stream"])
        .with_emits(&["stream.reopened"]),
        command_doc(
            "stream.close-host",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("reason", "Host close reason.", "token"),
            ],
            "commit",
            "Trusted host close used for remote, rate, and size violations.",
        )
        .with_errors(&["requires trusted host authority", "unknown stream", "invalid reason"])
        .with_emits(&["stream.closed"]),
    ]
}

fn stream_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "stream.opened",
            &[
                param("app", "App id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("verb", "Backend verb.", "token"),
                param("kind", "sse or ws.", "string"),
                param("request_json_redacted", "Redacted request JSON.", "json"),
            ],
            "Records desired stream state.",
        )
        .with_effects(&["stores StreamState.streams[app][name]"]),
        event_doc(
            "stream.message",
            &[
                param("app", "App id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("seq", "Per-stream sequence.", "u64"),
                param("data_kind", "inline or blob.", "string"),
                param("data", "Inline data or blob name.", "string"),
                param("data_is_base64", "Base64 marker.", "bool"),
                param("data_hash", "SHA-256.", "sha256"),
                param("data_size", "Byte length.", "u64"),
                param("received_at", "Host timestamp.", "string"),
            ],
            "Records one observed inbound message fact.",
        )
        .with_effects(&["advances lastSeq and stores last message"]),
        event_doc(
            "stream.reopened",
            &[
                param("app", "App id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("seq_before", "Last seq before reconnect.", "u64"),
                param("attempt", "Reconnect attempt.", "u64"),
            ],
            "Marks a successful reconnect and possible gap.",
        ),
        event_doc(
            "stream.closed",
            &[
                param("app", "App id.", "app_id"),
                param("name", "Stream name.", "stream_name"),
                param("reason", "Close reason.", "token"),
                param("by", "app, remote, or host.", "token"),
            ],
            "Marks stream desired state closed.",
        ),
    ]
}
