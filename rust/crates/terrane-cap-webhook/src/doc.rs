use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

fn resource_methods() -> Vec<terrane_cap_interface::ResourceMethodDoc> {
    let mut list = resource_method(
        "list",
        "read",
        &[],
        "List this app's registered webhook names, backend verbs, and URL paths.",
    );
    list.returns = "JSON array of {name, verb, url_path}".to_string();
    vec![list]
}

pub fn webhook_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "webhook".to_string(),
        title: "Inbound Webhooks".to_string(),
        summary: "Inbound HTTP deliveries recorded as replay-stable facts.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "webhook.register".to_string(),
                "webhook.rotate".to_string(),
                "webhook.unregister".to_string(),
                "webhook.ingest".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "webhook.registered".to_string(),
                "webhook.rotated".to_string(),
                "webhook.unregistered".to_string(),
                "webhook.received".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods(),
        },
        commands: commands(),
        queries: Vec::new(),
        events: events(),
        resources: vec![ResourceDoc {
            namespace: "webhook".to_string(),
            summary: "Read registered inbound HTTP routes for this app.".to_string(),
            methods: resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Register a local-network hook".to_string(),
            summary: "A listening host mints the token path and records webhook.registered."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane webhook register demo github receiveGithub".to_string(),
            expected: "prints a /hook/demo/github/<token> path".to_string(),
        }],
        constraints: vec![
            "The listener records webhook.received once at the edge; replay folds the event and never re-listens."
                .to_string(),
            "Tokens are minted by the host edge and recorded so replay rebuilds the route table."
                .to_string(),
            "Sensitive header values are redacted before EventRecord construction; signature/MAC headers are recorded for app-side verification."
                .to_string(),
            "webhook.ingest is trusted-host-only and should not be app or public CLI callable."
                .to_string(),
            "Apps receive delivery JSON through the registered backend verb after the event is committed."
                .to_string(),
        ],
        limits: vec![
            limit("hooks", "32 per app", "Keeps the folded route table bounded."),
            limit("name", "128 chars [a-z0-9-_]", "Names become stable URL path segments."),
            limit("headers", "32 KiB", "Headers are bounded before persistence."),
            limit("inline body", "256 KiB", "Larger or binary payloads are represented as blob-linked bodies."),
            limit("body", "32 MiB", "Oversized deliveries are refused before recording."),
            limit("rate", "60 deliveries/minute/hook", "Hosts refuse excess deliveries without recording an event."),
        ],
        compatibility: vec![
            "CLI registration is host-independent, but deliveries require a listening host."
                .to_string(),
            "App-controlled webhook HTTP responses are intentionally absent in v1; the sender receives 202 after commit."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "webhook.register returns Effect::WebhookRegister; the runner mints a token and emits webhook.registered or webhook.rotated. webhook.ingest commits only shaped, redacted facts."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "webhook.register",
            &[
                param("app", "Existing app id that owns the hook.", "app_id"),
                param("name", "Stable route name.", "string"),
                param("verb", "Backend verb invoked after commit.", "string"),
            ],
            "effect",
            "Validate a route request and ask the edge to mint its token.",
        )
        .with_errors(&["app not found", "invalid name", "too many hooks"])
        .with_effects(&["WebhookRegister"])
        .with_emits(&["webhook.registered"]),
        command_doc(
            "webhook.rotate",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Existing route name.", "string"),
            ],
            "effect",
            "Ask the edge to mint a replacement token for an existing route.",
        )
        .with_errors(&["app not found", "unknown route"])
        .with_effects(&["WebhookRegister"])
        .with_emits(&["webhook.rotated"]),
        command_doc(
            "webhook.unregister",
            &[
                param("app", "Existing app id.", "app_id"),
                param("name", "Existing route name.", "string"),
            ],
            "commit",
            "Remove a route from folded state.",
        )
        .with_errors(&["app not found", "invalid name"])
        .with_emits(&["webhook.unregistered"]),
        command_doc(
            "webhook.ingest",
            &[param(
                "delivery_json",
                "Host-only delivery envelope with app, name, token, method, headers, body/body_base64, body_mime, received_at.",
                "json",
            )],
            "commit",
            "Trusted listener records one inbound HTTP delivery as a redacted fact.",
        )
        .with_errors(&["unknown route", "bad token", "oversized headers", "oversized body"])
        .with_emits(&["webhook.received"]),
    ]
}

fn events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "webhook.registered",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Route name.", "string"),
                param("verb", "Backend verb.", "string"),
                param("token", "Unguessable URL token.", "hex"),
            ],
            "Records a minted webhook route.",
        ),
        event_doc(
            "webhook.rotated",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Route name.", "string"),
                param("verb", "Backend verb.", "string"),
                param("token", "Replacement URL token.", "hex"),
            ],
            "Records a replacement token for a route.",
        ),
        event_doc(
            "webhook.unregistered",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Route name.", "string"),
            ],
            "Drops a webhook route from folded state.",
        ),
        event_doc(
            "webhook.received",
            &[
                param("app", "Owning app id.", "app_id"),
                param("name", "Route name.", "string"),
                param("method", "HTTP method.", "string"),
                param("headers", "Redacted lower-case headers.", "json"),
                param("body_kind", "inline or blob.", "string"),
                param("body", "Inline text/base64 or blob link name.", "string"),
                param("body_hash", "SHA-256 of original bytes.", "sha256"),
                param("body_size", "Original body size.", "u64"),
                param("received_at", "Edge wall-clock epoch milliseconds.", "u64"),
            ],
            "Records one inbound delivery.",
        ),
    ]
}
