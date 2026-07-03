use terrane_cap_interface::{
    command_doc, event_doc, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, ParamDoc, ResourceDoc, ResourceMethodDoc,
};

use crate::resources;
use crate::types::DEFAULT_SAMPLE_RATE_HZ;

const STR: &str = "string";

pub fn stt_doc(_include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "stt".to_string(),
        title: "Speech To Text".to_string(),
        summary: "Ambient speech-to-text: the host edge captures the microphone, runs on-device \
                  ASR, and records only finalized transcript segments as ordinary events. The \
                  core owns the transcript (never the audio); replay folds segments without ever \
                  re-running inference."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["app-author".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "stt.session.open".to_string(),
                "stt.segment.append".to_string(),
                "stt.session.close-host".to_string(),
                "stt.retention.trim".to_string(),
                "stt.select".to_string(),
                "stt.stop".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "stt.session.opened".to_string(),
                "stt.segment.appended".to_string(),
                "stt.session.closed".to_string(),
                "stt.selection.made".to_string(),
                "stt.retention.trimmed".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: stt_commands(),
        queries: Vec::new(),
        events: stt_events(),
        resources: vec![ResourceDoc {
            namespace: "stt".to_string(),
            summary: "Backend resource surface installed as ctx.resource.stt for apps that \
                      declare the stt resource. Apps read the transcript and record selections; \
                      capture is host-owned and never reaches this surface."
                .to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Read the live transcript and record a selection".to_string(),
            summary: "A scribe-style app polls the folded transcript, then records a user-chosen \
                      slice. The slice text is re-derived by the core from the folded segments."
                .to_string(),
            language: "js".to_string(),
            code: "var stt = ctx.resource[\"stt\"];\nfunction handle(input) {\n  var segs = JSON.parse(stt.segments(input[1]));\n  if (input[0] === \"select\") { return String(stt.select(input[1], input[2], input[3], \"clipboard\")); }\n  return segs.map(function (s) { return s.text; }).join(\" \");\n}".to_string(),
            expected: "The joined text of the requested segment range, copied to the clipboard sink.".to_string(),
        }],
        constraints: vec![
            "Mic capture, VAD, and ASR run entirely at the host edge; only finalized transcript \
             text (plus integer timings) crosses into the core."
                .to_string(),
            "Raw audio is never an event. Replay folds segments without re-running inference."
                .to_string(),
            "Locale/clock never enter decide or fold: segment_seq, start_ms, end_ms all arrive \
             inside events."
                .to_string(),
            "stt.session.open/segment.append/session.close-host/retention.trim are trusted-host \
             only; apps may call only stt.select and stt.session.close."
                .to_string(),
        ],
        limits: vec![],
        compatibility: vec![],
        internal: vec![],
    }
}

fn stt_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "stt.session.open",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Edge-minted session token ([A-Za-z0-9_-]+).", STR),
                param("host_id", "Host that owns the session lifecycle.", STR),
                param("executor_host_id", "Host that will run ASR (sync guard).", STR),
                param("model", "ASR model id, e.g. whisper-tiny.", STR),
                param(
                    "sample_rate_hz",
                    &format!("PCM sample rate in Hz (default {DEFAULT_SAMPLE_RATE_HZ})."),
                    "u32",
                ),
                param("--origin-replica", "Optional replica id this session originated on.", "u64?"),
            ],
            "stt.session.opened",
            "Open a capture session. Trusted-host only: the host edge calls this after consent.",
        )
        .with_errors(&["app not found", "invalid session id", "requires trusted host"]),
        command_doc(
            "stt.segment.append",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("segment_seq", "Monotonic segment sequence (starts at 1).", "u64"),
                param("start_ms", "Offset from session open, milliseconds.", "u64"),
                param("end_ms", "Offset from session open, milliseconds.", "u64"),
                param("--confidence", "Confidence in thousandths (0-1000).", "u32?"),
                param("--lang", "BCP-47 language code.", STR),
                param("text", "Finalized transcript text for this segment.", STR),
            ],
            "stt.segment.appended",
            "Record one finalized transcript segment. Trusted-host only; fold is monotonic and \
             first-wins so retries and sync duplicates converge.",
        )
        .with_errors(&[
            "no open stt session",
            "session closed",
            "end_ms precedes start_ms",
            "requires trusted host",
        ]),
        command_doc(
            "stt.session.close-host",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("reason", "stopped | idle | revoked | host-exit | error:...", STR),
            ],
            "stt.session.closed",
            "Close a session from the host edge. Trusted-host only.",
        )
        .with_errors(&["no stt session", "unknown close reason", "requires trusted host"]),
        command_doc(
            "stt.retention.trim",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("dropped_before_seq", "New floor; segments with seq < this are dropped.", "u64"),
            ],
            "stt.retention.trimmed",
            "Raise the retention floor; retained segments below it are dropped. Trusted-host only. \
             The durable event log is untouched (compaction is deferred).",
        )
        .with_errors(&["no stt session", "requires trusted host"]),
        command_doc(
            "stt.select",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("from_segment_seq", "Inclusive start segment seq.", "u64"),
                param("to_segment_seq", "Inclusive end segment seq.", "u64"),
                param("sink", "Where the slice goes: clipboard, field, app:<id>, note.", STR),
            ],
            "stt.selection.made",
            "Record a user-chosen transcript slice. App-callable. The slice text is re-derived \
             from the folded segments, so it is authoritative and unforgeable.",
        )
        .with_errors(&[
            "no stt session",
            "to_segment_seq precedes from_segment_seq",
            "from_segment_seq trimmed",
            "from_segment_seq beyond last segment",
        ]),
        command_doc(
            "stt.stop",
            &[param("app", "Owning app id.", STR), param("session_id", "Session token.", STR)],
            "stt.session.closed",
            "Close a session from an app backend (ctx.resource.stt.stop). Records reason stopped; \
             fold makes a repeated close a no-op.",
        )
        .with_errors(&["no stt session"]),
    ]
}

fn stt_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "stt.session.opened",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("host_id", "Owning host.", STR),
                param("executor_host_id", "ASR-executing host.", STR),
                param("origin_replica", "Originating replica, if any.", "u64?"),
                param("model", "ASR model id.", STR),
                param("sample_rate_hz", "PCM sample rate.", "u32"),
            ],
            "A capture session opened. First-wins: a replay of an open for an existing session \
             is a no-op.",
        ),
        event_doc(
            "stt.segment.appended",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("segment_seq", "Segment sequence.", "u64"),
                param("start_ms", "Start offset from session open.", "u64"),
                param("end_ms", "End offset from session open.", "u64"),
                param("text", "Finalized transcript text.", STR),
                param("confidence_milli", "Confidence in thousandths, if known.", "u32?"),
                param("lang", "BCP-47 language code, if known.", STR),
            ],
            "One finalized transcript segment. Applied monotonically with first-wins idempotence.",
        ),
        event_doc(
            "stt.session.closed",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("reason", "Why the session ended.", STR),
            ],
            "A capture session closed. First close wins.",
        ),
        event_doc(
            "stt.selection.made",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("selection_id", "Deterministic id of this slice+sink.", STR),
                param("from_segment_seq", "Inclusive start.", "u64"),
                param("to_segment_seq", "Inclusive end.", "u64"),
                param("text", "Re-derived slice text.", STR),
                param("sink", "Destination.", STR),
            ],
            "A recorded user selection. First-wins by selection_id.",
        ),
        event_doc(
            "stt.retention.trimmed",
            &[
                param("app", "Owning app id.", STR),
                param("session_id", "Session token.", STR),
                param("dropped_before_seq", "New floor.", "u64"),
            ],
            "Retention floor advanced; older retained segments dropped.",
        ),
    ]
}

/// Mirror of [`resources::resource_methods`] as docs. The match is exhaustive:
/// adding a resource method without documenting it would otherwise `unreachable!`
/// panic at doc generation (the kv-capability precedent).
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
        ("call", "select") => "string — the re-derived slice text for the chosen range",
        ("call", "stop") => "string — `ok` once the close is recorded",
        ("read", "sessions") => "string — JSON array of this app's sessions (open first)",
        ("read", "segments") => "string — JSON array of retained finalized segments, oldest first",
        ("read", "selections") => "string — JSON array of recorded selections, by selection_id",
        ("write", _) => "void",
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
        "select" => "Record a user-chosen transcript slice and return the re-derived text.",
        "stop" => "Close the session (reason stopped); returns ok.",
        _ => "Call method.",
    }
}

fn read_summary(name: &str) -> &'static str {
    match name {
        "sessions" => "This app's sessions as JSON (open first).",
        "segments" => "Retained finalized segments for a session as JSON, oldest first.",
        "selections" => "Recorded selections for a session as JSON, by selection_id.",
        _ => "Read method.",
    }
}
