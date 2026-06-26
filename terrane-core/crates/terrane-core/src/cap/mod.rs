//! Capabilities — the pluggable units of the engine.
//!
//! Each capability owns a namespace and is wholly responsible for its commands,
//! its events, deciding, and folding. Add one here and register it in
//! [`crate::default_registry`]; nothing else central changes.

use terrane_domain::{Error, EventRecord, Result};

use crate::{Decision, State};

pub mod app;
pub mod host;
pub mod kv;
pub mod model;
pub mod net;

/// A self-contained slice of engine behaviour.
pub trait Capability {
    /// The namespace this capability owns, e.g. `"app"`. Commands named
    /// `"<namespace>.<verb>"` and events `"<namespace>.<kind>"` route here.
    fn namespace(&self) -> &'static str;

    /// Decide a command in this namespace. `name` is the full command name
    /// (`"app.add"`); `args` are the caller's tokens. Validate against `state`
    /// (reads across slices are fine) and return events to commit or an effect
    /// to run. Pure: no I/O, no clock, no rng.
    fn decide(&self, state: &State, name: &str, args: &[String]) -> Result<Decision>;

    /// Fold one recorded event into State. Called for *every* event (broadcast),
    /// so match on `record.kind` and ignore the ones you don't care about — this
    /// is also how a capability reacts to another's events.
    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()>;

    /// Optional human-readable one-liner for one of this capability's events,
    /// used by `terrane log`. Defaults to none (the engine falls back to the raw
    /// kind + byte size).
    fn describe(&self, record: &EventRecord) -> Option<String> {
        let _ = record;
        None
    }
}

/// Fetch a positional argument or fail with a clear message.
pub(crate) fn arg(args: &[String], index: usize, what: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| Error::InvalidInput(format!("missing {what}")))
}
