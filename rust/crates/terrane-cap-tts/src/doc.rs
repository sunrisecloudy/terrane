use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, LimitDoc, ParamDoc, QueryDoc,
    ResourceDoc, ResourceMethodDoc,
};

use crate::resources;
use crate::types::{
    DEFAULT_RATE_MILLI, MAX_RATE_MILLI, MAX_RENDERS_PER_APP, MAX_TEXT_BYTES, MIN_RATE_MILLI,
};

const STR: &str = "string";

pub fn tts_doc(_include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "tts".to_string(),
        title: "Text To Speech".to_string(),
        summary: "Text-to-speech at the edge: playback is transient and never recorded; rendering \
                  records only artifact metadata plus a blob reference so replay folds the fact \
                  without re-synthesizing."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["app-author".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec!["tts.speak".to_string(), "tts.render".to_string()],
            queries: vec!["tts.supports".to_string()],
            events: vec!["tts.rendered".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: tts_commands(),
        queries: tts_queries(),
        events: tts_events(),
        resources: vec![ResourceDoc {
            namespace: "tts".to_string(),
            summary: "Backend resource surface installed as ctx.resource.tts. speak() is \
                      transient; render() records a blob-backed render fact; renders() reads \
                      this app's folded render metadata."
                .to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Speak or render text".to_string(),
            summary: "An app can play a short utterance without changing replay state, or render \
                      speech into the blob CAS when it needs reusable audio bytes."
                .to_string(),
            language: "js".to_string(),
            code: "function handle(input) {\n  if (input[0] === \"render\") { return ctx.resource.tts.render(\"hello\", \"--rate\", \"1000\"); }\n  return ctx.resource.tts.speak(\"hello\");\n}".to_string(),
            expected: "speak returns ok and records nothing; render returns JSON with textHash and blobHash.".to_string(),
        }],
        constraints: vec![
            "tts.speak is a transient edge effect: it never emits an event, and replay never \
             makes sound."
                .to_string(),
            "tts.render records tts.rendered plus blob.stored metadata after the edge writes \
             synthesized bytes to the blob CAS."
                .to_string(),
            "The event stores text_hash, not text. describe() never prints the text."
                .to_string(),
            "Synthesizers are not bit-stable across OS versions; replay folds the recorded \
             artifact reference instead of re-running synthesis."
                .to_string(),
        ],
        limits: tts_limits(),
        compatibility: vec![
            "CLI v1 uses /usr/bin/say on macOS; non-macOS CLI hosts return typed Unsupported errors."
                .to_string(),
            "Web shell can speak through speechSynthesis but cannot render bytes in v1."
                .to_string(),
        ],
        internal: Vec::new(),
    }
}

fn tts_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "tts.speak",
            &[
                param("app", "Owning app id.", STR),
                param("--voice", "Optional host voice token.", STR),
                param("--rate", "Rate in thousandths (500-2000) or multiplier (0.5-2.0).", "u32"),
                param("text", "Text to play aloud.", STR),
            ],
            "transient effect; no event",
            "Speak text aloud now. Live-only; records nothing.",
        )
        .with_errors(&["app not found", "text too large", "invalid voice", "unsupported host"])
        .with_effects(&["TtsSpeak"]),
        command_doc(
            "tts.render",
            &[
                param("app", "Owning app id.", STR),
                param("--voice", "Optional host voice token.", STR),
                param("--rate", "Rate in thousandths (500-2000) or multiplier (0.5-2.0).", "u32"),
                param("text", "Text to synthesize.", STR),
            ],
            "tts.rendered + blob.stored",
            "Render text to audio bytes at the edge, store bytes in the blob CAS, and record the \
             produced artifact reference."
        )
        .with_errors(&["app not found", "text too large", "invalid voice", "unsupported host"])
        .with_effects(&["TtsRender"])
        .with_emits(&["tts.rendered", "blob.stored"]),
    ]
}

fn tts_queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "tts.supports",
        &[param("verb", "speak | render | voices", STR)],
        "bool",
        "Report whether this build/host class supports a TTS verb.",
    )
    .with_errors(&["unknown verb"])]
}

fn tts_events() -> Vec<EventDoc> {
    vec![event_doc(
        "tts.rendered",
        &[
            param("app", "Owning app id.", STR),
            param("text_hash", "SHA-256 of the exact source text.", "sha256_hex"),
            param("voice", "Voice token used, if any.", STR),
            param("rate_milli", "Speaking rate in thousandths.", "u32"),
            param("blob_hash", "SHA-256 of rendered audio bytes.", "sha256_hex"),
            param("size", "Rendered byte length.", "u64"),
            param("mime", "Rendered media type.", STR),
            param("duration_ms", "Duration of the produced artifact.", "u64"),
        ],
        "One completed speech render. Fold upserts by app/text_hash and keeps the last 100 per app.",
    )]
}

fn tts_limits() -> Vec<LimitDoc> {
    vec![
        limit(
            "text-bytes",
            &MAX_TEXT_BYTES.to_string(),
            "Keep edge synthesis bounded and avoid surprise audible playback of huge input.",
        ),
        limit(
            "rate-milli",
            &format!("{MIN_RATE_MILLI}-{MAX_RATE_MILLI} (default {DEFAULT_RATE_MILLI})"),
            "Mirror OS synthesizer useful range with integer replay metadata.",
        ),
        limit(
            "renders-per-app",
            &MAX_RENDERS_PER_APP.to_string(),
            "Bound folded metadata; blob bytes remain governed by blob retention and GC.",
        ),
    ]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    use terrane_cap_interface::ResourceMethod;
    resources::resource_methods()
        .into_iter()
        .map(|method| {
            let mut doc = match method {
                ResourceMethod::Call { name, params } => {
                    resource_method(name, "call", &expand(params), call_summary(name))
                }
                ResourceMethod::Read { name, params } => {
                    resource_method(name, "read", &expand(params), read_summary(name))
                }
                ResourceMethod::Write { name, params } => {
                    resource_method(name, "write", &expand(params), "Write method.")
                }
            };
            doc.returns = method_returns(&doc.kind, &doc.name).to_string();
            doc
        })
        .collect()
}

fn method_returns(kind: &str, name: &str) -> &'static str {
    match (kind, name) {
        ("call", "speak") => "string — ok",
        ("call", "render") => "string — JSON render record with blobHash",
        ("read", "voices") => "string — JSON array of host voices",
        ("read", "renders") => "string — JSON array of this app's render metadata",
        _ => "string",
    }
}

fn expand(params: &'static [&'static str]) -> Vec<ParamDoc> {
    params
        .iter()
        .map(|name| param(name, "Positional argument.", STR))
        .collect()
}

fn call_summary(name: &str) -> &'static str {
    match name {
        "speak" => "Speak text aloud now; transient and unrecorded.",
        "render" => "Render text into the blob CAS and record the artifact reference.",
        _ => "Call method.",
    }
}

fn read_summary(name: &str) -> &'static str {
    match name {
        "voices" => "Host voices as JSON; live and unrecorded.",
        "renders" => "Folded render records for this app as JSON.",
        _ => "Read method.",
    }
}
