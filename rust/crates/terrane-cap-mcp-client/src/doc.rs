use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn mcp_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut call = resource_method(
        "call",
        "call",
        &[
            param("connection", "Named external MCP server.", "string"),
            param("tool", "External MCP tool name.", "string"),
            param("argsJson", "Tool arguments JSON object; may include sensitiveArgs JSON pointers.", "json"),
        ],
        "Recorded external MCP tools/call. Replay folds mcp.called and never contacts the server.",
    );
    call.returns = "the MCP content array as canonical JSON, or a blob reference error for offloaded results".to_string();
    let mut tools = resource_method(
        "tools",
        "call",
        &[param("connection", "Named external MCP server.", "string")],
        "Transient external MCP tools/list discovery. Records nothing.",
    );
    tools.returns = "the live tools/list result as JSON".to_string();
    vec![call, tools]
}

pub fn mcp_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "mcp".to_string(),
        title: "External MCP Client".to_string(),
        summary: "Apps calling external MCP servers through host-mediated recorded effects.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "mcp.connect".to_string(),
                "mcp.disconnect".to_string(),
                "mcp.call".to_string(),
                "mcp.tools".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "mcp.connected".to_string(),
                "mcp.disconnected".to_string(),
                "mcp.called".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: mcp_resource_methods(),
        },
        commands: mcp_commands(),
        queries: Vec::new(),
        events: mcp_events(),
        resources: vec![ResourceDoc {
            namespace: "mcp".to_string(),
            summary: "External MCP tools for apps with per-server mcp:<name> grants.".to_string(),
            methods: mcp_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Call an external tool".to_string(),
            summary: "Call a named external MCP server; the observed result is recorded for replay."
                .to_string(),
            language: "javascript".to_string(),
            code: r#"const result = ctx.resource.mcp.call("linear", "issue_search", JSON.stringify({query:"bug"}));"#.to_string(),
            expected: "mcp.called records the result; replay folds it without another MCP call.".to_string(),
        }],
        constraints: vec![
            "mcp.call is a recorded edge effect; replay never spawns a process, opens HTTP, or calls tools/call.".to_string(),
            "mcp.tools is transient discovery and records nothing.".to_string(),
            "mcp.connect and mcp.disconnect are operator registry writes; stdio command lines are trusted-admin severity.".to_string(),
            "Per-server grants use resource ids like mcp:linear; a wholesale mcp grant is not sufficient for decide-time mcp.call.".to_string(),
            "$secret markers in transport JSON are validated and redacted before recording; the host resolves them only at the edge.".to_string(),
            "sensitiveArgs JSON pointers redact request argument values before the event is written.".to_string(),
            "Results are recorded exactly as returned; tools that echo secrets write those secrets to the log.".to_string(),
            "Folding app.removed removes the app's recorded MCP call state.".to_string(),
        ],
        limits: vec![
            limit("connections", "16 per home", "Operator-defined external MCP registry entries."),
            limit("args", "128 KiB", "Tool arguments after removing timeoutMs and sensitiveArgs controls."),
            limit("result inline", "256 KiB", "Larger or binary content is offloaded to blob."),
            limit("result hard cap", "32 MiB", "Larger results are rejected before recording."),
            limit("timeoutMs", "1..300000", "Default per-call timeout is 60000 ms."),
            limit("recorded resource calls", "60 per backend run", "ctx.resource.mcp.call is capped per app run."),
        ],
        compatibility: vec![
            "This is app-to-external-MCP and does not import Terrane's own MCP server adapter.".to_string(),
            "Stdio session reuse is an edge optimization; the durable contract is the recorded mcp.called event.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::McpCall and Effect::McpTools are edge-only. Only mcp.called enters replay state; mcp.tools runs through TransientEffect.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn mcp_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "mcp.connect",
            &[
                param("name", "Connection name.", "string"),
                param("transport_json", "stdio or http transport JSON with optional $secret markers.", "json"),
            ],
            "commit",
            "Validate and record a redacted external MCP transport registry entry.",
        )
        .with_emits(&["mcp.connected"])
        .with_errors(&["invalid name", "invalid transport JSON", "connection limit exceeded"]),
        command_doc(
            "mcp.disconnect",
            &[param("name", "Connection name.", "string")],
            "commit",
            "Remove an external MCP transport registry entry.",
        )
        .with_emits(&["mcp.disconnected"])
        .with_errors(&["invalid name"]),
        command_doc(
            "mcp.call",
            &[
                param("app", "Existing app id that owns the recorded call.", "app_id"),
                param("connection", "External MCP connection name.", "string"),
                param("tool", "Tool name.", "string"),
                param("args_json", "Tool arguments JSON object.", "json"),
            ],
            "effect",
            "Validate one app-scoped external MCP tools/call and return the edge effect.",
        )
        .with_effects(&["McpCall"])
        .with_emits(&["mcp.called", "blob.stored when result offloads"])
        .with_errors(&["app not found", "unknown mcp connection", "missing mcp:<name> grant", "invalid args JSON"]),
        command_doc(
            "mcp.tools",
            &[
                param("app", "Existing app id requesting discovery.", "app_id"),
                param("connection", "External MCP connection name.", "string"),
            ],
            "transient-effect",
            "List tools on a live external MCP server without recording the response.",
        )
        .with_effects(&["McpTools"])
        .with_errors(&["app not found", "unknown mcp connection", "missing mcp:<name> grant"]),
    ]
}

fn mcp_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "mcp.connected",
            &[
                param("name", "Connection name.", "string"),
                param("transport_json_redacted", "Redacted stdio/http transport JSON.", "json"),
            ],
            "Records an operator-defined external MCP connection.",
        ),
        event_doc(
            "mcp.disconnected",
            &[param("name", "Connection name.", "string")],
            "Removes an external MCP connection from folded state.",
        ),
        event_doc(
            "mcp.called",
            &[
                param("app", "App id that requested the call.", "app_id"),
                param("connection", "External MCP connection name.", "string"),
                param("tool", "Tool name.", "string"),
                param("args_json_redacted", "Canonical arguments after sensitiveArgs redaction.", "json"),
                param("result_kind", "inline or blob.", "string"),
                param("result", "Canonical MCP content JSON or empty for blob.", "json|string"),
                param("result_is_base64", "True when inline result is base64 encoded.", "bool"),
                param("result_hash", "SHA-256 of result bytes.", "sha256"),
                param("result_size", "Result byte length.", "u64"),
                param("is_error", "MCP isError flag or transport failure.", "bool"),
                param("called_at", "Host timestamp recorded at the edge.", "string"),
            ],
            "Records an observed external MCP tool result for replay.",
        )
        .with_effects(&["stores McpClientState.calls[app][call_key]"]),
    ]
}
