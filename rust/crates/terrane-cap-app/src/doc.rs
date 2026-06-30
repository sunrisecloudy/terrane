use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ParamDoc, QueryDoc, ResourceDoc, SchemaDoc,
};

pub fn app_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "app".to_string(),
        title: "App Catalog".to_string(),
        summary: "Deterministic catalog of saved Terrane apps and their runtime entrypoints."
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
                "app.add".to_string(),
                "app.import".to_string(),
                "app.remove".to_string(),
            ],
            queries: vec!["app.exists".to_string()],
            events: vec!["app.added".to_string(), "app.removed".to_string()],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: app_commands(),
        queries: app_queries(),
        events: app_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![
            ExampleDoc {
                title: "Register an app bundle".to_string(),
                summary: "Create a catalog entry with a stable app id, display name, source, and runtime."
                    .to_string(),
                language: "cli".to_string(),
                code: "terrane app add calendar Calendar --source /apps/calendar --runtime js"
                    .to_string(),
                expected: "records app.added and makes app.exists(calendar) true".to_string(),
            },
            ExampleDoc {
                title: "Remove an app".to_string(),
                summary: "Remove the catalog entry and publish the cleanup signal consumed by app-scoped capabilities."
                    .to_string(),
                language: "cli".to_string(),
                code: "terrane app remove calendar".to_string(),
                expected: "records app.removed; subscribers clean their own app-scoped state on fold"
                    .to_string(),
            },
        ],
        constraints: vec![
            "app.add validates id, name, and runtime before recording app.added.".to_string(),
            "App ids under __terrane/ are reserved for platform-owned logical stores."
                .to_string(),
            "app.import is effectful: the edge host reads a JS bundle directory, records app.added with a kv:// source, and stores bundle files under reserved cap-kv keys."
                .to_string(),
            "app.remove only records app.removed for an existing app id.".to_string(),
            "Replay rebuilds the catalog solely from app.added and app.removed events.".to_string(),
            "app.exists is a derived query over folded AppState and is never recorded as an event."
                .to_string(),
            "App removal cleanup is intentionally fan-out: each subscriber removes its own app-scoped state while folding app.removed."
                .to_string(),
        ],
        limits: vec![
            limit("defaultRuntime", "js", "Keeps older app.add calls deterministic."),
            limit("catalogScope", "home", "The catalog is local to the current TERRANE_HOME."),
        ],
        compatibility: vec![
            "Other capabilities that store app-scoped data must subscribe to app.removed and treat it as their cleanup boundary."
                .to_string(),
            "The app capability does not delete bundles or external files; it records catalog state only."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Removal boundary".to_string(),
                body: "app.removed is the durable removal signal. Cascading cleanup belongs to subscribers so replay stays capability-local."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn app_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "app.add",
            &[
                param("id", "Stable app id.", "app_id"),
                param("name", "Human-readable app name.", "string"),
                optional_param("source", "Optional app bundle path.", "path"),
                optional_param(
                    "runtime",
                    "Optional runtime name; defaults to js.",
                    "string",
                ),
            ],
            "commit",
            "Record a saved app catalog entry.",
        )
        .with_errors(&["empty id", "empty name", "empty runtime", "duplicate app"])
        .with_emits(&["app.added"])
        .with_examples(&[ExampleDoc {
            title: "Register through MCP".to_string(),
            summary: "Use the same ordered argv vector with dryRun first, then without dryRun to commit."
                .to_string(),
            language: "json".to_string(),
            code: r#"{"name":"app.add","args":["calendar","Calendar","--source","/apps/calendar","--runtime","js"],"dryRun":true}"#
                .to_string(),
            expected: r#"{"dryRun":true,"records":1}"#.to_string(),
        }]),
        command_doc(
            "app.import",
            &[
                param("source", "JS bundle directory containing manifest.json.", "path"),
                optional_param(
                    "storage",
                    "Optional cap-kv backend for this app: memory, sqlite, or rocksdb.",
                    "string",
                ),
                optional_param(
                    "path",
                    "Optional storage backend path, relative to TERRANE_HOME unless absolute.",
                    "path",
                ),
            ],
            "effect",
            "Import a JS bundle into reserved cap-kv keys and catalog it with a kv:// source.",
        )
        .with_errors(&[
            "missing bundle path",
            "unsafe manifest id",
            "non-js runtime",
            "duplicate app",
            "unavailable storage backend feature when rocksdb is selected without rocksdb-storage",
            "non-UTF-8 or binary bundle file",
        ])
        .with_emits(&["kv.storage.configured", "app.added", "kv.set"])
        .with_effects(&["reads bundle files from the host filesystem once"])
        .with_examples(&[ExampleDoc {
            title: "Import bundle into cap-kv".to_string(),
            summary: "Store the bundle in reserved kv keys; by default, those keys project to the app's SQLite-backed KV store."
                .to_string(),
            language: "json".to_string(),
            code: r#"{"name":"app.import","args":["/apps/calendar","--storage","sqlite","--path","apps/calendar.sqlite3"]}"#
                .to_string(),
            expected: "records kv.storage.configured, app.added, and kv.set bundle file events"
                .to_string(),
        }]),
        command_doc(
            "app.remove",
            &[param("id", "Existing app id.", "app_id")],
            "commit",
            "Remove one app catalog entry and publish the app-scoped cleanup signal.",
        )
        .with_errors(&["missing app id", "app not found"])
        .with_emits(&["app.removed"])
        .with_examples(&[ExampleDoc {
            title: "Remove through MCP".to_string(),
            summary: "Pass the app id as the only argv element.".to_string(),
            language: "json".to_string(),
            code: r#"{"name":"app.remove","args":["calendar"],"dryRun":true}"#.to_string(),
            expected: r#"{"dryRun":true,"records":1}"#.to_string(),
        }]),
    ]
}

fn app_queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "app.exists",
        &[param("app", "App id to check.", "app_id")],
        "bool",
        "Return whether the folded app catalog contains the app id.",
    )
    .with_errors(&["missing app id"])]
}

fn app_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "app.added",
            &[
                param("id", "Stable app id.", "app_id"),
                param("name", "Human-readable app name.", "string"),
                optional_param("source", "Optional app bundle path.", "path"),
                param("runtime", "Runtime name used to launch the app.", "string"),
            ],
            "Adds or replaces the folded catalog record for one app id.",
        )
        .with_effects(&["folds into AppState.apps"]),
        event_doc(
            "app.removed",
            &[param("id", "Removed app id.", "app_id")],
            "Removes the folded catalog record and signals subscribers to clean app-scoped state.",
        )
        .with_effects(&["removes AppState.apps[id]", "broadcast cleanup boundary"]),
    ]
}

fn optional_param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: false,
        schema_ref: schema_ref.to_string(),
    }
}
