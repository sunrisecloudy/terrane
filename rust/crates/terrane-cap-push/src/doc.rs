use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, schema, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, ResourceDoc, ResourceMethodDoc,
};

pub(crate) fn push_doc(_include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "push".to_string(),
        title: "Local Push Notifications".to_string(),
        summary: "Local push subscriptions and per-device notification delivery bookkeeping."
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
                "push.subscribe".to_string(),
                "push.unsubscribe".to_string(),
                "push.record-delivery".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "push.subscribed".to_string(),
                "push.unsubscribed".to_string(),
                "push.delivered".to_string(),
                "push.failed".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods(),
        },
        commands: commands(),
        queries: Vec::new(),
        events: events(),
        resources: resources(),
        schemas: vec![schema(
            terrane_cap_interface::NAMESPACE_SELECTOR_SCHEMA_ID,
            "Namespace selector",
            terrane_cap_interface::NAMESPACE_SELECTOR_SCHEMA_JSON,
        )],
        examples: vec![ExampleDoc {
            title: "Subscribe to KV changes".to_string(),
            summary: "Notify when app KV data changes.".to_string(),
            language: "sh".to_string(),
            code: "terrane push.subscribe notes kv.* 'Notes changed|{kind} {key}'".to_string(),
            expected: "push.subscribed".to_string(),
        }],
        constraints: vec![
            "Push v1 is local push: subscriptions are synced facts, and delivery is a local edge effect on whichever of the user's hosts is running."
                .to_string(),
            "A device whose host is not running gets the notification when its host next starts and catches up, subject to the staleness cutoff; v1 does not wake a sleeping phone or use APNs/FCM/relay infrastructure."
                .to_string(),
            "Delivery outcomes are replica-local bookkeeping and are never sync-allowlisted."
                .to_string(),
        ],
        limits: vec![
            limit(
                "subscriptionsPerApp",
                "32",
                "Keeps matching predictable and avoids notification fan-out surprises.",
            ),
            limit(
                "stalenessCutoff",
                "24h host default",
                "A long-offline host must not flood the user with stale banners.",
            ),
            limit(
                "deliveryHistory",
                "512 per app",
                "Folded state needs bounded dedup history; full facts remain in the log.",
            ),
        ],
        compatibility: vec![
            "push.subscribed and push.unsubscribed are sync v2 allowlisted; push.delivered and push.failed stay local."
                .to_string(),
            "The host edge should later converge push triggers onto automation without changing subscription facts."
                .to_string(),
        ],
        internal: Vec::new(),
    }
}

fn commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "push.subscribe",
            &[
                param("app", "App id.", "string"),
                param(
                    "eventPattern",
                    "Exact kind such as kv.set or namespace wildcard such as kv.*.",
                    "string",
                ),
                param(
                    "template",
                    "Notification template. Split title/body on the first |.",
                    "string",
                ),
                param("subId", "Optional stable subscription id.", "string"),
            ],
            "commit",
            "Record a durable app subscription.",
        )
        .with_errors(&["unknown app", "invalid event pattern", "too many subscriptions"])
        .with_emits(&["push.subscribed"]),
        command_doc(
            "push.unsubscribe",
            &[
                param("app", "App id.", "string"),
                param("subId", "Subscription id.", "string"),
            ],
            "commit",
            "Remove a durable app subscription.",
        )
        .with_errors(&["unknown app", "invalid subscription id"])
        .with_emits(&["push.unsubscribed"]),
        command_doc(
            "push.record-delivery",
            &[
                param("app", "App id.", "string"),
                param("subId", "Subscription id.", "string"),
                param("eventSeq", "Local log sequence of the matched event.", "u64"),
                param("status", "delivered or failed.", "string"),
                param("detail", "Optional failure detail.", "string"),
            ],
            "commit",
            "Record this replica's notification attempt outcome.",
        )
        .with_errors(&["invalid status"])
        .with_emits(&["push.delivered", "push.failed"]),
    ]
}

fn events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "push.subscribed",
            &[
                param("app", "App id.", "string"),
                param("subId", "Subscription id.", "string"),
                param("eventPattern", "Exact kind or namespace wildcard.", "string"),
                param("template", "Notification template.", "string"),
            ],
            "Upsert a synced subscription fact.",
        ),
        event_doc(
            "push.unsubscribed",
            &[
                param("app", "App id.", "string"),
                param("subId", "Subscription id.", "string"),
            ],
            "Remove a synced subscription fact.",
        ),
        event_doc(
            "push.delivered",
            &[
                param("app", "App id.", "string"),
                param("subId", "Subscription id.", "string"),
                param("eventSeq", "Matched local event sequence.", "u64"),
            ],
            "Record this replica's successful delivery.",
        ),
        event_doc(
            "push.failed",
            &[
                param("app", "App id.", "string"),
                param("subId", "Subscription id.", "string"),
                param("eventSeq", "Matched local event sequence.", "u64"),
                param("detail", "Failure detail.", "string"),
            ],
            "Record this replica's failed delivery.",
        ),
    ]
}

fn resources() -> Vec<ResourceDoc> {
    vec![ResourceDoc {
        namespace: "push".to_string(),
        summary: "App-scoped local push subscription methods.".to_string(),
        methods: resource_methods(),
    }]
}

fn resource_methods() -> Vec<ResourceMethodDoc> {
    vec![
        method_returns(resource_method(
            "subscribe",
            "call",
            &[
                param("pattern", "Exact kind or namespace wildcard.", "string"),
                param("template", "Notification template.", "string"),
            ],
            "Subscribe this app to matching data changes.",
        ), "subId string"),
        method_returns(resource_method(
            "unsubscribe",
            "call",
            &[param("subId", "Subscription id.", "string")],
            "Remove a subscription.",
        ), "JSON subscription list"),
        method_returns(resource_method(
            "list",
            "read",
            &[],
            "Return this app's subscriptions as JSON.",
        ), "JSON subscription list"),
    ]
}

fn method_returns(mut method: ResourceMethodDoc, returns: &str) -> ResourceMethodDoc {
    method.returns = returns.to_string();
    method
}
