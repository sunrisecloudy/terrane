use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use terrane_cap_interface::{
    arg, ensure_app_exists, required_tail, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{
    retention_trimmed_event, segment_appended_event, selection_made_event,
    session_closed_event, session_opened_event, SegmentAppendedRecord, SelectionMadeRecord,
    SessionOpenedRecord,
};
use crate::types::{SttState, CLOSE_REASONS};

/// `stt.session.open <app> <session_id> <host_id> <executor_host_id> <model>
/// <sample_rate_hz> [--origin-replica <replica>]` — trusted-host only. The host
/// edge mints `session_id` after consent; the core just validates and records.
/// Replay folds the first open for a given id and ignores duplicates.
pub(crate) fn decide_session_open(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let session_id = valid_token(arg(args, 1, "session_id")?, "session id")?;
    let host_id = non_empty_arg(arg(args, 2, "host_id")?)?;
    let executor_host_id = non_empty_arg(arg(args, 3, "executor_host_id")?)?;
    let model = non_empty_arg(arg(args, 4, "model")?)?;
    let sample_rate_hz = positive_u32(arg(args, 5, "sample_rate_hz")?)?;

    let mut origin_replica = None;
    let mut i = 6;
    while i < args.len() {
        match args[i].as_str() {
            "--origin-replica" => {
                let raw = arg(args, i + 1, "--origin-replica value")?;
                origin_replica = Some(parse_u64(&raw, "replica id")?);
                i += 2;
            }
            other => {
                return Err(Error::InvalidInput(format!(
                    "unknown option {other:?}; expected --origin-replica"
                )))
            }
        }
    }

    Ok(Decision::Commit(vec![session_opened_event(
        &SessionOpenedRecord {
            app,
            session_id,
            host_id,
            executor_host_id,
            origin_replica,
            model,
            sample_rate_hz,
        },
    )?]))
}

/// `stt.segment.append <app> <session_id> <segment_seq> <start_ms> <end_ms>
/// [--confidence <0-1000>] [--lang <code>] <text…>` — trusted-host only. The
/// edge delivers one finalized, VAD-closed utterance per call. Fold applies it
/// monotonically (first-wins), so a retried or sync-duplicated append is a no-op.
pub(crate) fn decide_segment_append(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let session_id = valid_token(arg(args, 1, "session_id")?, "session id")?;
    let segment_seq = parse_u64(&arg(args, 2, "segment_seq")?, "segment_seq")?;
    if segment_seq == 0 {
        return Err(Error::InvalidInput("segment_seq starts at 1".into()));
    }
    let start_ms = parse_u64(&arg(args, 3, "start_ms")?, "start_ms")?;
    let end_ms = parse_u64(&arg(args, 4, "end_ms")?, "end_ms")?;
    if end_ms < start_ms {
        return Err(Error::InvalidInput(format!(
            "end_ms ({end_ms}) must not precede start_ms ({start_ms})"
        )));
    }

    let mut confidence_milli = None;
    let mut lang = None;
    let mut i = 5;
    while i < args.len() {
        match args[i].as_str() {
            "--confidence" => {
                let raw = arg(args, i + 1, "--confidence value")?;
                let parsed = parse_u64(&raw, "confidence")?;
                if parsed > 1000 {
                    return Err(Error::InvalidInput(format!(
                        "--confidence is thousandths (0-1000), got {raw}"
                    )));
                }
                confidence_milli = Some(parsed as u32);
                i += 2;
            }
            "--lang" => {
                let value = non_empty_arg(arg(args, i + 1, "--lang value")?)?;
                lang = Some(value);
                i += 2;
            }
            _ => break,
        }
    }

    // The session must exist and be open before the edge appends to it. Fold is
    // additionally tolerant for the sync edge case where a segment precedes the
    // session.open at a replica.
    let state = state_ref::<SttState>(ctx.state, "stt")?;
    let session = state
        .sessions
        .get(&app)
        .and_then(|m| m.get(&session_id))
        .ok_or_else(|| Error::InvalidInput(format!("no open stt session: {app}/{session_id}")))?;
    if !session.status.is_open() {
        return Err(Error::InvalidInput(format!(
            "stt session {app}/{session_id} is closed; cannot append"
        )));
    }

    let text = required_tail(args, i, "text")?;
    Ok(Decision::Commit(vec![segment_appended_event(
        &SegmentAppendedRecord {
            app,
            session_id,
            segment_seq,
            start_ms,
            end_ms,
            text,
            confidence_milli,
            lang,
        },
    )?]))
}

/// `stt.session.close-host <app> <session_id> <reason>` — trusted-host only.
/// `reason` is `stopped`/`idle`/`revoked`/`host-exit` or an `error:…` detail.
pub(crate) fn decide_session_close_host(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let session_id = valid_token(arg(args, 1, "session_id")?, "session id")?;
    let reason = validate_reason(arg(args, 2, "reason")?)?;
    ensure_session_exists(ctx, &app, &session_id)?;
    Ok(Decision::Commit(vec![session_closed_event(
        &app, &session_id, &reason,
    )?]))
}

/// `stt.session.close <app> <session_id>` — app-callable (`ctx.resource.stt.stop`).
/// Always records reason `"stopped"`; fold makes a repeated close a no-op.
pub(crate) fn decide_session_close(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let session_id = valid_token(arg(args, 1, "session_id")?, "session id")?;
    ensure_session_exists(ctx, &app, &session_id)?;
    Ok(Decision::Commit(vec![session_closed_event(
        &app,
        &session_id,
        "stopped",
    )?]))
}

/// `stt.retention.trim <app> <session_id> <dropped_before_seq>` — trusted-host
/// only. Raises the retention floor and drops retained segments below it. The
/// durable event log is untouched (compaction is a later phase).
pub(crate) fn decide_retention_trim(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let session_id = valid_token(arg(args, 1, "session_id")?, "session id")?;
    let dropped_before_seq = parse_u64(&arg(args, 2, "dropped_before_seq")?, "dropped_before_seq")?;
    ensure_session_exists(ctx, &app, &session_id)?;
    Ok(Decision::Commit(vec![retention_trimmed_event(
        &app,
        &session_id,
        dropped_before_seq,
    )?]))
}

/// `stt.select <app> <session_id> <from_segment_seq> <to_segment_seq> <sink>`
/// — app-callable (`ctx.resource.stt.select`). The slice text is RE-DERIVED by
/// concatenating the folded segments in `[from..=to]`, so the record is
/// authoritative and the app cannot forge it. `selection_id` is a deterministic
/// hash of the tuple, so re-dispatching the same selection is idempotent.
pub(crate) fn decide_select(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let session_id = valid_token(arg(args, 1, "session_id")?, "session id")?;
    let from_segment_seq = parse_u64(&arg(args, 2, "from_segment_seq")?, "from_segment_seq")?;
    let to_segment_seq = parse_u64(&arg(args, 3, "to_segment_seq")?, "to_segment_seq")?;
    let sink = non_empty_arg(arg(args, 4, "sink")?)?;
    if to_segment_seq < from_segment_seq {
        return Err(Error::InvalidInput(format!(
            "to_segment_seq ({to_segment_seq}) must not precede from_segment_seq ({from_segment_seq})"
        )));
    }

    let state = state_ref::<SttState>(ctx.state, "stt")?;
    let session = state
        .sessions
        .get(&app)
        .and_then(|m| m.get(&session_id))
        .ok_or_else(|| Error::InvalidInput(format!("no stt session: {app}/{session_id}")))?;
    if from_segment_seq < session.dropped_before_seq {
        return Err(Error::InvalidInput(format!(
            "from_segment_seq ({from_segment_seq}) was trimmed (floor {})",
            session.dropped_before_seq
        )));
    }
    if from_segment_seq > session.last_segment_seq {
        return Err(Error::InvalidInput(format!(
            "from_segment_seq ({from_segment_seq}) is beyond the last segment ({})",
            session.last_segment_seq
        )));
    }

    let text = join_range(session, from_segment_seq, to_segment_seq);
    let selection_id = selection_id(&app, &session_id, from_segment_seq, to_segment_seq, &sink);
    Ok(Decision::Commit(vec![selection_made_event(
        &SelectionMadeRecord {
            app,
            session_id,
            selection_id,
            from_segment_seq,
            to_segment_seq,
            text,
            sink,
        },
    )?]))
}

/// Concatenate the retained segments in `[from..=to]`, in order, joined by
/// spaces. Missing seqs in the range (a rare gap) are skipped, so the derived
/// text is always the actual recorded words.
fn join_range(
    session: &crate::types::SttSession,
    from_segment_seq: u64,
    to_segment_seq: u64,
) -> String {
    session
        .segments
        .range(from_segment_seq..=to_segment_seq)
        .map(|(_, segment)| segment.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

fn ensure_session_exists(ctx: CommandCtx<'_>, app: &str, session_id: &str) -> Result<()> {
    let state = state_ref::<SttState>(ctx.state, "stt")?;
    if state
        .sessions
        .get(app)
        .and_then(|m| m.get(session_id))
        .is_some()
    {
        Ok(())
    } else {
        Err(Error::InvalidInput(format!(
            "no stt session: {app}/{session_id}"
        )))
    }
}

fn selection_id(app: &str, session_id: &str, from: u64, to: u64, sink: &str) -> String {
    let mut hasher = DefaultHasher::new();
    app.hash(&mut hasher);
    0u8.hash(&mut hasher);
    session_id.hash(&mut hasher);
    0u8.hash(&mut hasher);
    from.hash(&mut hasher);
    to.hash(&mut hasher);
    0u8.hash(&mut hasher);
    sink.hash(&mut hasher);
    format!("sel_{:016x}", hasher.finish())
}

fn valid_token(raw: String, label: &str) -> Result<String> {
    let valid = !raw.is_empty()
        && raw
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
    if !valid {
        return Err(Error::InvalidInput(format!(
            "{label} must be [A-Za-z0-9_-]+, got {raw:?}"
        )));
    }
    Ok(raw)
}

fn non_empty_arg(raw: String) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        Err(Error::InvalidInput("value must not be empty".into()))
    } else {
        Ok(trimmed.to_string())
    }
}

fn positive_u32(raw: String) -> Result<u32> {
    let parsed = parse_u64(&raw, "sample_rate_hz")?;
    if parsed == 0 {
        return Err(Error::InvalidInput(format!(
            "sample_rate_hz must be positive, got {raw:?}"
        )));
    }
    u32::try_from(parsed).map_err(|_| {
        Error::InvalidInput(format!("sample_rate_hz out of range, got {raw:?}"))
    })
}

fn parse_u64(raw: &str, label: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be a non-negative integer, got {raw:?}")))
}

fn validate_reason(raw: String) -> Result<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(Error::InvalidInput("reason must not be empty".into()));
    }
    let allowed = CLOSE_REASONS.contains(&trimmed) || trimmed.starts_with("error:");
    if !allowed {
        return Err(Error::InvalidInput(format!(
            "unknown close reason {trimmed:?}; expected one of {CLOSE_REASONS:?} or error:…"
        )));
    }
    Ok(trimmed.to_string())
}
