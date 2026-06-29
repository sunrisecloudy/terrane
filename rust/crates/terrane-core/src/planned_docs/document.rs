use terrane_cap_interface::{
    command_doc, event_doc, limit, param, schema, CapabilityDoc, CapabilityManifestDoc, CommandDoc,
    EventDoc, ExampleDoc, InternalNote, ParamDoc, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};
pub fn document_doc(include_internal: bool) -> CapabilityDoc {
    let resource_methods = vec![
        method_doc(
            "create",
            "write",
            vec![
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
            "void",
            &[
                "invalid document id",
                "invalid metadata JSON",
                "body too large",
                "document quota exceeded",
            ],
        ),
        method_doc(
            "patch",
            "write",
            vec![
                param("id", "Stable document id.", "document_id.schema.json"),
                param(
                    "patchJson",
                    "Partial document update.",
                    "document_patch.schema.json",
                ),
            ],
            "Patch title, body, or metadata for one document.",
            "void",
            &[
                "missing document",
                "invalid patch JSON",
                "metadata too large",
                "body too large",
            ],
        ),
        method_doc(
            "append",
            "write",
            vec![
                param("id", "Stable document id.", "document_id.schema.json"),
                param("text", "Text to append to the body.", ""),
            ],
            "Append text to a document body.",
            "void",
            &["missing document", "body too large"],
        ),
        method_doc(
            "delete",
            "write",
            vec![param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Delete one app-owned document.",
            "void",
            &["invalid document id"],
        ),
        method_doc(
            "get",
            "read",
            vec![param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Read one document as JSON.",
            "string|null",
            &["invalid document id"],
        ),
        method_doc(
            "list",
            "read",
            Vec::new(),
            "List document ids and titles.",
            "string",
            &[],
        ),
        method_doc(
            "exportMarkdown",
            "read",
            vec![param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Return the body as markdown/plain text for copy or preview.",
            "string|null",
            &["missing document", "invalid document id"],
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
        commands: document_commands(),
        queries: Vec::new(),
        events: document_events(),
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
                code: include_str!("examples/document/create_note.js").to_string(),
                expected: "document created".to_string(),
            },
            ExampleDoc {
                title: "Append generated content".to_string(),
                summary: "Grow a document body without rewriting the whole document.".to_string(),
                language: "js".to_string(),
                code: include_str!("examples/document/append_generated_content.js").to_string(),
                expected: "document appended".to_string(),
            },
        ],
        constraints: vec![
            "Documents are app-scoped.".to_string(),
            "Bodies are strings; binary assets stay out of this capability.".to_string(),
            "Writes must be recorded as deterministic events.".to_string(),
            "Reads are derived from folded state and are not recorded.".to_string(),
            "Planned resource availability warning: ctx.resource.document may be absent until the host/runtime grants this planned capability; generated apps must feature-detect it before calling document methods."
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
                "that the runtime actually grants ctx.resource.document before calling it."
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

fn document_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "document.create",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param("title", "Human-readable title.", "string"),
                param("body", "Initial document body.", "string"),
                param(
                    "metadataJson",
                    "Optional metadata JSON.",
                    "document_meta.schema.json",
                ),
            ],
            "commit",
            "Create or replace one app-owned document.",
        )
        .with_errors(&[
            "document resource unavailable: planned capability not granted by runtime",
            "invalid document id",
            "invalid metadata JSON",
            "body too large",
            "document quota exceeded",
        ])
        .with_emits(&["document.created"]),
        command_doc(
            "document.patch",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param(
                    "patchJson",
                    "Partial document update.",
                    "document_patch.schema.json",
                ),
            ],
            "commit",
            "Patch title, body, or metadata for one document.",
        )
        .with_errors(&[
            "document resource unavailable: planned capability not granted by runtime",
            "missing document",
            "invalid patch JSON",
            "metadata too large",
            "body too large",
        ])
        .with_emits(&["document.patched"]),
        command_doc(
            "document.append",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param("text", "Text to append to the body.", "string"),
            ],
            "commit",
            "Append text to a document body.",
        )
        .with_errors(&[
            "document resource unavailable: planned capability not granted by runtime",
            "missing document",
            "body too large",
        ])
        .with_emits(&["document.patched"]),
        command_doc(
            "document.delete",
            &[param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "commit",
            "Delete one app-owned document.",
        )
        .with_errors(&[
            "document resource unavailable: planned capability not granted by runtime",
            "invalid document id",
        ])
        .with_emits(&["document.deleted"]),
    ]
}

fn document_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "document.created",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param("title", "Human-readable title.", "string"),
                param("body", "Document body.", "string"),
                param(
                    "metadataJson",
                    "Document metadata JSON.",
                    "document_meta.schema.json",
                ),
            ],
            "Creates or replaces the folded document record.",
        )
        .with_effects(&["folds into document state"]),
        event_doc(
            "document.patched",
            &[
                param("id", "Stable document id.", "document_id.schema.json"),
                param(
                    "patchJson",
                    "Partial document update.",
                    "document_patch.schema.json",
                ),
            ],
            "Applies a deterministic patch to the folded document record.",
        )
        .with_effects(&["folds into document state"]),
        event_doc(
            "document.deleted",
            &[param(
                "id",
                "Stable document id.",
                "document_id.schema.json",
            )],
            "Removes one folded document record.",
        )
        .with_effects(&["removes document state for id"]),
    ]
}

fn method_doc(
    name: &str,
    kind: &str,
    params: Vec<ParamDoc>,
    summary: &str,
    returns: &str,
    specific_errors: &[&str],
) -> ResourceMethodDoc {
    let mut errors = vec![
        "document resource unavailable: planned capability not granted by runtime".to_string(),
    ];
    errors.extend(specific_errors.iter().map(|error| (*error).to_string()));
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params,
        returns: returns.to_string(),
        summary: summary.to_string(),
        errors,
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
