//! Capabilities — the pluggable units of the engine.
//!
//! Each capability owns a namespace and is wholly responsible for its commands,
//! its events, deciding, and folding. Add one here and register it in
//! [`crate::default_registry`]; nothing else central changes.

use std::collections::BTreeMap;

use terrane_domain::{Error, EventRecord, Result};

use crate::{Decision, State};

pub mod app;
pub mod build;
pub mod builder;
pub mod codex;
pub mod crdt;
pub mod host;
pub mod kv;
pub mod model;
pub mod net;
pub mod replica;

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

    /// The methods this capability exposes to a backend on `ctx.resource.<ns>`.
    /// This single declaration drives BOTH the runtime bridge (which installs
    /// exactly these methods) and the generated API docs — so they cannot drift.
    /// Default: none (the capability is not a backend resource).
    fn resource_api(&self) -> Vec<ResourceMethod> {
        Vec::new()
    }
}

/// A value a resource read hands back to the backend JS.
pub enum ReadValue {
    /// A string or, if absent, JS `null`/`undefined`.
    OptString(Option<String>),
    /// A JS object `{ key: value, … }`.
    StringMap(BTreeMap<String, String>),
    /// A JS array `[ value, … ]`.
    StringList(Vec<String>),
}

/// A pure read over a capability's State slice for one app: `(state, app, args)`.
pub type ReadFn = fn(&State, &str, &[String]) -> ReadValue;

/// One method a capability exposes on `ctx.resource.<namespace>`.
pub enum ResourceMethod {
    /// A write, forwarded to the command `<namespace>.<name>` (app id prepended,
    /// args validated by that capability's `decide`).
    Write {
        name: &'static str,
        params: &'static [&'static str],
    },
    /// A read, served from the capability's State slice — no event, not recorded.
    Read {
        name: &'static str,
        params: &'static [&'static str],
        read: ReadFn,
    },
}

impl ResourceMethod {
    pub fn name(&self) -> &'static str {
        match self {
            ResourceMethod::Write { name, .. } | ResourceMethod::Read { name, .. } => name,
        }
    }

    pub fn params(&self) -> &'static [&'static str] {
        match self {
            ResourceMethod::Write { params, .. } | ResourceMethod::Read { params, .. } => params,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ResourceMethod::Write { .. } => "write",
            ResourceMethod::Read { .. } => "read",
        }
    }
}

/// Fetch a positional argument or fail with a clear message.
pub(crate) fn arg(args: &[String], index: usize, what: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| Error::InvalidInput(format!("missing {what}")))
}
