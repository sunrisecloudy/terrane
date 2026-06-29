use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, CommandDoc,
    EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

pub fn net_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "net".to_string(),
        title: "Recorded HTTP".to_string(),
        summary: "Recorded HTTP GET effects for apps that need replay-stable network reads."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["net.fetch".to_string()],
            queries: Vec::new(),
            events: vec!["net.fetched".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: net_commands(),
        queries: Vec::new(),
        events: net_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Fetch and record a URL".to_string(),
            summary: "Ask the edge runner to perform a GET and record the response for deterministic replay."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane net fetch demo https://example.test/data".to_string(),
            expected: "returns Effect::HttpGet; the runner records net.fetched with status and body"
                .to_string(),
        }],
        constraints: vec![
            "net.fetch validates that the app exists and the URL is non-empty before returning Effect::HttpGet."
                .to_string(),
            "The HTTP request is performed only by the edge effect runner, never by replay.".to_string(),
            "A completed GET is recorded as net.fetched with app id, URL, status, and body."
                .to_string(),
            "Replay folds recorded net.fetched events into per-app response state keyed by URL."
                .to_string(),
            "Folding app.removed removes all recorded HTTP responses for that app.".to_string(),
        ],
        limits: vec![
            limit("method", "GET", "The initial network effect surface only records HTTP GET."),
            limit("responseKey", "app+url", "Later responses for the same app and URL replace the folded value."),
        ],
        compatibility: vec![
            "Network availability is outside replay; deterministic behavior depends on recording net.fetched once at the edge."
                .to_string(),
            "App removal cleanup is driven by the app.removed subscription and does not require a net-specific command."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::HttpGet is transient. net.fetched is the durable replay input and stores the observed response."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn net_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "net.fetch",
        &[
            param(
                "app",
                "Existing app id that owns the recorded response.",
                "app_id",
            ),
            param("url", "Absolute URL to fetch with HTTP GET.", "url"),
        ],
        "effect",
        "Validate one app-scoped HTTP GET request and return the edge effect.",
    )
    .with_errors(&["app not found", "empty url"])
    .with_effects(&["HttpGet"])
    .with_emits(&["net.fetched"])]
}

fn net_events() -> Vec<EventDoc> {
    vec![event_doc(
        "net.fetched",
        &[
            param("app", "App id that requested the fetch.", "app_id"),
            param("url", "Fetched URL.", "url"),
            param("status", "HTTP response status.", "u16"),
            param("body", "Recorded response body.", "string"),
        ],
        "Records the observed HTTP GET response for replay.",
    )
    .with_effects(&["stores NetState.fetches[app][url]"])]
}
