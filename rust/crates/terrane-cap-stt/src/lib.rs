//! The `stt` capability — ambient speech-to-text transcripts, recorded.
//!
//! Microphone capture, VAD, and ASR inference run entirely at the host edge;
//! only finalized transcript text (plus integer timings) crosses into the core
//! as ordinary events. Replay folds segments without ever re-running inference
//! (Option A, the same replay discipline `local-model` uses for generations).
//! Raw audio is never an event. Reacts to `app.removed` by dropping that app's
//! sessions: a revoked app must not inherit recorded transcripts.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, ReadValue, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod doc;
mod events;
mod resources;
mod types;

pub use events::{
    retention_trimmed_event, segment_appended_event, selection_made_event,
    session_closed_event, session_opened_event, session_purged_event, SegmentAppendedRecord,
    SelectionMadeRecord, SessionOpenedRecord,
};
pub use types::{
    SttSegment, SttSelection, SttSession, SttState, SttStatus, DEFAULT_SAMPLE_RATE_HZ,
};

pub struct SttCapability;

impl Capability for SttCapability {
    fn namespace(&self) -> &'static str {
        "stt"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "stt.session.open",
                },
                CommandSpec {
                    name: "stt.segment.append",
                },
                CommandSpec {
                    name: "stt.session.close-host",
                },
                CommandSpec {
                    name: "stt.retention.trim",
                },
                CommandSpec {
                    name: "stt.session.purge",
                },
                CommandSpec {
                    name: "stt.select",
                },
                CommandSpec {
                    name: "stt.stop",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "stt.session.opened",
                },
                EventSpec {
                    kind: "stt.segment.appended",
                },
                EventSpec {
                    kind: "stt.session.closed",
                },
                EventSpec {
                    kind: "stt.selection.made",
                },
                EventSpec {
                    kind: "stt.retention.trimmed",
                },
                EventSpec {
                    kind: "stt.session.purged",
                },
            ],
            queries: Vec::new(),
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "stt",
                &["call", "read"],
                "Ambient speech-to-text sessions, transcript, and selections. Apps read the \
                 transcript and record selections; capture is host-owned.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::stt_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "stt.session.open" => commands::decide_session_open(ctx, args),
            "stt.segment.append" => commands::decide_segment_append(ctx, args),
            "stt.session.close-host" => commands::decide_session_close_host(ctx, args),
            "stt.retention.trim" => commands::decide_retention_trim(ctx, args),
            "stt.session.purge" => commands::decide_session_purge(ctx, args),
            "stt.select" => commands::decide_select(ctx, args),
            "stt.stop" => commands::decide_session_close(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
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
            "select" => Ok(ReadValue::OptString(events::selection_text_from_records(records))),
            "stop" => Ok(ReadValue::OptString(Some("ok".to_string()))),
            other => Err(Error::InvalidInput(format!(
                "stt.{other} is not a callable resource"
            ))),
        }
    }
}
