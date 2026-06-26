//! terrane-domain — the pure vocabulary the core speaks.
//!
//! `Command`, `Event`, `State`, `Id`, and `Error`: the shapes that flow through
//! the engine. Pure (no I/O), intended to stay wasm-clean. The engine in
//! `terrane-core` turns Commands into Events and folds Events into State; the
//! rules for *how* live there, the words live here.
//!
//! First slice: the **app catalog** — the user's saved apps.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Identifier for a saved app. Caller-supplied and stable.
pub type AppId = String;

/// A saved app, as the user sees it in their catalog. `source` is where the
/// app's body lives — a path to its bundle (UI + backend). It is metadata for
/// now; the host will read it once it can run apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppRecord {
    pub id: AppId,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// An intent to change the catalog. Commands are *requests*; the engine decides
/// whether they are allowed and what Events they produce.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    AddApp {
        id: AppId,
        name: String,
        #[serde(default)]
        source: Option<String>,
    },
    RemoveApp {
        id: AppId,
    },
}

/// A fact that has happened. Events are the durable truth: the log is a list of
/// Events, and folding them from empty reproduces the State exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    AppAdded {
        id: AppId,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source: Option<String>,
    },
    AppRemoved {
        id: AppId,
    },
}

/// The whole world the core holds: the catalog of saved apps, keyed and ordered
/// (BTreeMap keeps iteration deterministic, which keeps replay deterministic).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub apps: BTreeMap<AppId, AppRecord>,
}

/// Typed errors. No panics on real paths — every failure is one of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    AppExists(AppId),
    AppNotFound(AppId),
    InvalidInput(String),
    Storage(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AppExists(id) => write!(f, "app already exists: {id}"),
            Error::AppNotFound(id) => write!(f, "app not found: {id}"),
            Error::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Error::Storage(msg) => write!(f, "storage error: {msg}"),
        }
    }
}

impl std::error::Error for Error {}

/// Convenience alias used throughout the engine.
pub type Result<T> = std::result::Result<T, Error>;
