//! terrane-domain — the pure vocabulary the core speaks.
//!
//! `Command`, `Event`, `State`, `Id`, and `Error`: the shapes that flow through
//! the engine. Pure (no I/O), intended to stay wasm-clean. The engine in
//! `terrane-core` turns Commands into Events and folds Events into State; the
//! rules for *how* live there, the words live here.
//!
//! Only [`Event`] is serialized — it is the durable truth written to the log —
//! so it alone derives borsh. Commands are transient and State is rebuilt by
//! folding, so neither needs a wire format.
//!
//! First slice: the **app catalog** — the user's saved apps.

use borsh::{BorshDeserialize, BorshSerialize};
use std::collections::BTreeMap;

/// Identifier for a saved app. Caller-supplied and stable.
pub type AppId = String;

/// A saved app, as the user sees it in their catalog. `source` is where the
/// app's body lives — a path to its bundle (UI + backend). It is metadata for
/// now; the host will read it once it can run apps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppRecord {
    pub id: AppId,
    pub name: String,
    pub source: Option<String>,
}

/// An intent to change the catalog. Commands are *requests*; the engine decides
/// whether they are allowed and what Events they produce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    AddApp {
        id: AppId,
        name: String,
        source: Option<String>,
    },
    RemoveApp {
        id: AppId,
    },
    /// Store a value under `key` in `app`'s key/value resource.
    KvSet {
        app: AppId,
        key: String,
        value: String,
    },
    /// Remove `key` from `app`'s key/value resource.
    KvDelete {
        app: AppId,
        key: String,
    },
    /// Fetch `url` over the network into `app`'s recorded responses. This is the
    /// engine's first *effectful* command: it is performed at the edge and its
    /// result is recorded as [`Event::Fetched`], so replay reproduces it without
    /// touching the network.
    Fetch {
        app: AppId,
        url: String,
    },
}

/// A fact that has happened. Events are the durable truth: the log is a list of
/// Events, and folding them from empty reproduces the State exactly.
///
/// Serialized with borsh (binary, deterministic). Variant *order* is part of the
/// wire format — append new variants at the end, never reorder existing ones.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Event {
    AppAdded {
        id: AppId,
        name: String,
        source: Option<String>,
    },
    AppRemoved {
        id: AppId,
    },
    KvSet {
        app: AppId,
        key: String,
        value: String,
    },
    KvDeleted {
        app: AppId,
        key: String,
    },
    /// The recorded result of a `Fetch`. Carrying the body in the event is what
    /// makes the non-deterministic network call deterministic on replay.
    Fetched {
        app: AppId,
        url: String,
        status: u16,
        body: String,
    },
}

/// A recorded network response, rebuilt by folding a `Fetched` event. It lives
/// in State so apps can read what was fetched; it is never serialized directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    pub status: u16,
    pub body: String,
}

/// The whole world the core holds: the catalog of saved apps, each app's
/// key/value resource, and each app's recorded network responses. Keyed and
/// ordered (BTreeMap keeps iteration deterministic, which keeps replay
/// deterministic).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    pub apps: BTreeMap<AppId, AppRecord>,
    pub data: BTreeMap<AppId, BTreeMap<String, String>>,
    /// Recorded network responses per app, keyed by URL.
    pub fetches: BTreeMap<AppId, BTreeMap<String, FetchResponse>>,
}

/// Typed errors. No panics on real paths — every failure is one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    AppExists(AppId),
    AppNotFound(AppId),
    KeyNotFound(AppId, String),
    InvalidInput(String),
    Storage(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AppExists(id) => write!(f, "app already exists: {id}"),
            Error::AppNotFound(id) => write!(f, "app not found: {id}"),
            Error::KeyNotFound(app, key) => write!(f, "key not found: {app}/{key}"),
            Error::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Error::Storage(msg) => write!(f, "storage error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias used throughout the engine.
pub type Result<T> = std::result::Result<T, Error>;
