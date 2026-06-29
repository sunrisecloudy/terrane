use terrane_cap_interface::{
    limit, param, resource_method, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, ResourceDoc, SchemaDoc,
};
pub fn document_doc(include_internal: bool) -> CapabilityDoc {
    let resource_methods = vec![
        resource_method(
            "create",
            "write",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param("title", "Human-readable title.", ""),
                param("body", "Initial document body.", ""),
                param(
                    "metadataJson",
                    "Optional metadata JSON.",
                    "document_meta.schema.json",
                ),
            ],
            "Create or replace one app-owned document.",
        ),
        resource_method(
            "patch",
            "write",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param(
                    "patchJson",
                    "Partial document update.",
                    "document_patch.schema.json",
                ),
            ],
            "Patch title, body, or metadata for one document.",
        ),
        resource_method(
            "append",
            "write",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param("text", "Text to append to the body.", ""),
            ],
            "Append text to a document body.",
        ),
        resource_method(
            "delete",
            "write",
            &[param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Delete one app-owned document.",
        ),
        resource_method(
            "get",
            "read",
            &[param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Read one document as JSON.",
        ),
        resource_method("list", "read", &[], "List document ids and titles."),
        resource_method(
            "exportMarkdown",
            "read",
            &[param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Return the body as markdown/plain text for copy or preview.",
        ),
    ];
    CapabilityDoc {
        namespace: "document".to_string(),
        title: "Document".to_string(),
        summary: "Planned app-owned document storage for notes, drafts, and generated content."
            .to_string(),
        status: "planned".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "document.create".to_string(),
                "document.patch".to_string(),
                "document.append".to_string(),
                "document.delete".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "document.created".to_string(),
                "document.patched".to_string(),
                "document.deleted".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods.clone(),
        },
        resources: vec![ResourceDoc {
            namespace: "document".to_string(),
            summary: "App-scoped document records with explicit metadata and body text."
                .to_string(),
            methods: resource_methods,
        }],
        schemas: document_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Create a note".to_string(),
                summary: "Store a markdown note with simple metadata.".to_string(),
                language: "js".to_string(),
                code: r###"ctx.resource.document.create(
  "daily-plan",
  "Daily Plan",
  "## Today\n- Ship the capability docs",
  JSON.stringify({ contentType: "text/markdown", tags: ["planning"] })
);"###
                .to_string(),
                expected: "document created".to_string(),
            },
            ExampleDoc {
                title: "Append generated content".to_string(),
                summary: "Grow a document body without rewriting the whole document.".to_string(),
                language: "js".to_string(),
                code: r#"ctx.resource.document.append("daily-plan", "\n- Verify MCP completion");"#
                    .to_string(),
                expected: "document appended".to_string(),
            },
        ],
        constraints: vec![
            "Documents are app-scoped.".to_string(),
            "Bodies are strings; binary assets stay out of this capability.".to_string(),
            "Writes must be recorded as deterministic events.".to_string(),
            "Reads are derived from folded state and are not recorded.".to_string(),
            "Generated apps must check runtime availability before using this planned surface."
                .to_string(),
        ],
        limits: vec![
            limit(
                "maxDocumentsPerApp",
                "10000",
                "Keeps local-first indexes bounded.",
            ),
            limit("maxBodyBytes", "1048576", "Keeps individual documents reviewable."),
            limit("maxMetadataBytes", "16384", "Bounds metadata parsing."),
        ],
        compatibility: vec![
            concat!(
                "This planned doc is exposed before runtime injection; generated apps must check ",
                "that the runtime actually grants the resource before calling it."
            )
            .to_string(),
            "For collaborative merge semantics, use `crdt` until document-level collaboration lands.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Likely backing store".to_string(),
                body: concat!(
                    "The first runtime version can project documents onto reserved kv prefixes ",
                    "before a dedicated storage engine exists."
                )
                .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn document_schemas() -> Vec<SchemaDoc> {
    vec![
        schema(
            "document_id.schema.json",
            "Document id",
            include_str!("schemas/document/document_id.schema.json"),
        ),
        schema(
            "document_meta.schema.json",
            "Document metadata",
            include_str!("schemas/document/document_meta.schema.json"),
        ),
        schema(
            "document_patch.schema.json",
            "Document patch",
            include_str!("schemas/document/document_patch.schema.json"),
        ),
        schema(
            "document.schema.json",
            "Document",
            include_str!("schemas/document/document.schema.json"),
        ),
    ]
}
