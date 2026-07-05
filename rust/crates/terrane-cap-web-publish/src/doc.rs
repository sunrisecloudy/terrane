use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    ExampleDoc, InternalNote,
};

pub fn web_publish_doc(include_internal: bool) -> CapabilityDoc {
    let mut doc = CapabilityDoc {
        namespace: "web-publish".to_string(),
        title: "Web Publish".to_string(),
        summary: "Replayable public URL intent for Premium relay serving.".to_string(),
        status: "experimental".to_string(),
        version: "v1".to_string(),
        audience: vec!["app-author".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "web-publish.enable".to_string(),
                "web-publish.disable".to_string(),
                "web-publish.domain.set".to_string(),
            ],
            queries: vec!["web-publish.status".to_string()],
            events: vec![
                "web-publish.enabled".to_string(),
                "web-publish.disabled".to_string(),
                "web-publish.domain.set".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: vec![
            command_doc(
                "web-publish.enable",
                &[
                    param("app", "App id.", "appId"),
                    param("mode", "static or interactive; defaults to static.", "publishMode"),
                    param("slug", "Relay-allocated or operator-supplied slug.", "dnsLabel"),
                ],
                "web-publish.enabled",
                "Record that an app should be available through the Premium relay.",
            )
            .with_errors(&["unknown app", "invalid mode", "invalid slug"])
            .with_emits(&["web-publish.enabled"]),
            command_doc(
                "web-publish.disable",
                &[param("app", "App id.", "appId")],
                "web-publish.disabled",
                "Record the kill-switch fact for an app's public route.",
            )
            .with_errors(&["invalid app"])
            .with_emits(&["web-publish.disabled"]),
            command_doc(
                "web-publish.domain.set",
                &[
                    param("app", "Published app id.", "appId"),
                    param("domain", "Custom domain handled by the relay.", "fqdn"),
                ],
                "web-publish.domain.set",
                "Record a custom domain for an already published app.",
            )
            .with_errors(&["unpublished app", "invalid domain"])
            .with_emits(&["web-publish.domain.set"]),
        ],
        queries: vec![query_doc(
            "web-publish.status",
            &[param("app", "Optional app id.", "appId")],
            "JSON folded publish status. Live tunnel health is read at the host edge.",
            "Read recorded publish intent/state.",
        )
        .with_errors(&["invalid app id", "state unavailable"])],
        events: vec![
            event_doc(
                "web-publish.enabled",
                &[
                    param("app", "App id.", "appId"),
                    param("mode", "static or interactive.", "publishMode"),
                    param("slug", "Relay slug.", "dnsLabel"),
                ],
                "An app has a desired public relay route.",
            ),
            event_doc(
                "web-publish.disabled",
                &[param("app", "App id.", "appId")],
                "An app's public relay route should be disabled.",
            ),
            event_doc(
                "web-publish.domain.set",
                &[
                    param("app", "App id.", "appId"),
                    param("domain", "Custom domain.", "fqdn"),
                ],
                "A published app has a custom domain.",
            ),
        ],
        resources: Vec::new(),
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Publish a static public URL".to_string(),
            summary: "The local home records intent only; the host dials out to the Premium relay with edge-held credentials.".to_string(),
            language: "sh".to_string(),
            code: "terrane web-publish enable notes static notes-r4k9p".to_string(),
            expected: "The log records web-publish.enabled; relay credentials and visitor traffic stay out of replay state.".to_string(),
        }],
        constraints: vec![
            "The home host only dials out to the relay; it never listens for public inbound traffic.".to_string(),
            "Relay credentials live in the host keychain/connection layer and never in events.".to_string(),
            "Visitor traffic and request logs are transient relay/host edge data, not replay facts.".to_string(),
            "Interactive visitors may invoke only manifest.publicVerbs as the anonymous principal.".to_string(),
        ],
        limits: vec![
            limit("publicVerbs", "16", "Interactive mode allowlist stays auditable."),
            limit("interactiveBodyBytes", "1048576", "Relay mirrors net-v2 body caps."),
            limit("slugsPerApp", "1", "One relay slug is folded per app in v1."),
        ],
        compatibility: vec![
            "Free/local users keep existing LAN serving; public relay serving is Premium-gated.".to_string(),
            "Offline home behavior is a relay offline page in v1; no static snapshot rests on the relay.".to_string(),
        ],
        internal: vec![InternalNote {
            title: "Edge effects".to_string(),
            body: "Slug allocation, tunnel reconnect/backoff, ACME, rate limits, and abuse controls belong to terrane-host/terrane-premium edge code.".to_string(),
        }],
    };
    if !include_internal {
        doc = doc.without_internal();
    }
    doc
}
