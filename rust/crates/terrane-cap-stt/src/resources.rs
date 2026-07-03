use terrane_cap_interface::{
    state_ref, Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::types::SttState;

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Call {
            name: "select",
            params: &["sessionId", "fromSeq", "toSeq", "sink"],
        },
        ResourceMethod::Call {
            name: "stop",
            params: &["sessionId"],
        },
        ResourceMethod::Read {
            name: "sessions",
            params: &[],
        },
        ResourceMethod::Read {
            name: "segments",
            params: &["sessionId"],
        },
        ResourceMethod::Read {
            name: "selections",
            params: &["sessionId"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "sessions" => read_sessions(ctx.state, ctx.app),
        "segments" => read_segments(ctx.state, ctx.app, args),
        "selections" => read_selections(ctx.state, ctx.app, args),
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: stt.{other}"
        ))),
    }
}

/// `ctx.resource.stt.sessions()` — this app's sessions (open first, then by id),
/// as JSON for the transcript UI.
fn read_sessions(state: &dyn StateStore, app: &str) -> Result<ReadValue> {
    let stt = state_ref::<SttState>(state, "stt")?;
    let mut ordered: Vec<_> = stt
        .sessions
        .get(app)
        .map(|m| m.values().collect::<Vec<_>>())
        .unwrap_or_default();
    // Open sessions first, then by session_id for determinism.
    ordered.sort_by(|a, b| {
        b.status
            .is_open()
            .cmp(&a.status.is_open())
            .then_with(|| a.session_id.cmp(&b.session_id))
    });
    let encoded = serde_json::to_string(
        &ordered
            .iter()
            .map(|session| {
                serde_json::json!({
                    "sessionId": session.session_id,
                    "model": session.model,
                    "sampleRateHz": session.sample_rate_hz,
                    "status": if session.status.is_open() { "open" } else { "closed" },
                    "hostId": session.host_id,
                    "executorHostId": session.executor_host_id,
                    "segments": session.segments.len(),
                    "lastSegmentSeq": session.last_segment_seq,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| Error::InvalidInput(format!("stt sessions encode failed: {e}")))?;
    Ok(ReadValue::OptString(Some(encoded)))
}

/// `ctx.resource.stt.segments(sessionId)` — the retained, finalized segments
/// for a session, oldest first, as JSON.
fn read_segments(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let session_id = args.first().map(String::as_str).unwrap_or_default();
    if session_id.is_empty() {
        return Err(Error::InvalidInput("stt.segments needs a sessionId".into()));
    }
    let stt = state_ref::<SttState>(state, "stt")?;
    let session = stt
        .sessions
        .get(app)
        .and_then(|m| m.get(session_id))
        .ok_or_else(|| {
            Error::InvalidInput(format!("no stt session: {app}/{session_id}"))
        })?;
    let encoded = serde_json::to_string(
        &session
            .segments
            .values()
            .map(|segment| {
                serde_json::json!({
                    "seq": segment.segment_seq,
                    "startMs": segment.start_ms,
                    "endMs": segment.end_ms,
                    "text": segment.text,
                    "confidence": segment.confidence_milli,
                    "lang": segment.lang,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| Error::InvalidInput(format!("stt segments encode failed: {e}")))?;
    Ok(ReadValue::OptString(Some(encoded)))
}

/// `ctx.resource.stt.selections(sessionId)` — the recorded selections for a
/// session, by selection_id, as JSON.
fn read_selections(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let session_id = args.first().map(String::as_str).unwrap_or_default();
    if session_id.is_empty() {
        return Err(Error::InvalidInput(
            "stt.selections needs a sessionId".into(),
        ));
    }
    let stt = state_ref::<SttState>(state, "stt")?;
    let session = stt
        .sessions
        .get(app)
        .and_then(|m| m.get(session_id))
        .ok_or_else(|| {
            Error::InvalidInput(format!("no stt session: {app}/{session_id}"))
        })?;
    let encoded = serde_json::to_string(
        &session
            .selections
            .values()
            .map(|selection| {
                serde_json::json!({
                    "selectionId": selection.selection_id,
                    "fromSeq": selection.from_segment_seq,
                    "toSeq": selection.to_segment_seq,
                    "text": selection.text,
                    "sink": selection.sink,
                })
            })
            .collect::<Vec<_>>(),
    )
    .map_err(|e| Error::InvalidInput(format!("stt selections encode failed: {e}")))?;
    Ok(ReadValue::OptString(Some(encoded)))
}
