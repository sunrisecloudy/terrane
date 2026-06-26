//! terrane-domain — shared primitives and the dispatch boundary.
//!
//! What crosses every capability lives here: identifiers, the typed [`Error`],
//! and the two envelopes a command and an event take at the core boundary —
//! [`Request`] in, [`EventRecord`] out. Capability-specific commands, events,
//! and state live with their capability in `terrane-core`, not here.

use borsh::{BorshDeserialize, BorshSerialize};

/// Identifier for a saved app. Caller-supplied and stable.
pub type AppId = String;

/// A command as it arrives at the core: a namespaced name like `"app.add"` plus
/// the caller's argument tokens. The registry routes on the namespace (the part
/// before the first `.`); the owning capability interprets the rest and the args.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub name: String,
    pub args: Vec<String>,
}

impl Request {
    pub fn new(name: impl Into<String>, args: Vec<String>) -> Self {
        Request {
            name: name.into(),
            args,
        }
    }
}

/// A recorded event on the wire: a stable kind tag like `"app.added"` plus the
/// borsh-encoded payload of the capability's own event type. Name-tagging is what
/// makes the log extensible — adding a new kind never disturbs existing records,
/// and replay routes each record to whatever capability understands its kind.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EventRecord {
    pub kind: String,
    pub payload: Vec<u8>,
}

/// Typed errors. No panics on real paths — every failure is one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    AppExists(AppId),
    AppNotFound(AppId),
    KeyNotFound(AppId, String),
    InvalidInput(String),
    Storage(String),
    /// Backend (JS) execution failed: a thrown exception, a compile error, or a
    /// missing/unreadable bundle.
    Runtime(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AppExists(id) => write!(f, "app already exists: {id}"),
            Error::AppNotFound(id) => write!(f, "app not found: {id}"),
            Error::KeyNotFound(app, key) => write!(f, "key not found: {app}/{key}"),
            Error::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Error::Storage(msg) => write!(f, "storage error: {msg}"),
            Error::Runtime(msg) => write!(f, "runtime error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias used throughout the engine.
pub type Result<T> = std::result::Result<T, Error>;
