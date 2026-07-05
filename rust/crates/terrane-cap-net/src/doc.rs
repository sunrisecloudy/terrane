use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn net_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut get = resource_method(
        "get",
        "call",
        &[param("url", "Absolute URL to fetch with a live HTTP GET.", "url")],
        "Live HTTP GET for a transient query; returns the response body and records nothing.",
    );
    get.returns = "the response body as a string".to_string();
    let mut call = resource_method(
        "call",
        "call",
        &[param(
            "request_json",
            "Canonical net request JSON with method, url, headers, body, timeout, redirect, and responseBody controls.",
            "json",
        )],
        "Live full HTTP request for a transient query; returns inline response bodies and records nothing.",
    );
    call.returns = "the inline response body as a string; blob responses require the blob resource".to_string();
    vec![get, call]
}

pub fn net_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "net".to_string(),
        title: "Recorded HTTP".to_string(),
        summary:
            "Recorded full HTTP effects for apps that need replay-stable network reads.".to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["net.fetch".to_string(), "net.request".to_string()],
            queries: Vec::new(),
            events: vec!["net.fetched".to_string(), "net.responded".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: net_resource_methods(),
        },
        commands: net_commands(),
        queries: Vec::new(),
        events: net_events(),
        resources: vec![ResourceDoc {
            namespace: "net".to_string(),
            summary:
                "Live, unrecorded HTTP calls for transient queries (e.g. a breach check).".to_string(),
            methods: net_resource_methods(),
        }],
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
            "net.request validates and canonicalizes method, URL, headers, body, timeout, redirect, and responseBody before returning Effect::HttpRequest."
                .to_string(),
            "The HTTP request is performed only by the edge effect runner, never by replay.".to_string(),
            "A completed GET is recorded as net.fetched with app id, URL, status, and body."
                .to_string(),
            "A completed full HTTP request is recorded as net.responded with the redacted canonical request, status, filtered response headers, and either inline body bytes or blob metadata."
                .to_string(),
            "Sensitive request header values are redacted before EventRecord construction: authorization, proxy-authorization, cookie, set-cookie, x-api-key, api-key, *-token, *-secret, plus app-declared sensitiveHeaders."
                .to_string(),
            "{\"$secret\":\"name\"} is a reserved request value shape. net validates and round-trips it, but edge execution rejects unresolved secrets until cap-oauth-connections provides substitution."
                .to_string(),
            "SSRF guard: only http and https URLs are allowed, and 169.254.169.254 is denied after resolution. Private and loopback ranges stay allowed because Terrane is local-first and localhost APIs are a feature."
                .to_string(),
            "Replay folds recorded net.fetched events into per-app response state keyed by URL."
                .to_string(),
            "Replay folds recorded net.responded events into per-app response state keyed by the canonical request SHA-256."
                .to_string(),
            "Folding app.removed removes all recorded HTTP responses for that app.".to_string(),
        ],
        limits: vec![
            limit("method", "GET|POST|PUT|PATCH|DELETE|HEAD", "net.request supports common HTTP methods; net.fetch remains GET-only for compatibility."),
            limit("responseKey", "app+url", "Later responses for the same app and URL replace the folded value."),
            limit("requestKey", "sha256(canonical request JSON)", "Later net.request responses for the same app and canonical request replace the folded value."),
            limit("timeoutMs", "1..120000", "Default timeout is 30000 ms."),
            limit("redirect", "follow<=5|manual|deny", "follow caps at 5 redirects and refuses HTTPS-to-HTTP downgrades; manual records the 3xx; deny errors on 3xx."),
            limit("inline auto body", "256 KiB", "auto inlines text responses up to 256 KiB and offloads larger or binary responses to blob."),
            limit("inline forced body", "8 MiB", "responseBody=inline errors above 8 MiB."),
            limit("response hard cap", "32 MiB", "Responses above 32 MiB are rejected before recording."),
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
                body: "Effect::HttpGet and Effect::HttpRequest are transient. net.fetched and net.responded are the durable replay inputs and store observed responses with request secrets redacted before persistence."
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
    .with_emits(&["net.fetched"]),
    command_doc(
        "net.request",
        &[
            param(
                "app",
                "Existing app id that owns the recorded response.",
                "app_id",
            ),
            param(
                "request_json",
                "Request JSON: method, url, headers, body, sensitiveHeaders, timeoutMs, redirect, responseBody.",
                "json",
            ),
        ],
        "effect",
        "Validate, canonicalize, redact, and request one app-scoped full HTTP effect.",
    )
    .with_errors(&[
        "app not found",
        "invalid request JSON",
        "unsupported method",
        "invalid timeout",
        "unresolved $secret at edge",
        "response too large",
    ])
    .with_effects(&["HttpRequest"])
    .with_emits(&["net.responded", "blob.stored when body offloads"])]
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
    .with_effects(&["stores NetState.fetches[app][url]"]),
    event_doc(
        "net.responded",
        &[
            param("app", "App id that requested the HTTP call.", "app_id"),
            param("request_key", "SHA-256 of canonical request JSON.", "sha256"),
            param(
                "request_json_redacted",
                "Canonical request JSON with sensitive header values replaced before persistence.",
                "json",
            ),
            param("status", "HTTP response status.", "u16"),
            param(
                "response_headers",
                "Filtered response headers: content-type, content-length, etag, last-modified, location, cache-control.",
                "map",
            ),
            param("body_kind", "inline or blob.", "string"),
            param("body", "Inline response body or empty for blob.", "string"),
            param("body_is_base64", "True when inline body is base64-encoded binary.", "bool"),
            param("body_hash", "SHA-256 of response bytes.", "sha256"),
            param("body_size", "Response byte length.", "u64"),
            param("body_mime", "Response MIME type.", "mime"),
        ],
        "Records the observed full HTTP response for replay.",
    )
    .with_effects(&["stores NetState.requests[app][request_key]"])]
}
