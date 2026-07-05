use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn scheduler_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut create = resource_method(
        "create",
        "write",
        &[
            param("id", "Stable schedule id within the app.", "scheduler_id"),
            param("cron", "Five-field cron expression.", "cron"),
            param(
                "timezone",
                "IANA-style timezone such as Asia/Bangkok.",
                "timezone",
            ),
            param("action", "App backend action verb to invoke.", "action"),
            param("payload", "JSON payload for the scheduled action.", "json"),
        ],
        "Create one app-owned schedule.",
    );
    create.returns = "records scheduler.created".to_string();

    let mut list = resource_method(
        "list",
        "read",
        &[],
        "List this app's schedules as id-to-JSON.",
    );
    list.returns = "map of schedule id to JSON schedule object".to_string();

    let mut pause = resource_method(
        "pause",
        "write",
        &[param("id", "Schedule id.", "scheduler_id")],
        "Pause one schedule.",
    );
    pause.returns = "records scheduler.paused".to_string();

    let mut resume = resource_method(
        "resume",
        "write",
        &[param("id", "Schedule id.", "scheduler_id")],
        "Resume one schedule.",
    );
    resume.returns = "records scheduler.resumed".to_string();

    let mut remove = resource_method(
        "remove",
        "write",
        &[param("id", "Schedule id.", "scheduler_id")],
        "Remove one schedule that has no active run.",
    );
    remove.returns = "records scheduler.removed".to_string();

    let mut history = resource_method(
        "history",
        "read",
        &[
            param(
                "id",
                "Schedule id, or empty for all schedules.",
                "scheduler_id",
            ),
            param("limit", "Maximum number of recent runs.", "integer"),
        ],
        "Return recent scheduler runs as JSON strings.",
    );
    history.returns = "list of JSON run objects, newest first".to_string();

    vec![create, list, pause, resume, remove, history]
}

pub fn scheduler_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "scheduler".to_string(),
        title: "App Scheduler".to_string(),
        summary: "Deterministic schedule definitions plus host-recorded QuickJS action run history."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "scheduler.create".to_string(),
                "scheduler.pause".to_string(),
                "scheduler.resume".to_string(),
                "scheduler.remove".to_string(),
                "scheduler.run.start".to_string(),
                "scheduler.run.complete".to_string(),
                "scheduler.run.fail".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "scheduler.created".to_string(),
                "scheduler.paused".to_string(),
                "scheduler.resumed".to_string(),
                "scheduler.removed".to_string(),
                "scheduler.run.started".to_string(),
                "scheduler.run.completed".to_string(),
                "scheduler.run.failed".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: scheduler_resource_methods(),
        },
        commands: scheduler_commands(),
        queries: Vec::new(),
        events: scheduler_events(),
        resources: vec![ResourceDoc {
            namespace: "scheduler".to_string(),
            summary: "App-scoped schedule management and run-history reads.".to_string(),
            methods: scheduler_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Create an every-minute action".to_string(),
            summary: "Create the QuickJS ops proof heartbeat schedule from an app backend.".to_string(),
            language: "js".to_string(),
            code: "await ctx.resource.scheduler.create('quickjs-ops-heartbeat', '* * * * *', 'Asia/Bangkok', 'opsHeartbeat', { source: 'premium-ops-proof' });".to_string(),
            expected: "records scheduler.created; host ticks later record scheduler.run.* facts".to_string(),
        }],
        constraints: vec![
            "Schedule definitions are deterministic event-log state.".to_string(),
            "Clock ticks are host input; replay folds scheduler.run.* facts and never re-runs timers or JavaScript.".to_string(),
            "Host-owned run commands are trusted-host-only so apps cannot forge execution facts.".to_string(),
            "The scheduler invokes app backend actions through the app's runtime, not shell commands or raw JS strings.".to_string(),
        ],
        limits: vec![
            limit(
                "cron",
                "minute forms",
                "The initial host due calculator supports '* * * * *', '*/n * * * *', and 'm * * * *'.",
            ),
            limit(
                "payload",
                "JSON",
                "Payloads are stored as JSON facts and passed to the action as one JSON argument.",
            ),
        ],
        compatibility: vec![
            "Terminal run events carry the next due time, so old logs replay without asking the wall clock.".to_string(),
            "The public contract exposes ctx.resource.scheduler; Premium should consume that surface instead of private host semantics.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Trusted host boundary".to_string(),
                body: "scheduler.run.start/complete/fail are for host scheduler loops only; public capability_command refuses them."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn scheduler_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "scheduler.create",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
                param("cron", "Five-field cron expression.", "cron"),
                param("timezone", "IANA-style timezone.", "timezone"),
                param("action", "Backend action verb.", "action"),
                param("payloadJson", "JSON payload.", "json"),
            ],
            "commit",
            "Create one app-owned schedule.",
        )
        .with_errors(&[
            "app not found",
            "duplicate schedule",
            "invalid cron",
            "invalid timezone",
            "invalid JSON",
        ])
        .with_emits(&["scheduler.created"]),
        command_doc(
            "scheduler.pause",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
            ],
            "commit",
            "Pause one schedule.",
        )
        .with_errors(&["unknown schedule"])
        .with_emits(&["scheduler.paused"]),
        command_doc(
            "scheduler.resume",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
            ],
            "commit",
            "Resume one schedule.",
        )
        .with_errors(&["unknown schedule"])
        .with_emits(&["scheduler.resumed"]),
        command_doc(
            "scheduler.remove",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
            ],
            "commit",
            "Remove one inactive schedule.",
        )
        .with_errors(&["unknown schedule", "active run"])
        .with_emits(&["scheduler.removed"]),
        command_doc(
            "scheduler.run.start",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
                param("runId", "Host-generated run id.", "scheduler_run_id"),
                param("now", "Host clock epoch seconds.", "epoch_seconds"),
            ],
            "commit",
            "Trusted host claim for one due schedule run.",
        )
        .with_errors(&[
            "unknown schedule",
            "paused schedule",
            "not due",
            "active run",
            "duplicate run",
        ])
        .with_emits(&["scheduler.run.started"]),
        command_doc(
            "scheduler.run.complete",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
                param("runId", "Run id.", "scheduler_run_id"),
                param("finishedAt", "Host clock epoch seconds.", "epoch_seconds"),
                param("outputJson", "JSON output summary.", "json"),
            ],
            "commit",
            "Trusted host success fact for a claimed run.",
        )
        .with_errors(&[
            "unknown schedule",
            "unknown run",
            "run is not active",
            "invalid JSON",
        ])
        .with_emits(&["scheduler.run.completed"]),
        command_doc(
            "scheduler.run.fail",
            &[
                param("app", "Existing app id.", "app_id"),
                param("id", "Schedule id.", "scheduler_id"),
                param("runId", "Run id.", "scheduler_run_id"),
                param("finishedAt", "Host clock epoch seconds.", "epoch_seconds"),
                param("errorJson", "JSON error summary.", "json"),
            ],
            "commit",
            "Trusted host failure fact for a claimed run.",
        )
        .with_errors(&[
            "unknown schedule",
            "unknown run",
            "run is not active",
            "invalid JSON",
        ])
        .with_emits(&["scheduler.run.failed"]),
    ]
}

fn scheduler_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "scheduler.created",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
                param("cron", "Five-field cron expression.", "cron"),
                param("timezone", "IANA-style timezone.", "timezone"),
                param("action", "Backend action verb.", "action"),
                param("payloadJson", "JSON payload for the action.", "json"),
                param("nextDueAt", "Next due epoch seconds.", "epoch_seconds"),
            ],
            "Records one app-owned schedule definition.",
        ),
        event_doc(
            "scheduler.paused",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
            ],
            "Marks one schedule paused.",
        ),
        event_doc(
            "scheduler.resumed",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
            ],
            "Marks one schedule active.",
        ),
        event_doc(
            "scheduler.removed",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
            ],
            "Removes one schedule definition.",
        ),
        event_doc(
            "scheduler.run.started",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
                param("runId", "Host-generated run id.", "scheduler_run_id"),
                param("action", "Backend action verb.", "action"),
                param("payloadJson", "JSON payload for the action.", "json"),
                param("dueAt", "Due epoch seconds.", "epoch_seconds"),
                param("startedAt", "Host clock epoch seconds.", "epoch_seconds"),
            ],
            "Claims one due run for host execution.",
        ),
        event_doc(
            "scheduler.run.completed",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
                param("runId", "Run id.", "scheduler_run_id"),
                param("finishedAt", "Host clock epoch seconds.", "epoch_seconds"),
                param("nextDueAt", "Next due epoch seconds.", "epoch_seconds"),
                param("outputJson", "JSON output summary.", "json"),
            ],
            "Records a successful app action run.",
        ),
        event_doc(
            "scheduler.run.failed",
            &[
                param("app", "Owning app id.", "app_id"),
                param("id", "Schedule id within the app.", "scheduler_id"),
                param("runId", "Run id.", "scheduler_run_id"),
                param("finishedAt", "Host clock epoch seconds.", "epoch_seconds"),
                param("nextDueAt", "Next due epoch seconds.", "epoch_seconds"),
                param("errorJson", "JSON error summary.", "json"),
            ],
            "Records a failed app action run.",
        ),
    ]
}
