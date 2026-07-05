//! The `tts` capability — text-to-speech at the host edge.
//!
//! Playback is transient (`tts.speak`) and never enters the event log. Rendering
//! (`tts.render`) is recorded as a blob-backed artifact fact: the edge
//! synthesizes bytes, stores them in the blob CAS, and records `tts.rendered`.
//! Replay folds metadata only and never re-runs a synthesizer.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, QueryCtx, QueryValue, ReadValue, ResourceReadCtx, Result,
    StateStore,
};

mod commands;
mod doc;
mod events;
mod resources;
mod types;

pub use commands::sha256_hex;
pub use events::rendered_event;
pub use types::{
    RenderRecord, TtsState, DEFAULT_RATE_MILLI, MAX_RATE_MILLI, MAX_RENDERS_PER_APP,
    MAX_TEXT_BYTES, MIN_RATE_MILLI, TTS_MIME_WAV,
};

pub struct TtsCapability;

impl Capability for TtsCapability {
    fn namespace(&self) -> &'static str {
        "tts"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "tts.speak" },
                CommandSpec { name: "tts.render" },
            ],
            events: vec![EventSpec {
                kind: "tts.rendered",
            }],
            queries: vec![terrane_cap_interface::QuerySpec {
                name: "tts.supports",
            }],
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "tts",
                &["call", "read"],
                "Speak text aloud and render speech audio.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::tts_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "tts.speak" => commands::decide_speak(ctx, args),
            "tts.render" => commands::decide_render(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }

    fn query(&self, _ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "supports" | "tts.supports" => {
                let verb = args.first().map(String::as_str).unwrap_or_default();
                Ok(QueryValue::Bool(supports(verb)))
            }
            other => Err(Error::InvalidInput(format!("unknown query: {other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        resources::read(ctx, name, args)
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "speak" => Ok(ReadValue::OptString(Some("ok".to_string()))),
            "render" => {
                let render = events::render_from_records(records)?
                    .ok_or_else(|| Error::Runtime("tts.render produced no render event".into()))?;
                let encoded = serde_json::to_string(&serde_json::json!({
                    "textHash": render.text_hash,
                    "voice": render.voice,
                    "rateMilli": render.rate_milli,
                    "blobHash": render.blob_hash,
                    "size": render.size,
                    "mime": render.mime,
                    "durationMs": render.duration_ms,
                }))
                .map_err(|e| Error::InvalidInput(format!("tts render encode failed: {e}")))?;
                Ok(ReadValue::OptString(Some(encoded)))
            }
            other => Err(Error::InvalidInput(format!(
                "tts.{other} is not a callable resource"
            ))),
        }
    }
}

pub fn supports(verb: &str) -> bool {
    cfg!(target_os = "macos") && matches!(verb, "speak" | "voices" | "render")
}
