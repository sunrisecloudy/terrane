use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, InternalNote, QueryDoc, ResourceDoc,
    ResourceMethodDoc, SchemaDoc,
};

fn scheduler_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut set = resource_method(
        "set",
        "write",
        &[
            param("name", "Stable schedule name within the app.", "scheduler_name"),
            param("specJson", "Canonical one-shot or cron schedule JSON.", "json"),
        ],
        "Create or replace one app-owned schedule.",
    );
    set.returns = "records scheduler.set".to_string();

    let mut clear = resource_method(
        "clear",
        "write",
        &[param("name", "Schedule name.", "scheduler_name")],
        "Clear one schedule.",
    );
    clear.returns = "records scheduler.cleared when the schedule exists".to_string();

    let mut list = resource_method("list", "read", &[], "List this app's schedules.");
    list.returns = "map of schedule name to JSON schedule object".to_string();

    let mut stat = resource_method(
        "stat",
        "read",
        &[param("name", "Schedule name.", "scheduler_name")],
        "Return one schedule's folded state.",
    );
    stat.returns = "JSON schedule object or null".to_string();

    vec![set, clear, list, stat]
}

pub fn scheduler_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "scheduler".to_string(),
        title: "Scheduler".to_string(),
        summary: "Deterministic schedule definitions plus host-recorded firing facts."
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
                "scheduler.set".to_string(),
                "scheduler.clear".to_string(),
                "scheduler.fire".to_string(),
            ],
            queries: vec!["scheduler.due".to_string()],
            events: vec![
                "scheduler.set".to_string(),
                "scheduler.cleared".to_string(),
                "scheduler.fired".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: scheduler_resource_methods(),
        },
        commands: scheduler_commands(),
        queries: scheduler_queries(),
        events: scheduler_events(),
        resources: vec![ResourceDoc {
            namespace: "scheduler".to_string(),
            summary: "App-scoped schedule management and state reads.".to_string(),
            methods: scheduler_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Schedule a daily backend verb".to_string(),
            summary: "Create a UTC cron wake-up from an app backend.".to_string(),
            language: "js".to_string(),
            code: "await ctx.resource.scheduler.set('daily', JSON.stringify({ cron: '0 9 * * *', verb: 'on_timer', args: ['daily-digest'] }));".to_string(),
            expected: "records scheduler.set; host ticks later record scheduler.fired then invoke the backend verb".to_string(),
        }],
        constraints: vec![
            "The core never reads a clock; due checks take now_ms as an argument.".to_string(),
            "Replay folds scheduler.set/scheduler.cleared/scheduler.fired and never re-derives firings.".to_string(),
            "scheduler.fire is trusted-host-only so apps cannot forge timer facts.".to_string(),
            "The host records scheduler.fired before invoking the app backend.".to_string(),
        ],
        limits: vec![
            limit("schedules", "32 per app", "Create or replace within the app limit."),
            limit("name", "128 bytes", "ASCII token: letters, digits, '.', '-' and '_'."),
            limit("specJson", "4 KiB", "Validated and canonicalized during decide."),
            limit("cron", "5-field UTC", "Minute-granularity standard cron fields."),
            limit("args", "16 strings", "Passed after name and scheduled_for."),
        ],
        compatibility: vec![
            "A fired one-shot is removed by fold.".to_string(),
            "Recurring catch-up emits one fire for the newest missed occurrence with skipped older occurrences counted.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Host follow-up".to_string(),
                body: "After scheduler.fire commits, the host invokes handle([verb, name, scheduledFor, ...args]); run errors are logged by the host and do not change scheduler state."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn scheduler_queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "scheduler.due",
        &[param("now_ms", "Caller-supplied epoch milliseconds.", "epoch_ms")],
        "JSON array of due {app,name,scheduled_for,skipped} objects",
        "Pure host query for schedules due at the supplied time.",
    )
    .with_errors(&["now_ms must be an unsigned integer", "invalid folded schedule spec"])]
}

fn scheduler_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "scheduler.set",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Schedule name.", "scheduler_name"),
                param("specJson", "Schedule spec JSON.", "json"),
            ],
            "commit",
            "Create or replace one app-owned schedule.",
        )
        .with_errors(&["app not found", "invalid spec", "too many schedules"])
        .with_emits(&["scheduler.set"]),
        command_doc(
            "scheduler.clear",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Schedule name.", "scheduler_name"),
            ],
            "commit",
            "Clear one schedule when it exists.",
        )
        .with_errors(&["invalid name"])
        .with_emits(&["scheduler.cleared"]),
        command_doc(
            "scheduler.fire",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Schedule name.", "scheduler_name"),
                param("scheduled_for", "Observed due epoch milliseconds.", "epoch_ms"),
                param("fired_at", "Observed fire epoch milliseconds.", "epoch_ms"),
                param("skipped", "Older missed occurrences collapsed into this fire.", "integer"),
            ],
            "commit",
            "Trusted host fact for one timer firing.",
        )
        .with_errors(&["unknown schedule", "requires trusted host authority"])
        .with_emits(&["scheduler.fired"]),
    ]
}

fn scheduler_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "scheduler.set",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Schedule name within the app.", "scheduler_name"),
                param("specJson", "Canonical schedule spec JSON.", "json"),
            ],
            "Records one schedule definition.",
        ),
        event_doc(
            "scheduler.cleared",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Schedule name within the app.", "scheduler_name"),
            ],
            "Removes one schedule definition.",
        ),
        event_doc(
            "scheduler.fired",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Schedule name within the app.", "scheduler_name"),
                param("scheduled_for", "Observed due epoch milliseconds.", "epoch_ms"),
                param("fired_at", "Observed fire epoch milliseconds.", "epoch_ms"),
                param("skipped", "Older missed occurrences.", "integer"),
            ],
            "Records one host-observed firing fact.",
        ),
    ]
}
