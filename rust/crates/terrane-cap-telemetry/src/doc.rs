use terrane_cap_interface::{
    event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    EventDoc, ExampleDoc, InternalNote, ParamDoc, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn telemetry_resource_methods() -> Vec<ResourceMethodDoc> {
    fn mk(name: &'static str, summary: &str) -> ResourceMethodDoc {
        let mut m = resource_method(
            name,
            "call",
            &[
                param("msg", "Log message (string).", "string"),
                ParamDoc {
                    name: "dataJson".to_string(),
                    summary: "Optional structured payload; encoded into the ring buffer only, never into the event log.".to_string(),
                    required: false,
                    schema_ref: "json".to_string(),
                },
            ],
            summary,
        );
        m.returns = "null".to_string();
        m
    }
    let debug = mk("debug", "Ring-buffer only; never recorded and never permission-gated when granted.");
    let info = mk("info", "Ring-buffer only; never recorded.");
    let warn = mk("warn", "Ring-buffer only; never recorded.");
    let error = mk("error", "Ring-buffer + one recorded telemetry.error event (the crash fact worth its bytes).");
    let mut read = resource_method(
        "read",
        "read",
        &[
            ParamDoc {
                name: "level".to_string(),
                summary: "Optional level filter (debug/info/warn/error).".to_string(),
                required: false,
                schema_ref: "string".to_string(),
            },
            ParamDoc {
                name: "tail".to_string(),
                summary: "Optional max number of entries to return (default 200).".to_string(),
                required: false,
                schema_ref: "usize".to_string(),
            },
        ],
        "Read THIS app's own ring buffer. Cross-app reads are host/owner surfaces only.",
    );
    read.returns = "JSON {lines: [{ts, level, msg, data, source?}]}".to_string();
    vec![debug, info, warn, error, read]
}

pub fn telemetry_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "telemetry".to_string(),
        title: "App Logging + Error Reporting".to_string(),
        summary: "Structured per-app logging through a host-side ring buffer, plus a recorded telemetry.error event for crash facts (app-direct errors and auto-captured exceptions). Debug chatter folds into no state; replay reproduces error counts and the last 20 error facts per app from the event log alone."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: vec!["telemetry.error".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: telemetry_resource_methods(),
        },
        commands: Vec::new(),
        queries: Vec::new(),
        events: telemetry_events(),
        resources: vec![ResourceDoc {
            namespace: "telemetry".to_string(),
            summary: "Structured app logging and reading back this app's own log buffer."
                .to_string(),
            methods: telemetry_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Log and read back".to_string(),
            summary: "A backend logs at every level; the agent later reads its own buffer to self-debug."
                .to_string(),
            language: "js".to_string(),
            code: r#"ctx.resource.telemetry.info("started", JSON.stringify({verb: input[0]}));
ctx.resource.telemetry.error("boom", JSON.stringify({step: 3}));
var lines = ctx.resource.telemetry.read("warn");"#
                .to_string(),
            expected: "info/warn entries land in $TERRANE_HOME/logs/<app>/current.jsonl only; error also records one telemetry.error event."
                .to_string(),
        }],
        constraints: vec![
            "Debug/info/warn are transient: Effect::AppLog as a TransientEffect — the edge appends the line to the per-app ring buffer and records NOTHING. Replay never re-runs them."
                .to_string(),
            "Error is a recorded Decision::Effect whose runner appends the line to the buffer AND returns one telemetry.error event; replay folds counts and last-error facts from the log alone."
                .to_string(),
            "Auto-capture (a backend exception, a budget-interrupt timeout, or the resource first_error slot) mirrors the same line with source = exception | timeout | first_error and emits one telemetry.error event when the app grants telemetry; the buffer keeps every occurrence."
                .to_string(),
            "dataJson may contain user data — that is why it stays in the local jsonl and only a sha256 digest enters the (syncable) event log."
                .to_string(),
            "Folding app.removed removes the app's error_count and last_errors slice; the edge also deletes logs/<app>/. No telemetry-specific command is needed."
                .to_string(),
            "Logs never leave the machine except through local host routes (CLI logs, MCP app_logs, owner-only dev-panel route, app read of its own buffer). One line in doc.rs, load-bearing for privacy."
                .to_string(),
            "The console shim is installed whether or not telemetry is granted; the grant gates recording (and read), not the buffer write. Logging never triggers a permission prompt — reading does."
                .to_string(),
        ],
        limits: vec![
            limit(
                "msgBytes",
                &format!("{}", super::MAX_MSG_BYTES),
                "msg argument truncated (with marker) — a log call should not crash the app.",
            ),
            limit(
                "dataBytes",
                &format!("{}", super::MAX_DATA_BYTES),
                "dataJson truncated (with marker) — never errored, only into the buffer.",
            ),
            limit(
                "stackBytes",
                &format!("{}", super::MAX_STACK_BYTES),
                "stack text in telemetry.error events truncated (with marker).",
            ),
            limit(
                "errorsPerRun",
                &format!("{}", super::MAX_ERRORS_PER_RUN),
                "Recorded telemetry.error calls per backend run — a backstop against a runaway error loop bloating the event log.",
            ),
            limit(
                "lastErrors",
                &format!("{}", super::LAST_ERRORS_RING),
                "ErrorFact ring kept per app in TelemetryState; older ones drop off the front.",
            ),
            limit(
                "ringRotateBytes",
                &format!("{}", super::RING_ROTATE_BYTES),
                "Per-app ring buffer rotates at this size; RING_ROTATE_KEEP older files retained (≈16 MiB/app hard ceiling).",
            ),
        ],
        compatibility: vec![
            "Replay reproduces TelemetryState (error_count + last_errors) from telemetry.error events alone; the jsonl files are non-authoritative artifacts a fresh replica simply does not have."
                .to_string(),
            "App removal cleanup is driven by the app.removed subscription and does not require a telemetry-specific command."
                .to_string(),
            "The buffer is written only by the host edge; the core never opens it — same stance as blobs.sqlite3."
                .to_string(),
        ],
        internal: if include_internal {
            vec![
                InternalNote {
                    title: "Replay boundary".to_string(),
                    body: "Effect::AppLog for debug/info/warn is transient (never recorded); for error it is a recorded Effect that returns one telemetry.error event. Auto-capture builds the event directly via terrane_cap_telemetry::error_event (no Effect runner round-trip), so crash facts are folded from the log on replay without re-running JS."
                        .to_string(),
                },
                InternalNote {
                    title: "Source tagging".to_string(),
                    body: "telemetry.error carries source = explicit | exception | timeout | first_error. Effect::AppLog stays {app, level, msg, data}; the source is chosen by the emit path (app-direct decide for explicit, the js-runtime edge for auto-capture), keeping the effect payload per the plan's shape.".to_string(),
                },
                InternalNote {
                    title: "Per-run error dedup (deviation)".to_string(),
                    body: "The plan calls for edge dedup of identical (app, message) within a run for telemetry.error events. v1 emits one event per error call (the buffer keeps every occurrence as required). Implementing the per-run dedup needs a per-run stash that the shared Core-level runner arc does not naturally scope; deferred.".to_string(),
                },
            ]
        } else {
            Vec::new()
        },
    }
}

fn telemetry_events() -> Vec<EventDoc> {
    vec![event_doc(
        "telemetry.error",
        &[
            param("app", "App id that emitted the error.", "app_id"),
            param("source", "explicit | exception | timeout | first_error.", "string"),
            param("message", "Error message, truncated to MAX_MSG_BYTES.", "string"),
            param("stack", "Stack trace when available, truncated to MAX_STACK_BYTES.", "string"),
            param(
                "data_digest",
                "sha256 of the dataJson carried only in the local jsonl; the data itself never enters the syncable log.",
                "hex",
            ),
        ],
        "Records one crash fact for replay; replay folds error_count and the last 20 ErrorFacts.",
    )
    .with_effects(&[
        "stores TelemetryState.error_count[app]++",
        "pushes one ErrorFact, popping front when > LAST_ERRORS_RING",
        "app.removed clears the app's slice (subscription)",
    ])]
}