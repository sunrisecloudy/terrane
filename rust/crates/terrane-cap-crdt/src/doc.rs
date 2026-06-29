use terrane_cap_interface::{
    command_doc, event_doc, CapabilityDoc, CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc,
    InternalNote, LimitDoc, ParamDoc, ResourceDoc, ResourceMethodDoc,
};

use crate::resources;

pub fn crdt_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "crdt".to_string(),
        title: "CRDT Documents".to_string(),
        summary:
            "App-scoped Loro-backed map, list, and text containers that converge through recorded update bytes."
                .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "crdt.mapSet".to_string(),
                "crdt.mapDel".to_string(),
                "crdt.listPush".to_string(),
                "crdt.listInsert".to_string(),
                "crdt.listDel".to_string(),
                "crdt.textInsert".to_string(),
                "crdt.textDel".to_string(),
                "crdt.merge".to_string(),
            ],
            queries: Vec::new(),
            events: vec!["crdt.update".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: crdt_commands(),
        queries: Vec::new(),
        events: crdt_events(),
        resources: vec![ResourceDoc {
            namespace: "crdt".to_string(),
            summary:
                "Backend resource surface installed as ctx.resource.crdt for collaborative app-local containers."
                    .to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![
            ExampleDoc {
                title: "Map/list/text resources in one app".to_string(),
                summary:
                    "Each resource write records a crdt.update event; reads expose the folded app-local document."
                        .to_string(),
                language: "js".to_string(),
                code: include_str!("../examples/resources.js").to_string(),
                expected:
                    "A JSON string with convergent map, list, and text state for the current app."
                        .to_string(),
            },
            ExampleDoc {
                title: "Remove collaborative values".to_string(),
                summary: "Deletion writes also become crdt.update records.".to_string(),
                language: "js".to_string(),
                code: include_str!("../examples/deletions.js").to_string(),
                expected: "A JSON string representing the remaining map entries.".to_string(),
            },
        ],
        constraints: vec![
            "All ctx.resource.crdt methods operate on the current app only; app code cannot select another app id."
                .to_string(),
            "Writes are authored on a fork, exported as Loro update bytes, then folded from the recorded crdt.update event."
                .to_string(),
            "Replay imports recorded update bytes and never reauthors CRDT operations.".to_string(),
            "mapGet and mapAll expose string values only; non-string Loro map values are omitted from those string-typed reads."
                .to_string(),
            "listAll stringifies Loro scalar values for host/runtime compatibility.".to_string(),
            "crdt.merge is a command-level sync ingress for hex-encoded update bytes, not a ctx.resource method."
                .to_string(),
        ],
        limits: vec![
            limit(
                "documentScope",
                "one Loro document per app",
                "Named map, list, and text containers live inside the app's document.",
            ),
            limit(
                "recordedWriteShape",
                "crdt.update",
                "Every CRDT mutation and non-empty merge records update bytes rather than operation parameters.",
            ),
        ],
        compatibility: vec![
            "The sync helpers export version-vector deltas that can be fed to crdt.merge on another replica."
                .to_string(),
            "app.removed drops the app's CRDT document during broadcast fold.".to_string(),
            "Peer identity is taken from the replica capability when available and frozen into the recorded update bytes."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body:
                    "Loro authorship and merge validation happen in decide on a fork. fold imports exactly the bytes in crdt.update, which keeps replay deterministic."
                        .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn crdt_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "crdt.mapSet",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named map container.", "container_name"),
                param("key", "Map key.", "string"),
                param(
                    "value",
                    "String value to set; CLI tails are joined.",
                    "string",
                ),
            ],
            "events",
            "Set one map key and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.mapDel",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named map container.", "container_name"),
                param("key", "Map key.", "string"),
            ],
            "events",
            "Delete one map key and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.listPush",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named list container.", "container_name"),
                param(
                    "value",
                    "String value to append; CLI tails are joined.",
                    "string",
                ),
            ],
            "events",
            "Append one list value and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.listInsert",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named list container.", "container_name"),
                param("index", "Zero-based insertion index.", "usize_string"),
                param(
                    "value",
                    "String value to insert; CLI tails are joined.",
                    "string",
                ),
            ],
            "events",
            "Insert one list value and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "invalid index", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.listDel",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named list container.", "container_name"),
                param("index", "Zero-based item index.", "usize_string"),
            ],
            "events",
            "Delete one list value and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "invalid index", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.textInsert",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named text container.", "container_name"),
                param("index", "Zero-based text index.", "usize_string"),
                param("text", "Text to insert; CLI tails are joined.", "string"),
            ],
            "events",
            "Insert text and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "invalid index", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.textDel",
            &[
                param("app", "Target app id.", "app_id"),
                param("doc", "Named text container.", "container_name"),
                param("index", "Zero-based text index.", "usize_string"),
                param("len", "Number of characters to delete.", "usize_string"),
            ],
            "events",
            "Delete text and record the resulting Loro update bytes.",
        )
        .with_errors(&["missing app", "invalid range", "CRDT operation error"])
        .with_emits(&["crdt.update"]),
        command_doc(
            "crdt.merge",
            &[
                param("app", "Target app id.", "app_id"),
                param("update", "Lower-case hex-encoded Loro update bytes.", "hex"),
            ],
            "events",
            "Import a remote update into a fork and record it when it advances the document.",
        )
        .with_errors(&["missing app", "invalid update hex", "invalid Loro update"])
        .with_emits(&["crdt.update"]),
    ]
}

fn crdt_events() -> Vec<EventDoc> {
    vec![event_doc(
        "crdt.update",
        &[
            param("app", "Target app id.", "app_id"),
            param("bytes", "Recorded Loro update bytes.", "bytes"),
        ],
        "Imports recorded Loro update bytes into the app's CRDT document during fold.",
    )]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    resources::resource_methods()
        .into_iter()
        .map(|method| match method.name() {
            "mapSet" => method_doc(
                "mapSet",
                method.kind(),
                vec![
                    param("doc", "Named map container.", "container_name"),
                    param("key", "Map key.", "string"),
                    param("value", "String value to set.", "string"),
                ],
                "Set one string value in a named map.",
                "void",
                vec![
                    "missing app".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "mapGet" => method_doc(
                "mapGet",
                method.kind(),
                vec![
                    param("doc", "Named map container.", "container_name"),
                    param("key", "Map key.", "string"),
                ],
                "Read one string value from a named map.",
                "string|null",
                vec![
                    "Absent apps, containers, keys, or non-string values return null rather than an error."
                        .to_string(),
                    "Unknown resource methods return invalid input errors.".to_string(),
                ],
            ),
            "mapAll" => method_doc(
                "mapAll",
                method.kind(),
                vec![param("doc", "Named map container.", "container_name")],
                "Read all string entries from a named map.",
                "object",
                vec![
                    "Absent apps or containers return an empty object rather than an error."
                        .to_string(),
                    "Unknown resource methods return invalid input errors.".to_string(),
                ],
            ),
            "mapDel" => method_doc(
                "mapDel",
                method.kind(),
                vec![
                    param("doc", "Named map container.", "container_name"),
                    param("key", "Map key.", "string"),
                ],
                "Delete one key from a named map.",
                "void",
                vec![
                    "missing app".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "listPush" => method_doc(
                "listPush",
                method.kind(),
                vec![
                    param("doc", "Named list container.", "container_name"),
                    param("value", "String value to append.", "string"),
                ],
                "Append one value to a named list.",
                "void",
                vec![
                    "missing app".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "listInsert" => method_doc(
                "listInsert",
                method.kind(),
                vec![
                    param("doc", "Named list container.", "container_name"),
                    param("index", "Zero-based insertion index.", "usize_string"),
                    param("value", "String value to insert.", "string"),
                ],
                "Insert one value into a named list.",
                "void",
                vec![
                    "invalid index".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "listDel" => method_doc(
                "listDel",
                method.kind(),
                vec![
                    param("doc", "Named list container.", "container_name"),
                    param("index", "Zero-based item index.", "usize_string"),
                ],
                "Delete one item from a named list.",
                "void",
                vec![
                    "invalid index".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "listAll" => method_doc(
                "listAll",
                method.kind(),
                vec![param("doc", "Named list container.", "container_name")],
                "Read every item from a named list as strings.",
                "string[]",
                vec![
                    "Absent apps or containers return an empty list rather than an error."
                        .to_string(),
                    "Unknown resource methods return invalid input errors.".to_string(),
                ],
            ),
            "textInsert" => method_doc(
                "textInsert",
                method.kind(),
                vec![
                    param("doc", "Named text container.", "container_name"),
                    param("index", "Zero-based text index.", "usize_string"),
                    param("text", "Text to insert.", "string"),
                ],
                "Insert text into a named collaborative text container.",
                "void",
                vec![
                    "invalid index".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "textDel" => method_doc(
                "textDel",
                method.kind(),
                vec![
                    param("doc", "Named text container.", "container_name"),
                    param("index", "Zero-based text index.", "usize_string"),
                    param("len", "Number of characters to delete.", "usize_string"),
                ],
                "Delete text from a named collaborative text container.",
                "void",
                vec![
                    "invalid range".to_string(),
                    "CRDT operation error".to_string(),
                ],
            ),
            "textGet" => method_doc(
                "textGet",
                method.kind(),
                vec![param("doc", "Named text container.", "container_name")],
                "Read the current string from a named text container.",
                "string|null",
                vec![
                    "Absent apps or containers return null rather than an error.".to_string(),
                    "Unknown resource methods return invalid input errors.".to_string(),
                ],
            ),
            other => unreachable!("unexpected crdt resource method: {other}"),
        })
        .collect()
}

fn method_doc(
    name: &str,
    kind: &str,
    params: Vec<ParamDoc>,
    summary: &str,
    returns: &str,
    errors: Vec<String>,
) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params,
        returns: returns.to_string(),
        summary: summary.to_string(),
        errors,
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
