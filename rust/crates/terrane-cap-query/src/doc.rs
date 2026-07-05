use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc as query_method_doc, resource_method,
    CapabilityDoc, CapabilityManifestDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc,
};

use crate::resource_methods;

pub fn query_doc(include_internal: bool) -> CapabilityDoc {
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
                "Query read/materialized-view resource method.",
            );
            doc.returns = "JSON string".to_string();
            doc
        })
        .collect::<Vec<_>>();
    CapabilityDoc {
        namespace: "query".to_string(),
        title: "Query".to_string(),
        summary: "Deterministic JMESPath reads, aggregation pipelines, and on-demand materialized views.".to_string(),
        status: "alpha".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["app-author".to_string(), "agent".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "query.view.define".to_string(),
                "query.materialize".to_string(),
                "query.view.drop".to_string(),
            ],
            queries: vec!["query.jmespath".to_string()],
            events: vec![
                "query.view.defined".to_string(),
                "query.materialized".to_string(),
                "query.row.put".to_string(),
                "query.view.dropped".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: vec![
            command_doc(
                "query.view.define",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("view", "View name.", "view"),
                    param("definition_json", "{source,pipeline,key}", "json"),
                ],
                "events",
                "Register or replace a materialized view definition.",
            )
            .with_errors(&["missing app", "invalid view name", "invalid definition JSON"])
            .with_emits(&["query.view.defined"]),
            command_doc(
                "query.materialize",
                &[param("app", "Target app id.", "app_id"), param("view", "View name.", "view")],
                "events",
                "Run the registered pipeline over folded state and snapshot rows as ordinary query events.",
            )
            .with_errors(&["missing app", "undefined view", "pipeline error", "duplicate row key"])
            .with_emits(&["query.materialized", "query.row.put"]),
            command_doc(
                "query.view.drop",
                &[param("app", "Target app id.", "app_id"), param("view", "View name.", "view")],
                "events",
                "Drop one view definition and its rows.",
            )
            .with_errors(&["missing app", "invalid view name"])
            .with_emits(&["query.view.dropped"]),
        ],
        queries: vec![
            query_method_doc(
                "query.jmespath",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("sourceJson", "Source object.", "json"),
                    param("expression", "JMESPath expression.", "string"),
                ],
                "json",
                "Evaluate a JMESPath expression over a source document.",
            )
            .with_errors(&["missing app", "invalid source JSON", "invalid JMESPath expression"]),
        ],
        events: query_events(),
        resources: vec![ResourceDoc {
            namespace: "query".to_string(),
            summary: "App-scoped query reads and materialized-view lookups.".to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Group daily totals and materialize".to_string(),
            summary: "Read kv order documents, aggregate totals by day, then fetch a view row by key.".to_string(),
            language: "js".to_string(),
            code: r#"const rows = ctx.resource.query.pipeline(
  JSON.stringify({kv:{prefix:"orders/"}}),
  JSON.stringify([{$group:{_id:"$day", total:{$sum:"$total"}}}])
);"#.to_string(),
            expected: "JSON array of grouped rows".to_string(),
        }],
        constraints: vec![
            "Queries are pure deterministic reads over folded state; materialize records ordinary query events only.".to_string(),
            "Materialized events carry def_hash and source_cursor for future reactive refresh without a format change.".to_string(),
            "Cross-app sources are rejected in v1; all sources are app scoped.".to_string(),
        ],
        limits: vec![
            limit("maxStages", "32", "Pipeline stage ceiling."),
            limit("maxSourceDocs", "100000", "Per-source scan ceiling."),
            limit("maxResultDocs", "10000", "Pipeline/materialized result ceiling."),
            limit("maxLookupForeignScan", "100000", "$lookup foreign scan ceiling."),
        ],
        compatibility: Vec::new(),
        internal: if include_internal {
            vec![InternalNote {
                title: "Cursor".to_string(),
                body: "QueryState counts broadcast-folded events; materialize stores that count as source_cursor.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn query_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "query.view.defined",
            &[
                param("app", "Target app id.", "app_id"),
                param("view", "View name.", "view"),
                param("def_json", "Canonical view definition JSON.", "json"),
                param("def_hash", "SHA-256 hash of def_json.", "hex"),
            ],
            "View definition and def hash.",
        ),
        event_doc(
            "query.materialized",
            &[
                param("app", "Target app id.", "app_id"),
                param("view", "View name.", "view"),
                param("def_hash", "Definition hash used for the snapshot.", "hex"),
                param(
                    "source_cursor",
                    "Folded event cursor at materialize time.",
                    "u64",
                ),
                param("row_count", "Number of rows in this snapshot.", "u64"),
            ],
            "Snapshot header with def hash, source cursor, and row count.",
        ),
        event_doc(
            "query.row.put",
            &[
                param("app", "Target app id.", "app_id"),
                param("view", "View name.", "view"),
                param("def_hash", "Definition hash this row belongs to.", "hex"),
                param("key", "Materialized row key.", "string"),
                param("doc_json", "Canonical row document JSON.", "json"),
            ],
            "One materialized row document.",
        ),
        event_doc(
            "query.view.dropped",
            &[
                param("app", "Target app id.", "app_id"),
                param("view", "View name.", "view"),
            ],
            "View drop marker.",
        ),
    ]
}
