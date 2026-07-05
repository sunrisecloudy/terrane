use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, ExampleDoc, InternalNote, ResourceDoc,
};

use crate::{resource_methods, MAX_LIST_LIMIT, MAX_REVERT_KEYS};

pub fn history_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_methods()
        .into_iter()
        .map(|method| {
            let params = method
                .params()
                .iter()
                .map(|p| param(p, "Argument.", "string"))
                .collect::<Vec<_>>();
            let mut doc = resource_method(
                method.name(),
                method.kind(),
                &params,
                "Read app-scoped history from the folded event-log projection.",
            );
            doc.returns = "JSON string or string value".to_string();
            doc
        })
        .collect::<Vec<_>>();
    CapabilityDoc {
        namespace: "history".to_string(),
        title: "History".to_string(),
        summary: "Time-travel reads over the event log and KV point-in-time reverts as compensating events.".to_string(),
        status: "alpha".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["app-author".to_string(), "agent".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec!["history.revert".to_string()],
            queries: vec![
                "history.list".to_string(),
                "history.key".to_string(),
                "history.at".to_string(),
            ],
            events: vec!["history.reverted".to_string()],
            subscriptions: vec![
                "app.removed".to_string(),
                "kv.set".to_string(),
                "kv.deleted".to_string(),
            ],
            resource_methods: methods.clone(),
        },
        commands: vec![
            command_doc(
                "history.revert",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("to_seq", "Event-log sequence to restore to.", "u64"),
                    param("scope", "key, prefix, or app.", "string"),
                    param("selector", "Key, prefix, or empty selector for app scope.", "string"),
                    param("actor", "Optional actor filter for candidate keys.", "string"),
                ],
                "events",
                "Emit ordinary kv.set/kv.deleted compensating events plus a history.reverted marker.",
            )
            .with_errors(&["missing app", "invalid seq", "invalid scope", "too many changed keys"])
            .with_emits(&["kv.set", "kv.deleted", "history.reverted"]),
        ],
        queries: vec![
            query_doc(
                "history.list",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("filter", "Optional kind:, key-prefix:, or actor: filter.", "string"),
                    param("before", "Optional exclusive upper sequence.", "u64"),
                    param("limit", "Maximum rows, capped at 500.", "usize"),
                ],
                "json",
                "Return a paged app timeline with honest from_seq horizon metadata.",
            )
            .with_errors(&["missing app", "invalid before sequence", "invalid limit"]),
            query_doc(
                "history.key",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("key", "KV key.", "string"),
                    param("limit", "Maximum changes.", "usize"),
                ],
                "json",
                "Return old/new changes for one KV key.",
            )
            .with_errors(&["missing app", "missing key", "invalid limit"]),
            query_doc(
                "history.at",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("key", "KV key.", "string"),
                    param("seq", "Sequence to read at.", "u64"),
                ],
                "json",
                "Return the key value as of a sequence.",
            )
            .with_errors(&["missing app", "missing key", "invalid sequence"]),
        ],
        events: vec![event_doc(
            "history.reverted",
            &[
                param("app", "Target app id.", "app_id"),
                param("to_seq", "Restored-to sequence.", "u64"),
                param("scope", "key, prefix, or app.", "string"),
                param("selector", "Scope selector.", "string"),
                param("changed_count", "Number of compensating KV events.", "u64"),
            ],
            "Auditable marker for a point-in-time revert.",
        )],
        resources: vec![ResourceDoc {
            namespace: "history".to_string(),
            summary: "App-scoped history reads for undo/timeline UI.".to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Undo button".to_string(),
            summary: "Read recent changes and inspect the selected key at a prior sequence.".to_string(),
            language: "js".to_string(),
            code: r#"const timeline = JSON.parse(ctx.resource.history.list("", "", "20"));
const before = ctx.resource.history.at("note/title", String(timeline.items[0].seq));"#.to_string(),
            expected: "Timeline JSON and a prior value for the key.".to_string(),
        }],
        constraints: vec![
            "The event log is never rewritten; restore records compensating KV events and a marker event.".to_string(),
            "v1 covers KV state only; CRDT/blob-specific history is intentionally deferred.".to_string(),
            "History reads are app scoped for app resources; shell/operator surfaces may choose any app.".to_string(),
        ],
        limits: vec![
            limit("listPage", &MAX_LIST_LIMIT.to_string(), "Maximum history.list rows."),
            limit("revertKeys", &MAX_REVERT_KEYS.to_string(), "Maximum keys in one revert."),
        ],
        compatibility: Vec::new(),
        internal: if include_internal {
            vec![InternalNote {
                title: "Projection".to_string(),
                body: "HistoryState is a rebuildable projection from broadcast fold; compacted logs can report a later from_seq once archive horizons exist.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}
