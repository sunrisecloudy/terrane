//! terrane-core — the deterministic, replayable engine.
//!
//! The single shape: `apply(Command) -> [Event] -> State`, with the event log
//! persisted so that replaying it reproduces identical state. Platform effects
//! never live here.
//!
//! Scaffold only — `apply`, persistence, and replay arrive with the first
//! vertical slice.
