use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, InternalNote, QueryDoc, ResourceDoc,
    ResourceMethodDoc, SchemaDoc,
};

fn geo_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut current = resource_method(
        "current",
        "call",
        &[param("precision", "Granted precision tier: exact or coarse.", "string")],
        "Recorded one-shot location read; the edge observes OS/browser location, rounds before record, emits geo.observed, and returns fix JSON.",
    );
    current.returns = "fix JSON string".to_string();
    let mut peek = resource_method(
        "peek",
        "call",
        &[param("precision", "Granted precision tier: exact or coarse.", "string")],
        "Live, unrecorded one-shot location read for display-only uses; the response never enters the event log.",
    );
    peek.returns = "fix JSON string".to_string();
    let mut last = resource_method(
        "last",
        "read",
        &[],
        "Pure read of the app's newest recorded fix, or null.",
    );
    last.returns = "fix JSON string, or null".to_string();
    vec![current, peek, last]
}

pub fn geo_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "geo".to_string(),
        title: "Recorded Geolocation".to_string(),
        summary: "Replay-safe one-shot location reads. The edge observes location once, rounds it before event construction, and replay folds the recorded fact without touching a location provider."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["geo.locate".to_string()],
            queries: vec!["geo.supports".to_string()],
            events: vec!["geo.observed".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: geo_resource_methods(),
        },
        commands: geo_commands(),
        queries: geo_queries(),
        events: geo_events(),
        resources: vec![ResourceDoc {
            namespace: "geo".to_string(),
            summary: "Recorded and live one-shot location reads for app backends.".to_string(),
            methods: geo_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Record an approximate fix".to_string(),
            summary: "A backend calls ctx.resource.geo.current(\"coarse\"); the edge rounds before recording geo.observed."
                .to_string(),
            language: "js".to_string(),
            code: "var fix = JSON.parse(ctx.resource.geo.current(\"coarse\"));".to_string(),
            expected: "records geo.observed with coarse coordinates only; replay folds it without geolocation"
                .to_string(),
        }],
        constraints: vec![
            "geo.locate validates the app exists and precision is exact or coarse before returning Effect::GeoLocate."
                .to_string(),
            "Coarse precision rounds lat/lon to 0.01 degrees and raises accuracy_m to at least 1000 before geo.observed is constructed."
                .to_string(),
            "Replay folds recorded geo.observed events verbatim and never calls OS or browser geolocation."
                .to_string(),
            "geo.peek is transient and records nothing.".to_string(),
            "Folding app.removed removes all folded fixes for that app.".to_string(),
            "describe() redacts coordinates and prints only app, precision, accuracy, and observed_at."
                .to_string(),
        ],
        limits: vec![
            limit(
                "fixesPerAppInState",
                &super::types::MAX_FIXES_PER_APP.to_string(),
                "Newest folded fixes kept per app; older folded state entries are deterministically truncated.",
            ),
            limit(
                "recordedLocateRate",
                "1 per app per 10 seconds",
                "geo.locate/current are rejected when the newest folded observed_at is inside the rate window; geo.peek is transient.",
            ),
        ],
        compatibility: vec![
            "CLI host returns a typed unsupported edge error for geo.locate/geo.peek; native and web hosts can provide platform geolocation edges later."
                .to_string(),
            "App removal cleanup is driven by the app.removed subscription and does not require a geo-specific command."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::GeoLocate is the edge boundary. geo.observed is the durable replay input and stores integer e7 coordinates after precision has already been applied."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn geo_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "geo.locate",
        &[
            param("app", "Existing app id that owns the recorded observation.", "app_id"),
            param("precision", "Granted precision tier: exact or coarse.", "string"),
        ],
        "effect",
        "Validate one app-scoped location observation and return the edge effect.",
    )
    .with_errors(&["app not found", "bad precision", "rate limited"])
    .with_effects(&["GeoLocate"])
    .with_emits(&["geo.observed"])]
}

fn geo_queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "geo.supports",
        &[],
        "bool",
        "Whether the folded host platform reports geolocation support.",
    )
    .with_errors(&["none"])]
}

fn geo_events() -> Vec<EventDoc> {
    vec![event_doc(
        "geo.observed",
        &[
            param("app", "App id that requested the observation.", "app_id"),
            param("lat_e7", "Observed latitude in integer e7 degrees.", "i64"),
            param("lon_e7", "Observed longitude in integer e7 degrees.", "i64"),
            param("accuracy_m", "Accuracy radius in meters.", "u32"),
            param("precision", "Precision tier applied before recording.", "string"),
            param("observed_at", "Edge wall-clock milliseconds at observation time.", "u64"),
        ],
        "Records one observed location fix for replay.",
    )
    .with_effects(&["appends GeoState.fixes[app], keeping newest 20"])]
}
