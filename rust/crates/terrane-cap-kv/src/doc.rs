use terrane_cap_interface::{
    command_doc, event_doc, CapabilityDoc, CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc,
    InternalNote, LimitDoc, ParamDoc, ResourceDoc, ResourceMethodDoc,
};

use crate::resources;
use crate::{DEFAULT_SCAN_LIMIT, MAX_SCAN_LIMIT, RESERVED_PREFIX};

pub fn kv_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "kv".to_string(),
        title: "Key Value Store".to_string(),
        summary:
            "App-scoped string key/value storage with deterministic event replay and optional storage projection."
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
                "kv.set".to_string(),
                "kv.rm".to_string(),
                "kv.delete".to_string(),
                "kv.storage.set".to_string(),
                "kv.storage.clear".to_string(),
                "kv.public.set".to_string(),
                "kv.public.rm".to_string(),
                "kv.public.import".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "kv.set".to_string(),
                "kv.deleted".to_string(),
                "kv.storage.configured".to_string(),
                "kv.storage.cleared".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: kv_commands(),
        queries: Vec::new(),
        events: kv_events(),
        resources: vec![ResourceDoc {
            namespace: "kv".to_string(),
            summary: "Backend resource surface installed as ctx.resource.kv for apps that declare the kv resource."
                .to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![
            ExampleDoc {
                title: "Use every app KV resource method".to_string(),
                summary:
                    "Writes are scoped to the current app id; reads see the folded state after prior writes."
                        .to_string(),
                language: "js".to_string(),
                code: include_str!("../examples/kv_resource_methods.js").to_string(),
                expected:
                    "A JSON string containing the first value, bounded prefix/range reads, keys, and remaining app-local entries."
                        .to_string(),
            },
            ExampleDoc {
                title: "App-local key names can overlap".to_string(),
                summary:
                    "The runtime host injects the current app id, so apps do not pass or share an app argument."
                        .to_string(),
                language: "js".to_string(),
                code: include_str!("../examples/app_local_key_overlap.js").to_string(),
                expected: "The value for this app's settings/theme key only.".to_string(),
            },
            ExampleDoc {
                title: "Safely read optional index keys".to_string(),
                summary:
                    "Generated apps commonly keep index keys such as event_ids. Treat absent index keys as empty before parsing JSON."
                        .to_string(),
                language: "js".to_string(),
                code: include_str!("../examples/kv_optional_index.js").to_string(),
                expected:
                    "First-run reads return an empty array; after add, the index stores event ids and reads each event value."
                        .to_string(),
            },
            ExampleDoc {
                title: "Configure storage projection from the host".to_string(),
                summary:
                    "Storage commands are host/user commands, not ctx.resource methods. They record replayed binding events."
                        .to_string(),
                language: "sh".to_string(),
                code: include_str!("../examples/storage_projection.sh").to_string(),
                expected:
                    "kv.storage.configured and kv.storage.cleared events update the default or app-specific binding."
                        .to_string(),
            },
        ],
        constraints: vec![
            "All ctx.resource.kv methods operate on the current app only; app code cannot select another app id."
                .to_string(),
            format!(
                "Keys beginning with {RESERVED_PREFIX:?} are reserved for platform capabilities. Public kv commands and resource writes reject them, and public reads/scans hide them."
            ),
            "Values are strings. Apps should serialize JSON explicitly when storing structured data."
                .to_string(),
            "For optional/index keys, app code should treat a missing value as empty before JSON.parse; a small kvGetOrNull helper is safest for generated apps."
                .to_string(),
            "Reads are derived from folded state and are not recorded as events.".to_string(),
            "ctx.resource.kv.set and ctx.resource.kv.rm write deterministic kv.set and kv.deleted records through the runtime host."
                .to_string(),
            "kv.storage.set and kv.storage.clear affect physical projection bindings only; the event log remains the source of truth."
                .to_string(),
            "kv.public.set/rm/import write the shared cross-app public bucket and are trusted-host only; app code reaches it through the read-only ctx.resource.kv.public* methods."
                .to_string(),
            "Public reads (public/publicScan/publicAll/publicKeys) are cross-app and read-only; they never reveal another app's private bucket and do not filter reserved-key prefixes."
                .to_string(),
        ],
        limits: vec![
            limit(
                "defaultScanLimit",
                &DEFAULT_SCAN_LIMIT.to_string(),
                "Used by scan, range, and keys when the limit argument is omitted or empty.",
            ),
            limit(
                "maxScanLimit",
                &MAX_SCAN_LIMIT.to_string(),
                "scan, range, keys, and internal prefix deletion clamp caller-provided limits to this ceiling.",
            ),
            limit(
                "minScanLimit",
                "1",
                "A parsed zero limit is clamped upward so scans always make progress.",
            ),
        ],
        compatibility: vec![
            "When no storage binding has been recorded, KV projects to SQLite at terrane.db relative to TERRANE_HOME."
                .to_string(),
            "Default builds include memory and SQLite projection; enable rocksdb-storage only when RocksDB projection is needed."
                .to_string(),
            "Unavailable storage backends are rejected before kv.storage.configured is recorded."
                .to_string(),
            "An app-specific storage binding overrides the default binding; clearing the app binding falls back to the default."
                .to_string(),
            "The compatibility alias kv.delete is accepted for the kv.rm command path.".to_string(),
            "On app.removed, kv drops that app's data and app-specific storage binding.".to_string(),
        ],
        internal: if include_internal {
            vec![
                InternalNote {
                    title: "Reserved key writers".to_string(),
                    body: format!(
                        "Platform capabilities may intentionally emit kv.set or kv.deleted for {RESERVED_PREFIX:?} keys by using the event helpers directly; public kv commands still enforce the guardrail."
                    ),
                },
                InternalNote {
                    title: "Storage projection".to_string(),
                    body:
                        "Hosts materialize folded kv state into configured backends after commits. Projection files are derived state and can be rebuilt from the log."
                            .to_string(),
                },
            ]
        } else {
            Vec::new()
        },
    }
}

fn kv_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "kv.set",
            &[
                param("app", "Target app id.", "app_id"),
                param("key", "App-local key to write.", "kv_key"),
                param(
                    "value",
                    "String value to record; CLI tails are joined.",
                    "string",
                ),
            ],
            "events",
            "Record a string value in an app's KV store.",
        )
        .with_errors(&["missing app", "reserved key", "empty key"])
        .with_emits(&["kv.set"]),
        command_doc(
            "kv.rm",
            &[
                param("app", "Target app id.", "app_id"),
                param("key", "App-local key to delete.", "kv_key"),
            ],
            "events",
            "Delete an existing key from an app's KV store.",
        )
        .with_errors(&["missing app", "reserved key", "missing key"])
        .with_emits(&["kv.deleted"]),
        command_doc(
            "kv.delete",
            &[
                param("app", "Target app id.", "app_id"),
                param("key", "App-local key to delete.", "kv_key"),
            ],
            "events",
            "Compatibility alias for kv.rm.",
        )
        .with_errors(&["missing app", "reserved key", "missing key"])
        .with_emits(&["kv.deleted"]),
        command_doc(
            "kv.storage.set",
            &[
                param("scope", "default or app.", "kv_storage_scope"),
                optional_param("app", "Target app id when scope is app.", "app_id"),
                param(
                    "backend",
                    "memory, sqlite, or rocksdb.",
                    "kv_storage_backend",
                ),
                optional_param(
                    "path",
                    "Optional backend path relative to TERRANE_HOME.",
                    "path",
                ),
            ],
            "events",
            "Configure the default or app-specific physical storage projection for KV.",
        )
        .with_errors(&[
            "missing app",
            "unknown backend",
            "backend feature disabled",
            "empty path",
        ])
        .with_emits(&["kv.storage.configured"]),
        command_doc(
            "kv.storage.clear",
            &[
                param("scope", "default or app.", "kv_storage_scope"),
                optional_param("app", "Target app id when scope is app.", "app_id"),
            ],
            "events",
            "Clear the default or app-specific storage projection binding.",
        )
        .with_errors(&["missing app", "invalid scope"])
        .with_emits(&["kv.storage.cleared"]),
        command_doc(
            "kv.public.set",
            &[
                param("key", "Public bucket key (no app argument).", "kv_key"),
                param(
                    "value",
                    "String value to record; CLI tails are joined.",
                    "string",
                ),
            ],
            "events",
            "Record a cross-app read-only public value. Trusted host only.",
        )
        .with_errors(&["empty key", "requires trusted host authority"])
        .with_emits(&["kv.set"]),
        command_doc(
            "kv.public.rm",
            &[param("key", "Public bucket key to delete.", "kv_key")],
            "events",
            "Delete an existing cross-app public value. Trusted host only.",
        )
        .with_errors(&["missing key", "requires trusted host authority"])
        .with_emits(&["kv.deleted"]),
        command_doc(
            "kv.public.import",
            &[param(
                "json",
                "A flat {\"key\":\"value\"} object; emitted as a sorted batch.",
                "string",
            )],
            "events",
            "Import a flat string map into the public bucket deterministically. Trusted host only.",
        )
        .with_errors(&[
            "invalid JSON object",
            "non-string value",
            "requires trusted host authority",
        ])
        .with_emits(&["kv.set"]),
    ]
}

fn kv_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "kv.set",
            &[
                param("app", "Target app id.", "app_id"),
                param("key", "App-local key.", "kv_key"),
                param("value", "String value.", "string"),
            ],
            "Stores or replaces one app-local key/value pair during fold.",
        ),
        event_doc(
            "kv.deleted",
            &[
                param("app", "Target app id.", "app_id"),
                param("key", "App-local key.", "kv_key"),
            ],
            "Removes one app-local key/value pair during fold.",
        ),
        event_doc(
            "kv.storage.configured",
            &[
                optional_param(
                    "app",
                    "App id for app-specific binding; absent means default.",
                    "app_id",
                ),
                param(
                    "backend",
                    "memory, sqlite, or rocksdb.",
                    "kv_storage_backend",
                ),
                optional_param("path", "Optional backend path.", "path"),
            ],
            "Sets the folded default or app-specific storage binding.",
        ),
        event_doc(
            "kv.storage.cleared",
            &[optional_param(
                "app",
                "App id for app-specific binding; absent means default.",
                "app_id",
            )],
            "Clears the folded default or app-specific storage binding.",
        ),
    ]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    resources::resource_methods()
        .into_iter()
        .map(|method| match method.name() {
            "set" => method_doc(
                "set",
                method.kind(),
                vec![
                    param("key", "App-local key to write.", "kv_key"),
                    param("value", "String value to record.", "string"),
                ],
                "Record a string value for key in the current app's KV store.",
                "void",
                vec!["reserved key".to_string(), "empty key".to_string()],
            ),
            "get" => method_doc(
                "get",
                method.kind(),
                vec![param("key", "App-local key to read.", "kv_key")],
                "Read one non-reserved key from the current app's KV store.",
                "string|null",
                vec!["reserved keys return null".to_string()],
            ),
            "all" => method_doc(
                "all",
                method.kind(),
                Vec::new(),
                "Return every non-reserved key/value pair for the current app.",
                "object",
                vec![
                    "Absent apps return an empty object rather than an error.".to_string(),
                    "Reserved platform keys are filtered from the result.".to_string(),
                ],
            ),
            "rm" => method_doc(
                "rm",
                method.kind(),
                vec![param("key", "App-local key to delete.", "kv_key")],
                "Delete an existing non-reserved key from the current app's KV store.",
                "void",
                vec!["reserved key".to_string(), "missing key".to_string()],
            ),
            "scan" => method_doc(
                "scan",
                method.kind(),
                vec![
                    param("prefix", "Inclusive key prefix.", "kv_key_prefix"),
                    param(
                        "limit",
                        "Optional integer limit clamped to the scan limits.",
                        "integer_string",
                    ),
                ],
                "Return non-reserved key/value pairs whose keys start with prefix, ordered by key.",
                "object",
                vec!["reserved prefix".to_string(), "invalid limit".to_string()],
            ),
            "range" => method_doc(
                "range",
                method.kind(),
                vec![
                    param("start", "Inclusive start key.", "kv_key"),
                    param("endExclusive", "Exclusive end key.", "kv_key"),
                    param(
                        "limit",
                        "Optional integer limit clamped to the scan limits.",
                        "integer_string",
                    ),
                ],
                "Return non-reserved key/value pairs in lexicographic [start, endExclusive) order.",
                "object",
                vec![
                    "reserved boundary".to_string(),
                    "endExclusive must sort after start".to_string(),
                    "invalid limit".to_string(),
                ],
            ),
            "keys" => method_doc(
                "keys",
                method.kind(),
                vec![
                    param("prefix", "Inclusive key prefix.", "kv_key_prefix"),
                    param(
                        "limit",
                        "Optional integer limit clamped to the scan limits.",
                        "integer_string",
                    ),
                ],
                "Return non-reserved keys matching prefix, ordered by key.",
                "string[]",
                vec!["reserved prefix".to_string(), "invalid limit".to_string()],
            ),
            "public" => method_doc(
                "public",
                method.kind(),
                vec![param("key", "Public bucket key to read.", "kv_key")],
                "Read one value from the shared cross-app public bucket.",
                "string|null",
                vec!["absent key returns null".to_string()],
            ),
            "publicScan" => method_doc(
                "publicScan",
                method.kind(),
                vec![
                    param("prefix", "Inclusive key prefix.", "kv_key_prefix"),
                    param(
                        "limit",
                        "Optional integer limit clamped to the scan limits.",
                        "integer_string",
                    ),
                ],
                "Return public bucket key/value pairs whose keys start with prefix.",
                "object",
                vec!["invalid limit".to_string()],
            ),
            "publicAll" => method_doc(
                "publicAll",
                method.kind(),
                Vec::new(),
                "Return every key/value pair in the shared public bucket.",
                "object",
                vec!["Absent bucket returns an empty object.".to_string()],
            ),
            "publicKeys" => method_doc(
                "publicKeys",
                method.kind(),
                vec![
                    param("prefix", "Inclusive key prefix.", "kv_key_prefix"),
                    param(
                        "limit",
                        "Optional integer limit clamped to the scan limits.",
                        "integer_string",
                    ),
                ],
                "Return public bucket keys matching prefix, ordered by key.",
                "string[]",
                vec!["invalid limit".to_string()],
            ),
            other => unreachable!("unexpected kv resource method: {other}"),
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

fn optional_param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: false,
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
