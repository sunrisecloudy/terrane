//! The in-process event sink the core emits [`CoreEvent`]s onto.
//!
//! prd-merged/01 CR-A1 (shells subscribe to `Event<Payload>`), prd-merged/02
//! §observability. In M0a the sink is a simple in-memory collector owned by the
//! [`WorkspaceCore`](crate::WorkspaceCore): every `runtime.run` emits
//! `run.started` / `ui.patch` / `run.completed` (or `run.failed`) events through
//! it, and tests/CLI drain them for assertions and display.
//!
//! Events carry a monotone [`LogicalTimestamp`] so the deterministic spine
//! orders them by logical time rather than wall-clock (the sink mints the ids
//! and timestamps so callers never have to).

use forge_domain::{AppletId, CoreEvent, EventId, LogicalTimestamp};

/// An in-memory, append-only collector of [`CoreEvent`]s.
///
/// The sink owns the monotone event clock: each [`emit`](EventSink::emit) mints
/// the next [`EventId`]/[`LogicalTimestamp`] so emitted events are totally
/// ordered and uniquely identified without the caller tracking counters.
#[derive(Debug, Default)]
pub struct EventSink {
    events: Vec<CoreEvent>,
    clock: LogicalTimestamp,
    next_event_seq: u64,
}

impl EventSink {
    pub fn new() -> Self {
        EventSink::default()
    }

    /// Emit an event of `kind` (e.g. `run.started`, `ui.patch`) carrying
    /// `payload`, scoped to `applet_id` when present. Returns the minted
    /// [`EventId`] so a caller can correlate.
    pub fn emit(
        &mut self,
        applet_id: Option<AppletId>,
        kind: impl Into<String>,
        payload: serde_json::Value,
    ) -> EventId {
        let event_id = EventId::new(format!("ev_{}", self.next_event_seq));
        self.next_event_seq += 1;
        self.clock = self.clock.next();
        let event = CoreEvent {
            event_id: event_id.clone(),
            applet_id,
            kind: kind.into(),
            payload,
            created_at_logical: self.clock,
        };
        self.events.push(event);
        event_id
    }

    /// The [`LogicalTimestamp`] the NEXT [`emit`](EventSink::emit) will stamp,
    /// WITHOUT advancing the clock. A producer whose durable audit row must commit in
    /// the SAME `Store::transact` as its decision uses this to stamp the row with the
    /// timestamp its observability event will carry, then emits that event ONLY after
    /// the transaction commits — so a rolled-back decision neither persists a row nor
    /// emits a spurious event, while a committed one keeps the transient event and the
    /// durable row under one clock (SC-12 §2 atomicity). Because `emit` derives the
    /// stamp the same way (`clock.next()`), a peek immediately followed by an `emit`
    /// with no intervening emission yields the SAME value.
    pub fn peek_next_logical_time(&self) -> LogicalTimestamp {
        self.clock.next()
    }

    /// All events emitted so far, in emission order.
    pub fn events(&self) -> &[CoreEvent] {
        &self.events
    }

    /// Number of events emitted so far.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether no events have been emitted.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Every event whose `kind` equals `kind`, in order (observability filter).
    pub fn events_of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a CoreEvent> + 'a {
        self.events.iter().filter(move |e| e.kind == kind)
    }

    /// Drain and return all collected events, resetting the sink's buffer (the
    /// logical clock keeps advancing so subsequent events stay ordered).
    pub fn drain(&mut self) -> Vec<CoreEvent> {
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_mints_unique_ids_and_monotone_timestamps() {
        let mut sink = EventSink::new();
        assert!(sink.is_empty());
        let a = sink.emit(None, "run.started", serde_json::json!({}));
        let b = sink.emit(Some(AppletId::new("x")), "ui.patch", serde_json::json!({}));
        assert_ne!(a, b, "event ids must be unique");
        assert_eq!(sink.len(), 2);
        let ts: Vec<_> = sink.events().iter().map(|e| e.created_at_logical).collect();
        assert!(ts[0] < ts[1], "logical timestamps must be strictly monotone");
        assert_eq!(sink.events()[1].applet_id, Some(AppletId::new("x")));
    }

    #[test]
    fn events_of_kind_filters() {
        let mut sink = EventSink::new();
        sink.emit(None, "run.started", serde_json::json!({}));
        sink.emit(None, "ui.patch", serde_json::json!({}));
        sink.emit(None, "ui.patch", serde_json::json!({}));
        assert_eq!(sink.events_of_kind("ui.patch").count(), 2);
        assert_eq!(sink.events_of_kind("run.started").count(), 1);
        assert_eq!(sink.events_of_kind("nope").count(), 0);
    }

    #[test]
    fn drain_empties_buffer_but_clock_keeps_advancing() {
        let mut sink = EventSink::new();
        sink.emit(None, "a", serde_json::json!({}));
        let drained = sink.drain();
        assert_eq!(drained.len(), 1);
        assert!(sink.is_empty());
        // A new event after drain still gets a later timestamp and fresh id.
        sink.emit(None, "b", serde_json::json!({}));
        assert!(sink.events()[0].created_at_logical > drained[0].created_at_logical);
        assert_ne!(sink.events()[0].event_id, drained[0].event_id);
    }
}
