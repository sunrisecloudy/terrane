use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn time_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut now = resource_method(
        "now",
        "call",
        &[],
        "Recorded wall-clock read; the edge observes SystemTime, emits time.observed, and returns the epoch-ms decimal string.",
    );
    now.returns = "epoch-ms decimal string".to_string();
    let mut live = resource_method(
        "live",
        "call",
        &[],
        "Live, unrecorded wall-clock read; the response never enters the log (not replay-stable) — for display-only timestamps.",
    );
    live.returns = "epoch-ms decimal string".to_string();
    let mut last = resource_method(
        "last",
        "read",
        &[],
        "Pure read of the app's last recorded observation, or null.",
    );
    last.returns = "epoch-ms decimal string, or null".to_string();
    vec![now, live, last]
}

pub fn time_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "time".to_string(),
        title: "Recorded Wall-clock".to_string(),
        summary:
            "Replay-safe wall-clock reads. The edge observes time once and records the observation; replay folds the recorded fact and never consults a clock."
                .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["time.now".to_string()],
            queries: Vec::new(),
            events: vec!["time.observed".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: time_resource_methods(),
        },
        commands: time_commands(),
        queries: Vec::new(),
        events: time_events(),
        resources: vec![ResourceDoc {
            namespace: "time".to_string(),
            summary: "Recorded and live wall-clock reads for app backends.".to_string(),
            methods: time_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Observe and record the clock".to_string(),
            summary: "A backend calls ctx.resource.time.now(); the edge reads SystemTime and the recorded time.observed is replayed from the log."
                .to_string(),
            language: "js".to_string(),
            code: "var now = ctx.resource.time.now(); // e.g. \"1700000000000\""
                .to_string(),
            expected: "records time.observed { app, epoch_ms }; replay folds it without a clock"
                .to_string(),
        }],
        constraints: vec![
            "time.now() validates the app exists, returns Effect::ObserveTime, and lets the edge read the clock; replay folds time.observed and reads no clock."
                .to_string(),
            "A completed observation is recorded as time.observed with app id and epoch_ms (UTC)."
                .to_string(),
            "Replay folds recorded time.observed events into per-app last-seen state."
                .to_string(),
            "Folding app.removed removes the app's last-seen entry.".to_string(),
            "Values are UTC epoch milliseconds only, returned as decimal strings; formatting and timezones are the UI's job."
                .to_string(),
            "No monotonicity guarantee: the host clock may step back under NTP correction. Consumers needing ordering must use event-log order, which is total."
                .to_string(),
            "Date.now() in the QuickJS sandbox is live and bypasses this capability — apps that need replay-stable timestamps must call ctx.resource.time.now() instead. Hardening the sandbox is a separate, confirmable decision."
                .to_string(),
        ],
        limits: vec![
            limit(
                "recordedObservationsPerRun",
                &super::MAX_OBSERVATIONS_PER_RUN.to_string(),
                "Recorded time.now() calls per backend run (soft cap so a loop can't bloat the log); transient time.live() is uncapped.",
            ),
            limit(
                "epochMs",
                "u64 (post-1970)",
                "The edge fails with a typed error if the wall clock reads before the Unix epoch.",
            ),
        ],
        compatibility: vec![
            "Clock availability is outside replay; deterministic behavior depends on recording time.observed once at the edge."
                .to_string(),
            "App removal cleanup is driven by the app.removed subscription and does not require a time-specific command."
                .to_string(),
        ],
        internal: if include_internal {
            vec![
                InternalNote {
                    title: "Replay boundary".to_string(),
                    body: "Effect::ObserveTime is transient. time.observed is the durable replay input and stores the observed epoch_ms."
                        .to_string(),
                },
                InternalNote {
                    title: "Per-run cap enforcement".to_string(),
                    body: "The per-run cap on recorded time.now() calls is enforced by the runtime host (RuntimeResourceHost), which is fresh per backend run — the natural per-run scope. Decide stays pure; the host refuses the (limit+1)-th recorded call with a typed error naming time.live() as the escape hatch."
                        .to_string(),
                },
            ]
        } else {
            Vec::new()
        },
    }
}

fn time_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "time.now",
        &[param(
            "app",
            "Existing app id that owns the recorded observation.",
            "app_id",
        )],
        "effect",
        "Validate one app-scoped wall-clock observation and return the edge effect.",
    )
    .with_errors(&["app not found"])
    .with_effects(&["ObserveTime"])
    .with_emits(&["time.observed"])]
}

fn time_events() -> Vec<EventDoc> {
    vec![event_doc(
        "time.observed",
        &[
            param("app", "App id that requested the observation.", "app_id"),
            param("epoch_ms", "Observed UTC epoch milliseconds.", "u64"),
        ],
        "Records one observed wall-clock reading for replay.",
    )
    .with_effects(&["stores TimeState.last[app]"])]
}