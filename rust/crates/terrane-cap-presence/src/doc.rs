use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    Capability, CommandDoc, EventDoc, ExampleDoc, LimitDoc, ParamDoc, ResourceDoc,
    ResourceMethodDoc,
};

use crate::{
    DEFAULT_MAX_PAYLOAD_BYTES, DEFAULT_MAX_RATE_PER_SEC, MAX_CHANNEL_NAME_CHARS,
    MAX_CHANNELS_PER_APP, MAX_PAYLOAD_BYTES,
};

const STR: &str = "string";

pub fn presence_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    let mut doc = CapabilityDoc {
        namespace: "presence".to_string(),
        title: "Presence".to_string(),
        summary: "Ephemeral realtime channel definitions and live pub/sub. Only channel limits are durable; messages are transient sync-v2 frames and never enter the event log.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["app-author".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "presence.channel.define".to_string(),
                "presence.channel.drop".to_string(),
                "presence.publish".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "presence.channel.defined".to_string(),
                "presence.channel.dropped".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: commands(),
        queries: Vec::new(),
        events: events(),
        resources: vec![ResourceDoc {
            namespace: "presence".to_string(),
            summary: "Backend resource surface installed as ctx.resource.presence. publish() fans out live only; peers() reads live connected peers.".to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Publish a cursor".to_string(),
            summary: "An app sends optional cursor positions without creating replay state.".to_string(),
            language: "js".to_string(),
            code: "function handle(input) {\n  ctx.resource.presence.publish(\"cursor\", {x: 12, y: 8});\n  return \"sent\";\n}".to_string(),
            expected: "The publish returns ok and records no events; offline peers miss it.".to_string(),
        }],
        constraints: vec![
            "Presence messages are best-effort, at-most-once, and unordered across peers.".to_string(),
            "Presence messages have no history, acknowledgements, retries, queueing, folding, or replay.".to_string(),
            "Replay sees only presence.channel.defined and presence.channel.dropped records.".to_string(),
            "If a signal needs durability, write it through kv or crdt instead.".to_string(),
            "Fan-out is limited to peers that hold a share grant for the app; unshared peers receive nothing.".to_string(),
        ],
        limits: limits(),
        compatibility: vec![
            "Presence rides the sync v2 edge transport; pure replay and import/export ignore live frames.".to_string(),
            "ctx.resource.presence.publish accepts any JSON payload that fits the channel limit.".to_string(),
        ],
        internal: vec![terrane_cap_interface::InternalNote {
            title: "Transient effect".to_string(),
            body: "presence.publish returns Decision::TransientEffect(Effect::PresencePublish). The edge runner returns no event records.".to_string(),
        }],
    };
    if !include_internal {
        doc.internal.clear();
    }
    doc
}

fn commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "presence.channel.define",
            &[
                param("app", "Owning app id.", STR),
                param("channel", "Live channel name.", STR),
                param("max_payload", "Optional byte cap; defaults to 16 KiB.", "u32"),
                param("max_rate", "Optional per-publisher msgs/sec cap; defaults to 20.", "u32"),
            ],
            "presence.channel.defined",
            "Define or update durable limits for a live presence channel.",
        )
        .with_errors(&["app not found", "invalid channel", "limit out of range"])
        .with_emits(&["presence.channel.defined"]),
        command_doc(
            "presence.channel.drop",
            &[
                param("app", "Owning app id.", STR),
                param("channel", "Live channel name.", STR),
            ],
            "presence.channel.dropped",
            "Drop a durable channel definition.",
        )
        .with_errors(&["app not found", "invalid channel"])
        .with_emits(&["presence.channel.dropped"]),
        command_doc(
            "presence.publish",
            &[
                param("app", "Owning app id.", STR),
                param("channel", "Live channel name.", STR),
                param("payload", "JSON payload to fan out live.", "json"),
            ],
            "transient effect; no event",
            "Publish one live message. Auto-defined channels use default limits; the message is never recorded.",
        )
        .with_errors(&["app not found", "invalid channel", "invalid JSON", "payload too large", "rate limited"])
        .with_effects(&["PresencePublish"]),
    ]
}

fn events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "presence.channel.defined",
            &[
                param("app", "Owning app id.", STR),
                param("channel", "Live channel name.", STR),
                param("max_payload", "Maximum payload bytes.", "u32"),
                param("max_rate", "Maximum messages/sec per publisher.", "u32"),
            ],
            "Upsert durable channel limits.",
        ),
        event_doc(
            "presence.channel.dropped",
            &[
                param("app", "Owning app id.", STR),
                param("channel", "Live channel name.", STR),
            ],
            "Remove a durable channel definition.",
        ),
    ]
}

fn limits() -> Vec<LimitDoc> {
    vec![
        limit("default-payload-bytes", &DEFAULT_MAX_PAYLOAD_BYTES.to_string(), "Keep realtime frames bounded."),
        limit("max-payload-bytes", &MAX_PAYLOAD_BYTES.to_string(), "A channel may define lower limits but never higher."),
        limit("default-rate", &format!("{DEFAULT_MAX_RATE_PER_SEC} msgs/sec"), "Drop newest frames rather than queueing."),
        limit("channels-per-app", &MAX_CHANNELS_PER_APP.to_string(), "Bound folded channel metadata."),
        limit("channel-name-chars", &MAX_CHANNEL_NAME_CHARS.to_string(), "Keep selectors promptable and transport-friendly."),
    ]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    use terrane_cap_interface::ResourceMethod;
    crate::PresenceCapability
        .manifest()
        .resources
        .into_iter()
        .map(|method| {
            let mut doc = match method {
                ResourceMethod::Call { name, params } => {
                    resource_method(name, "call", &expand(params), "Publish one transient JSON message.")
                }
                ResourceMethod::Read { name, params } => {
                    resource_method(name, "read", &expand(params), "Read connected peers seen on a channel.")
                }
                ResourceMethod::Write { name, params } => {
                    resource_method(name, "write", &expand(params), "Write method.")
                }
            };
            doc.returns = match (doc.kind.as_str(), doc.name.as_str()) {
                ("call", "publish") => "string — ok",
                ("read", "peers") => "string[] — connected peer ids",
                _ => "string",
            }
            .to_string();
            doc
        })
        .collect()
}

fn expand(params: &'static [&'static str]) -> Vec<ParamDoc> {
    params
        .iter()
        .map(|name| param(name, "Positional argument.", STR))
        .collect()
}
