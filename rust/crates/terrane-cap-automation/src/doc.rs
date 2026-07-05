use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, InternalNote, QueryDoc, ResourceDoc,
    ResourceMethodDoc, SchemaDoc,
};

fn automation_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut set = resource_method(
        "set",
        "write",
        &[
            param("name", "Stable rule name within the app.", "automation_name"),
            param("ruleJson", "Event trigger rule JSON.", "json"),
        ],
        "Create or replace one app-owned event automation rule.",
    );
    set.returns = "records automation.set".to_string();

    let mut rm = resource_method(
        "rm",
        "write",
        &[param("name", "Rule name.", "automation_name")],
        "Remove one automation rule.",
    );
    rm.returns = "records automation.removed when the rule exists".to_string();

    let mut list = resource_method("list", "read", &[], "List this app's automation rules.");
    list.returns = "map of rule name to JSON rule object".to_string();

    let mut stat = resource_method(
        "stat",
        "read",
        &[param("name", "Rule name.", "automation_name")],
        "Return one rule's folded state.",
    );
    stat.returns = "JSON rule object or null".to_string();

    vec![set, rm, list, stat]
}

pub fn automation_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "automation".to_string(),
        title: "Automation".to_string(),
        summary: "Deterministic event-triggered rule facts plus host-recorded firings."
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
                "automation.set".to_string(),
                "automation.rm".to_string(),
                "automation.fire".to_string(),
                "automation.suppress".to_string(),
            ],
            queries: vec!["automation.list".to_string(), "automation.stat".to_string()],
            events: vec![
                "automation.set".to_string(),
                "automation.removed".to_string(),
                "automation.fired".to_string(),
                "automation.suppressed".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: automation_resource_methods(),
        },
        commands: automation_commands(),
        queries: automation_queries(),
        events: automation_events(),
        resources: vec![ResourceDoc {
            namespace: "automation".to_string(),
            summary: "App-scoped event automation rule management and state reads.".to_string(),
            methods: automation_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Run summarize when inbox KV changes".to_string(),
            summary: "Create a rule that watches the app's own inbox keys.".to_string(),
            language: "js".to_string(),
            code: "ctx.resource.automation.set('inbox-summary', JSON.stringify({ trigger: { kind: 'kv.set', filter: \"starts_with(payload.key, 'inbox/')\" }, action: { verb: 'summarize', argsTemplate: ['{{payload.key}}'] }, cooldownMs: 1000 }));".to_string(),
            expected: "host tick records automation.fired and invokes handle(['summarize', key])".to_string(),
        }],
        constraints: vec![
            "Replay folds automation facts and never evaluates rule matchers.".to_string(),
            "automation.fire and automation.suppress are trusted-host-only.".to_string(),
            "Filters use the platform JMESPath evaluator over {kind, actor, payload}.".to_string(),
            "Cross-app triggers require an existing grant before automation.set succeeds.".to_string(),
        ],
        limits: vec![
            limit("rules", "32 per app", "Create or replace within the app limit."),
            limit("name", "128 bytes", "ASCII token: letters, digits, '.', '-' and '_'."),
            limit("ruleJson", "8 KiB", "Validated and canonicalized during decide."),
            limit("cooldown", "minimum 1000 ms", "Lower values are raised to the floor."),
            limit("fire budget", "8 per matcher pass", "Extra matches record automation.suppressed."),
        ],
        compatibility: vec![
            "Push subscriptions should later converge onto automation rules with notify actions."
                .to_string(),
            "Time triggers remain in scheduler; automation may invoke app verbs that set schedules."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Host follow-up".to_string(),
                body: "After automation.fire commits, the host invokes handle([verb, ...renderedArgs]); run errors do not change automation state.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn automation_queries() -> Vec<QueryDoc> {
    vec![
        query_doc(
            "automation.list",
            &[param("app", "Existing app id.", "app_id")],
            "JSON object keyed by rule name",
            "Read folded automation rules for one app.",
        )
        .with_errors(&["app not found"]),
        query_doc(
            "automation.stat",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Rule name.", "automation_name"),
            ],
            "JSON rule object or null",
            "Read one folded automation rule.",
        )
        .with_errors(&["app not found"]),
    ]
}

fn automation_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "automation.set",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Rule name.", "automation_name"),
                param("rule_json", "Rule JSON.", "json"),
            ],
            "commit",
            "Create or replace one event-triggered rule.",
        )
        .with_errors(&["app not found", "invalid rule", "too many rules"])
        .with_emits(&["automation.set"]),
        command_doc(
            "automation.rm",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Rule name.", "automation_name"),
            ],
            "commit",
            "Remove one rule when it exists.",
        )
        .with_errors(&["invalid name"])
        .with_emits(&["automation.removed"]),
        command_doc(
            "automation.fire",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Rule name.", "automation_name"),
                param("rule_hash", "Hash of the canonical rule.", "sha256"),
                param("event_ref", "Stable hash of the triggering event.", "sha256"),
                param("fired_at", "Observed fire epoch milliseconds.", "epoch_ms"),
            ],
            "commit",
            "Trusted host fact for one rule firing.",
        )
        .with_errors(&["unknown rule", "stale rule hash", "requires trusted host authority"])
        .with_emits(&["automation.fired"]),
        command_doc(
            "automation.suppress",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Rule name.", "automation_name"),
                param("rule_hash", "Hash of the canonical rule.", "sha256"),
                param("event_ref", "Stable hash of the triggering event.", "sha256"),
                param("suppressed_at", "Observed suppression epoch milliseconds.", "epoch_ms"),
                param("reason", "Suppression reason.", "token"),
            ],
            "commit",
            "Trusted host fact for a visible skipped firing.",
        )
        .with_errors(&["unknown rule", "stale rule hash", "requires trusted host authority"])
        .with_emits(&["automation.suppressed"]),
    ]
}

fn automation_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "automation.set",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Rule name within the app.", "automation_name"),
                param("ruleJson", "Canonical rule JSON.", "json"),
                param("ruleHash", "Canonical rule hash.", "sha256"),
            ],
            "Records one rule definition.",
        ),
        event_doc(
            "automation.removed",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Rule name within the app.", "automation_name"),
            ],
            "Removes one rule definition.",
        ),
        event_doc(
            "automation.fired",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Rule name within the app.", "automation_name"),
                param("ruleHash", "Canonical rule hash.", "sha256"),
                param("eventRef", "Triggering event reference.", "sha256"),
                param("firedAt", "Observed fire epoch milliseconds.", "epoch_ms"),
            ],
            "Records one host-observed firing fact.",
        ),
        event_doc(
            "automation.suppressed",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Rule name within the app.", "automation_name"),
                param("reason", "Suppression reason.", "token"),
            ],
            "Records one visible skipped firing fact.",
        ),
    ]
}
