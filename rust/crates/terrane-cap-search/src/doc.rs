use terrane_cap_interface::{
    command_doc, CapabilityDoc, CapabilityManifestDoc, CommandDoc, ExampleDoc, InternalNote,
    LimitDoc, ParamDoc, ResourceDoc, ResourceMethodDoc,
};

use crate::{resource_methods, SEARCH_PREFIX};

pub fn search_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "search".to_string(),
        title: "Search".to_string(),
        summary:
            "Hybrid BM25 + dense-vector search as a rebuildable KV projection with RRF fusion."
                .to_string(),
        status: "alpha".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: command_names(),
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: methods.clone(),
        },
        commands: search_commands(),
        queries: Vec::new(),
        events: Vec::new(),
        resources: vec![ResourceDoc {
            namespace: "search".to_string(),
            summary: "Backend resource surface for indexing and hybrid search reads.".to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Index documents and run hybrid search".to_string(),
            summary: "Upsert documents, store embeddings, and query with BM25 + vector RRF."
                .to_string(),
            language: "js".to_string(),
            code: include_str!("../examples/notes.js").to_string(),
            expected: "JSON hits returned from ctx.resource.search.query".to_string(),
        }],
        constraints: vec![
            "The search index is a derived read-model rebuilt from reserved kv records; it is not replay-critical State."
                .to_string(),
            "Document text and recorded embedding vectors are stored under reserved kv keys; writes commit kv.set and kv.deleted records."
                .to_string(),
            "Embeddings are produced by local-model at the edge; search stores the recorded vector via setEmbedding."
                .to_string(),
            "Hybrid query accepts an optional queryVec in optionsJson; without it, BM25-only fusion runs."
                .to_string(),
        ],
        limits: vec![
            limit("maxDocIdBytes", "128", "Portable key layout."),
            limit("maxQueryLimit", "100", "Hard bounded read ceiling."),
            limit("defaultRrfK", "60", "Reciprocal Rank Fusion constant."),
        ],
        compatibility: vec![
            "Reserved kv keys are hidden from public ctx.resource.kv reads, scans, and writes."
                .to_string(),
            "App removal drops all app-scoped kv data, including search projection keys."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Reserved kv layout".to_string(),
                body: format!(
                    "{}config stores SearchConfig JSON; {}doc/<doc_id> stores document text + metadata; {}embeddings/<model>/<doc_id> stores recorded embedding vectors.",
                    SEARCH_PREFIX, SEARCH_PREFIX, SEARCH_PREFIX
                ),
            }]
        } else {
            Vec::new()
        },
    }
}

fn command_names() -> Vec<String> {
    vec![
        "search.upsert".to_string(),
        "search.upsertJson".to_string(),
        "search.remove".to_string(),
        "search.configure".to_string(),
        "search.setEmbedding".to_string(),
    ]
}

fn search_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "search.upsert",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc_id", "Document id.", "doc_id"),
                param("text", "Document body.", "text"),
            ],
            "events",
            "Index or replace one document's plain text.",
        )
        .with_errors(&["missing app", "invalid doc_id", "empty text"])
        .with_emits(&["kv.set"]),
        command_doc(
            "search.upsertJson",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc_id", "Document id.", "doc_id"),
                param("docJson", r#"{"text":"...","metadata":{...}}"#, "doc_json"),
            ],
            "events",
            "Index or replace one document from JSON.",
        )
        .with_errors(&["missing app", "invalid doc_id", "invalid document JSON"])
        .with_emits(&["kv.set"]),
        command_doc(
            "search.remove",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc_id", "Document id.", "doc_id"),
            ],
            "events",
            "Remove a document and its stored embedding.",
        )
        .with_errors(&["missing app", "invalid doc_id"])
        .with_emits(&["kv.deleted"]),
        command_doc(
            "search.configure",
            &[
                param("app", "Target app id.", "app_id"),
                param(
                    "configJson",
                    r#"{"embedModel":"nomic","ftsWeight":1,"vecWeight":1,"rrfK":60}"#,
                    "config_json",
                ),
            ],
            "events",
            "Store hybrid-search configuration for the app.",
        )
        .with_errors(&["missing app", "invalid config JSON"])
        .with_emits(&["kv.set"]),
        command_doc(
            "search.setEmbedding",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc_id", "Document id.", "doc_id"),
                param("embeddingJson", "JSON array of floats.", "embedding_json"),
            ],
            "events",
            "Store a recorded embedding vector for an indexed document.",
        )
        .with_errors(&["missing app", "document not indexed", "invalid embedding JSON"])
        .with_emits(&["kv.set"]),
    ]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    resource_methods()
        .into_iter()
        .map(|method| match method.name() {
            "upsert" => method_doc(
                "upsert",
                method.kind(),
                vec![
                    param("docId", "Document id.", "doc_id"),
                    param("text", "Document body.", "text"),
                ],
                "Index or replace one document's plain text.",
                "void",
            ),
            "upsertJson" => method_doc(
                "upsertJson",
                method.kind(),
                vec![
                    param("docId", "Document id.", "doc_id"),
                    param("docJson", r#"{"text":"...","metadata":{...}}"#, "doc_json"),
                ],
                "Index or replace one document from JSON.",
                "void",
            ),
            "remove" => method_doc(
                "remove",
                method.kind(),
                vec![param("docId", "Document id.", "doc_id")],
                "Remove a document and its stored embedding.",
                "void",
            ),
            "configure" => method_doc(
                "configure",
                method.kind(),
                vec![param(
                    "configJson",
                    r#"{"embedModel":"nomic","ftsWeight":1,"vecWeight":1}"#,
                    "config_json",
                )],
                "Store hybrid-search configuration for the app.",
                "void",
            ),
            "setEmbedding" => method_doc(
                "setEmbedding",
                method.kind(),
                vec![
                    param("docId", "Document id.", "doc_id"),
                    param("embeddingJson", "JSON array of floats.", "embedding_json"),
                ],
                "Store a recorded embedding vector for an indexed document.",
                "void",
            ),
            "query" => method_doc(
                "query",
                method.kind(),
                vec![
                    param("text", "Query text for BM25 recall.", "text"),
                    param(
                        "optionsJson",
                        r#"{"limit":10,"queryVec":[...],"ftsWeight":1,"vecWeight":1}"#,
                        "query_options_json",
                    ),
                ],
                "Run hybrid BM25 + vector search fused with RRF.",
                "string",
            ),
            "bm25" => method_doc(
                "bm25",
                method.kind(),
                vec![
                    param("text", "Query text.", "text"),
                    param("optionsJson", r#"{"limit":10}"#, "query_options_json"),
                ],
                "Run keyword/BM25 search only.",
                "string",
            ),
            "vectorSearch" => method_doc(
                "vectorSearch",
                method.kind(),
                vec![
                    param("queryVecJson", "JSON array query embedding.", "embedding_json"),
                    param("optionsJson", r#"{"limit":10}"#, "query_options_json"),
                ],
                "Run dense-vector search only.",
                "string",
            ),
            "status" => method_doc(
                "status",
                method.kind(),
                Vec::new(),
                "Return index status for the current app.",
                "string",
            ),
            other => unreachable!("unexpected search resource method: {other}"),
        })
        .collect()
}

fn method_doc(
    name: &str,
    kind: &str,
    params: Vec<ParamDoc>,
    summary: &str,
    returns: &str,
) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params,
        returns: returns.to_string(),
        summary: summary.to_string(),
        errors: vec!["invalid input".to_string(), "missing document".to_string()],
    }
}

fn param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: true,
        schema_ref: schema_ref.to_string(),
    }
}

fn limit(name: &str, value: &str, reason: &str) -> LimitDoc {
    LimitDoc {
        name: name.to_string(),
        value: value.to_string(),
        reason: reason.to_string(),
    }
}