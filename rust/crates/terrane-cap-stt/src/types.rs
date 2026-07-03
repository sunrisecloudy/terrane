use std::collections::BTreeMap;

use terrane_cap_interface::AppId;

/// Reasons a capture session ends. `"stopped"` is the user-initiated close (app
/// or host); `"idle"` is the always-on watchdog timing out; `"revoked"` follows
/// an `auth.revoke`; `"host-exit"` is the edge going away cleanly; `"error:…"`
/// carries a runner failure.
pub const CLOSE_REASONS: &[&str] = &["stopped", "idle", "revoked", "host-exit"];

/// The smallest phase-1 sample rate the edge promises to deliver. whisper.cpp
/// runs on 16 kHz mono PCM; the host downsamples before pushing segments.
pub const DEFAULT_SAMPLE_RATE_HZ: u32 = 16_000;

/// Whether a capture session is still receiving segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SttStatus {
    Open,
    Closed,
}

impl SttStatus {
    pub fn is_open(&self) -> bool {
        matches!(self, SttStatus::Open)
    }
}

/// One finalized transcript segment — the only ASR product the core ever sees.
/// `start_ms`/`end_ms` are offsets from session open (no wall clock in core),
/// and `confidence_milli` is confidence in thousandths so the slice stays `Eq`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttSegment {
    pub segment_seq: u64,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
    pub confidence_milli: Option<u32>,
    pub lang: Option<String>,
}

/// A user-chosen slice `[from_segment_seq..=to_segment_seq]` routed to a sink.
/// `text` is re-derived by `decide` from the folded segments, so the record is
/// authoritative: the app cannot forge or alter it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttSelection {
    pub selection_id: String,
    pub from_segment_seq: u64,
    pub to_segment_seq: u64,
    pub text: String,
    pub sink: String,
}

/// One capture session's folded view: open/closed status, the finalized segment
/// log, the high-water mark for idempotent append, the retention floor, and the
/// recorded selections.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SttSession {
    pub session_id: String,
    pub host_id: String,
    /// The host that ran ASR for this session. Carried from v1 so future LAN
    /// sync can tell which edge produced the segments and avoid double-capture.
    pub executor_host_id: String,
    pub origin_replica: Option<u64>,
    pub model: String,
    pub sample_rate_hz: u32,
    pub status: SttStatus,
    pub closed_reason: Option<String>,
    pub segments: BTreeMap<u64, SttSegment>,
    /// Monotonic high-water mark; `segment.appended` only applies when
    /// `segment_seq > last_segment_seq` (first-wins idempotence under retry/sync).
    pub last_segment_seq: u64,
    /// Lowest segment seq still retained; `retention.trimmed` raises it.
    pub dropped_before_seq: u64,
    pub selections: BTreeMap<String, SttSelection>,
}

/// This capability's slice of State: per-app sessions keyed by session id.
/// Reacts to `app.removed` by dropping that app's sessions wholesale (revoked
/// apps must not inherit recorded transcripts). All `BTreeMap` + integer fields
/// so the slice derives `Eq` and `replay_matches()` compares exactly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SttState {
    pub sessions: BTreeMap<AppId, BTreeMap<String, SttSession>>,
}
