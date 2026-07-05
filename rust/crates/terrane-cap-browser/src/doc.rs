use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn browser_resource_methods() -> Vec<ResourceMethodDoc> {
    let mut render = resource_method(
        "render",
        "call",
        &[param(
            "request_json",
            "Render request JSON with url, output, waitMs, viewport, allowedHosts, and sensitiveHeaders.",
            "json",
        )],
        "Recorded headless browser render. Replay folds browser.rendered and never opens a page.",
    );
    render.returns = "inline text/html body; blob outputs require the blob resource".to_string();
    let mut peek = resource_method(
        "peek",
        "call",
        &[param(
            "request_json",
            "Render request JSON; same validation as render.",
            "json",
        )],
        "Live unrecorded render for transient inspection.",
    );
    peek.returns = "inline text/html body; blob outputs require the blob resource".to_string();
    vec![render, peek]
}

pub fn browser_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "browser".to_string(),
        title: "Recorded Browser Rendering".to_string(),
        summary: "Headless page rendering as a replay-stable recorded effect.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["browser.render".to_string()],
            queries: Vec::new(),
            events: vec!["browser.rendered".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: browser_resource_methods(),
        },
        commands: browser_commands(),
        queries: Vec::new(),
        events: browser_events(),
        resources: vec![ResourceDoc {
            namespace: "browser".to_string(),
            summary: "Recorded or transient hidden browser renders.".to_string(),
            methods: browser_resource_methods(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Summarize rendered page text".to_string(),
            summary: "Ask a hidden browser for post-JavaScript body text, then summarize that text locally."
                .to_string(),
            language: "javascript".to_string(),
            code: r#"const text = ctx.resource.browser.render(JSON.stringify({url:"https://example.test",output:"text"}));"#.to_string(),
            expected: "browser.rendered is recorded once; replay uses the recorded text.".to_string(),
        }],
        constraints: vec![
            "browser.render validates and canonicalizes the request before returning Effect::BrowserRender.".to_string(),
            "The page is rendered only by a host edge runner, never by replay.".to_string(),
            "A completed render records browser.rendered with redacted request JSON, canonical request key, status, output kind, size, mime, title, and inline body or blob hash.".to_string(),
            "text/html outputs inline up to 256 KiB and offload larger captures to blob; screenshot/pdf are always blob refs.".to_string(),
            "URL policy matches net-v2: only http and https are allowed, cloud metadata 169.254.169.254 is denied after resolution, and localhost remains allowed.".to_string(),
            "Ephemeral profiles are required at the edge; rendered pages do not share persistent browser cookies/storage.".to_string(),
            "Folding app.removed removes all recorded browser renders for that app.".to_string(),
        ],
        limits: vec![
            limit("requestKey", "sha256(canonical request JSON)", "Later renders for the same app and canonical request replace the folded value."),
            limit("waitMs", "0..15000", "Additional settle delay after load."),
            limit("total render", "30000 ms", "The host edge runner kills renders that exceed the cap."),
            limit("viewport", "1x1..3840x2160", "Default viewport is 1280x800."),
            limit("inline auto body", "256 KiB", "text/html auto inline only below this size."),
            limit("body hard cap", "32 MiB", "Larger captures are rejected before recording."),
            limit("recorded resource calls", "30 per backend run", "ctx.resource.browser.render is capped; browser.peek is the unrecorded escape hatch."),
        ],
        compatibility: vec![
            "WKWebView is the preferred macOS engine; CLI/web hosts use a system Chrome/Chromium fallback when present.".to_string(),
            "App removal cleanup is driven by the app.removed subscription and does not require a browser-specific cleanup command.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::BrowserRender is transient. browser.rendered and optional blob.stored metadata are the durable replay inputs.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn browser_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "browser.render",
        &[
            param("app", "Existing app id that owns the recorded render.", "app_id"),
            param("request_json", "Render request JSON.", "json"),
        ],
        "effect",
        "Validate one app-scoped hidden browser render and return the edge effect.",
    )
    .with_errors(&[
        "app not found",
        "invalid request JSON",
        "invalid URL scheme",
        "cloud metadata URL denied",
        "browser engine unavailable",
        "capture too large",
    ])
    .with_effects(&["BrowserRender"])
    .with_emits(&["browser.rendered", "blob.stored when capture offloads"])]
}

fn browser_events() -> Vec<EventDoc> {
    vec![event_doc(
        "browser.rendered",
        &[
            param("app", "App id that requested the render.", "app_id"),
            param("request_key", "SHA-256 of canonical request JSON.", "sha256"),
            param("request_json_redacted", "Canonical request JSON with URL query redacted.", "json"),
            param("url", "Rendered URL.", "url"),
            param("output", "text, html, screenshot, or pdf.", "string"),
            param("status", "Observed render status code, or 0 when the engine did not expose one.", "u16"),
            param("body_kind", "inline or blob.", "string"),
            param("body", "Inline text/html body or empty for blob.", "string"),
            param("body_hash", "SHA-256 of captured bytes.", "sha256"),
            param("size", "Captured byte length.", "u64"),
            param("mime", "Capture MIME type.", "mime"),
            param("title", "Document title when available.", "string"),
        ],
        "Records the observed browser render for replay.",
    )
    .with_effects(&["stores BrowserState.renders[app][request_key]"])]
}
