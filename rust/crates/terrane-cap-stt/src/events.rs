use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;
use terrane_cap_interface::{
    decode_app_removed, decode_event, encode_event, state_mut, truncate, EventRecord, Result,
    StateStore,
};

use crate::types::{SttSegment, SttSelection, SttSession, SttState, SttStatus};

#[derive(BorshSerialize, BorshDeserialize)]
struct SessionOpened {
    app: String,
    session_id: String,
    host_id: String,
    executor_host_id: String,
    origin_replica: Option<u64>,
    model: String,
    sample_rate_hz: u32,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct SegmentAppended {
    app: String,
    session_id: String,
    segment_seq: u64,
    start_ms: u64,
    end_ms: u64,
    text: String,
    confidence_milli: Option<u32>,
    lang: Option<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct SessionClosed {
    app: String,
    session_id: String,
    reason: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct SelectionMade {
    app: String,
    session_id: String,
    selection_id: String,
    from_segment_seq: u64,
    to_segment_seq: u64,
    text: String,
    sink: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct RetentionTrimmed {
    app: String,
    session_id: String,
    dropped_before_seq: u64,
}

/// Everything a session open records. The host edge fills this after consent,
/// so the `"stt.session.opened"` payload shape stays owned by this crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOpenedRecord {
    pub app: String,
    pub session_id: String,
    pub host_id: String,
    pub executor_host_id: String,
    pub origin_replica: Option<u64>,
    pub model: String,
    pub sample_rate_hz: u32,
}

/// One finalized ASR segment the host edge dispatches as a trusted append.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentAppendedRecord {
    pub app: String,
    pub session_id: String,
    pub segment_seq: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence_milli: Option<u32>,
    pub lang: Option<String>,
}

/// A recorded selection (the app-facing surface re-derives `text` and mints
/// `selection_id`, so only the commit-relevant fields cross here).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionMadeRecord {
    pub app: String,
    pub session_id: String,
    pub selection_id: String,
    pub from_segment_seq: u64,
    pub to_segment_seq: u64,
    pub text: String,
    pub sink: String,
}

/// Build the recorded event for a capture session opening.
pub fn session_opened_event(record: &SessionOpenedRecord) -> Result<EventRecord> {
    encode_event(
        "stt.session.opened",
        &SessionOpened {
            app: record.app.clone(),
            session_id: record.session_id.clone(),
            host_id: record.host_id.clone(),
            executor_host_id: record.executor_host_id.clone(),
            origin_replica: record.origin_replica,
            model: record.model.clone(),
            sample_rate_hz: record.sample_rate_hz,
        },
    )
}

/// Build the recorded event for one finalized transcript segment.
pub fn segment_appended_event(record: &SegmentAppendedRecord) -> Result<EventRecord> {
    encode_event(
        "stt.segment.appended",
        &SegmentAppended {
            app: record.app.clone(),
            session_id: record.session_id.clone(),
            segment_seq: record.segment_seq,
            start_ms: record.start_ms,
            end_ms: record.end_ms,
            text: record.text.clone(),
            confidence_milli: record.confidence_milli,
            lang: record.lang.clone(),
        },
    )
}

/// Build the recorded event for a session closing.
pub fn session_closed_event(app: &str, session_id: &str, reason: &str) -> Result<EventRecord> {
    encode_event(
        "stt.session.closed",
        &SessionClosed {
            app: app.to_string(),
            session_id: session_id.to_string(),
            reason: reason.to_string(),
        },
    )
}

/// Build the recorded event for a user selection (text re-derived by `decide`).
pub fn selection_made_event(record: &SelectionMadeRecord) -> Result<EventRecord> {
    encode_event(
        "stt.selection.made",
        &SelectionMade {
            app: record.app.clone(),
            session_id: record.session_id.clone(),
            selection_id: record.selection_id.clone(),
            from_segment_seq: record.from_segment_seq,
            to_segment_seq: record.to_segment_seq,
            text: record.text.clone(),
            sink: record.sink.clone(),
        },
    )
}

/// Build the recorded event for a retention floor advance.
pub fn retention_trimmed_event(
    app: &str,
    session_id: &str,
    dropped_before_seq: u64,
) -> Result<EventRecord> {
    encode_event(
        "stt.retention.trimmed",
        &RetentionTrimmed {
            app: app.to_string(),
            session_id: session_id.to_string(),
            dropped_before_seq,
        },
    )
}

/// The joined selection text inside a freshly committed batch (used by the
/// `select` call surface to hand the re-derived text back to the app).
pub(crate) fn selection_text_from_records(records: &[EventRecord]) -> Option<String> {
    records
        .iter()
        .rev()
        .find(|record| record.kind == "stt.selection.made")
        .and_then(|record| decode_event::<SelectionMade>(record).ok())
        .map(|selection| selection.text)
}

pub(crate) fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "stt.session.opened" => {
            let e: SessionOpened = decode_event(record)?;
            let app_sessions = state_mut::<SttState>(state, "stt")?
                .sessions
                .entry(e.app)
                .or_default();
            // First-wins: a replay of an already-open session is a no-op.
            app_sessions.entry(e.session_id.clone()).or_insert(SttSession {
                session_id: e.session_id,
                host_id: e.host_id,
                executor_host_id: e.executor_host_id,
                origin_replica: e.origin_replica,
                model: e.model,
                sample_rate_hz: e.sample_rate_hz,
                status: SttStatus::Open,
                closed_reason: None,
                segments: BTreeMap::new(),
                last_segment_seq: 0,
                dropped_before_seq: 0,
                selections: BTreeMap::new(),
            });
        }
        "stt.segment.appended" => {
            let e: SegmentAppended = decode_event(record)?;
            let app_sessions = state_mut::<SttState>(state, "stt")?.sessions.get_mut(&e.app);
            let Some(session) = app_sessions.and_then(|m| m.get_mut(&e.session_id)) else {
                // Unknown app/session: nothing to attach. Segments are only
                // meaningful inside an open, owned session.
                return Ok(());
            };
            if !session.status.is_open() {
                return Ok(());
            }
            // Monotonic, first-wins: a seq at or below the high-water mark, or
            // below the retention floor, is a replay/sync duplicate.
            if e.segment_seq <= session.last_segment_seq
                || e.segment_seq < session.dropped_before_seq
            {
                return Ok(());
            }
            session.segments.insert(
                e.segment_seq,
                SttSegment {
                    segment_seq: e.segment_seq,
                    start_ms: e.start_ms,
                    end_ms: e.end_ms,
                    text: e.text,
                    confidence_milli: e.confidence_milli,
                    lang: e.lang,
                },
            );
            session.last_segment_seq = e.segment_seq;
        }
        "stt.session.closed" => {
            let e: SessionClosed = decode_event(record)?;
            let app_sessions = state_mut::<SttState>(state, "stt")?.sessions.get_mut(&e.app);
            if let Some(session) = app_sessions.and_then(|m| m.get_mut(&e.session_id)) {
                // First close wins; a later close (e.g. duplicate dispatch) is a no-op.
                if session.status.is_open() {
                    session.status = SttStatus::Closed;
                    session.closed_reason = Some(e.reason);
                }
            }
        }
        "stt.selection.made" => {
            let e: SelectionMade = decode_event(record)?;
            let app_sessions = state_mut::<SttState>(state, "stt")?.sessions.get_mut(&e.app);
            if let Some(session) = app_sessions.and_then(|m| m.get_mut(&e.session_id)) {
                // First-wins by selection_id: re-dispatching the same selection
                // (same range + sink) is idempotent.
                session.selections.entry(e.selection_id.clone()).or_insert(SttSelection {
                    selection_id: e.selection_id,
                    from_segment_seq: e.from_segment_seq,
                    to_segment_seq: e.to_segment_seq,
                    text: e.text,
                    sink: e.sink,
                });
            }
        }
        "stt.retention.trimmed" => {
            let e: RetentionTrimmed = decode_event(record)?;
            let app_sessions = state_mut::<SttState>(state, "stt")?.sessions.get_mut(&e.app);
            if let Some(session) = app_sessions.and_then(|m| m.get_mut(&e.session_id)) {
                if e.dropped_before_seq > session.dropped_before_seq {
                    session.dropped_before_seq = e.dropped_before_seq;
                    session.segments.retain(|seq, _| *seq >= e.dropped_before_seq);
                }
            }
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            // Sessions are app-scoped and go with the app: a revoked app must
            // not inherit recorded transcripts.
            state_mut::<SttState>(state, "stt")?.sessions.remove(&e.id);
        }
        _ => {}
    }
    Ok(())
}

pub(crate) fn describe(record: &EventRecord) -> Option<String> {
    match record.kind.as_str() {
        "stt.session.opened" => {
            let e: SessionOpened = decode_event(record).ok()?;
            Some(format!(
                "stt.session.opened {}/{} ({} Hz, model {})",
                e.app, e.session_id, e.sample_rate_hz, e.model
            ))
        }
        "stt.segment.appended" => {
            let e: SegmentAppended = decode_event(record).ok()?;
            Some(format!(
                "stt.segment.appended {}/{}#{} [{}..{}ms]: {}",
                e.app,
                e.session_id,
                e.segment_seq,
                e.start_ms,
                e.end_ms,
                truncate(&e.text, 40)
            ))
        }
        "stt.session.closed" => {
            let e: SessionClosed = decode_event(record).ok()?;
            Some(format!(
                "stt.session.closed {}/{} ({})",
                e.app, e.session_id, e.reason
            ))
        }
        "stt.selection.made" => {
            let e: SelectionMade = decode_event(record).ok()?;
            Some(format!(
                "stt.selection.made {}/{} [{}..{}] -> {} ({} chars)",
                e.app,
                e.session_id,
                e.from_segment_seq,
                e.to_segment_seq,
                e.sink,
                e.text.chars().count()
            ))
        }
        "stt.retention.trimmed" => {
            let e: RetentionTrimmed = decode_event(record).ok()?;
            Some(format!(
                "stt.retention.trimmed {}/{} (drop < {})",
                e.app, e.session_id, e.dropped_before_seq
            ))
        }
        _ => None,
    }
}
