//! terrane-core — the deterministic, replayable engine.
//!
//! The single shape:
//!
//! ```text
//! Command ──decide──▶ [Event] ──append──▶ log   ──fold──▶ State
//! ```
//!
//! - [`decide`] is pure: given the current State and a Command, it returns the
//!   Events that should happen, or a typed error. No I/O, no clock, no rng.
//! - [`fold`] is pure: it applies one Event to State.
//! - [`Core`] is the thin effectful shell: it owns the on-disk event log,
//!   appends Events as they happen, and rebuilds State by [`replay`]ing the log.
//!
//! Because State is *only ever* produced by folding Events, replaying the log
//! from empty reproduces identical State — the property that earns the word
//! *core*. Effects (the Resources layer) will be mediated and recorded here too,
//! so that replay stays deterministic even when the resources are not.

use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use terrane_domain::{AppRecord, Command, Error, Event, Result, State};

/// Map any I/O error into a domain `Storage` error.
fn storage_err(e: std::io::Error) -> Error {
    Error::Storage(e.to_string())
}

/// Decide what Events a Command produces, validating it against current State.
/// Pure: same `(state, cmd)` always yields the same result.
pub fn decide(state: &State, cmd: &Command) -> Result<Vec<Event>> {
    match cmd {
        Command::AddApp { id, name, source } => {
            if id.trim().is_empty() {
                return Err(Error::InvalidInput("app id must not be empty".into()));
            }
            if name.trim().is_empty() {
                return Err(Error::InvalidInput("app name must not be empty".into()));
            }
            if state.apps.contains_key(id) {
                return Err(Error::AppExists(id.clone()));
            }
            Ok(vec![Event::AppAdded {
                id: id.clone(),
                name: name.clone(),
                source: source.clone(),
            }])
        }
        Command::RemoveApp { id } => {
            if !state.apps.contains_key(id) {
                return Err(Error::AppNotFound(id.clone()));
            }
            Ok(vec![Event::AppRemoved { id: id.clone() }])
        }
        Command::KvSet { app, key, value } => {
            if !state.apps.contains_key(app) {
                return Err(Error::AppNotFound(app.clone()));
            }
            if key.trim().is_empty() {
                return Err(Error::InvalidInput("key must not be empty".into()));
            }
            Ok(vec![Event::KvSet {
                app: app.clone(),
                key: key.clone(),
                value: value.clone(),
            }])
        }
        Command::KvDelete { app, key } => {
            let missing = state
                .data
                .get(app)
                .map(|kv| !kv.contains_key(key))
                .unwrap_or(true);
            if missing {
                return Err(Error::KeyNotFound(app.clone(), key.clone()));
            }
            Ok(vec![Event::KvDeleted {
                app: app.clone(),
                key: key.clone(),
            }])
        }
    }
}

/// Apply one Event to State. Pure and total — Events are already-true facts.
pub fn fold(state: &mut State, event: &Event) {
    match event {
        Event::AppAdded { id, name, source } => {
            state.apps.insert(
                id.clone(),
                AppRecord {
                    id: id.clone(),
                    name: name.clone(),
                    source: source.clone(),
                },
            );
        }
        Event::AppRemoved { id } => {
            state.apps.remove(id);
            // Removing an app cascades to its key/value resource.
            state.data.remove(id);
        }
        Event::KvSet { app, key, value } => {
            state
                .data
                .entry(app.clone())
                .or_default()
                .insert(key.clone(), value.clone());
        }
        Event::KvDeleted { app, key } => {
            if let Some(kv) = state.data.get_mut(app) {
                kv.remove(key);
                if kv.is_empty() {
                    state.data.remove(app);
                }
            }
        }
    }
}

/// Read every Event from the log, in order. The log is a flat sequence of
/// length-prefixed borsh records: a little-endian `u32` byte length followed by
/// that many bytes of one borsh-encoded [`Event`]. A missing log is an empty
/// history, not an error.
pub fn read_log(log_path: &Path) -> Result<Vec<Event>> {
    let mut events = Vec::new();
    let file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(events),
        Err(e) => return Err(storage_err(e)),
    };
    let mut reader = BufReader::new(file);
    loop {
        let mut len_buf = [0u8; 4];
        match reader.read_exact(&mut len_buf) {
            Ok(()) => {}
            // A clean EOF at a record boundary is the end of the log.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(storage_err(e)),
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| Error::Storage(format!("truncated log record: {e}")))?;
        let event = borsh::from_slice::<Event>(&buf)
            .map_err(|e| Error::Storage(format!("corrupt log record: {e}")))?;
        events.push(event);
    }
    Ok(events)
}

/// Rebuild State by folding every Event in the log from empty. A missing log is
/// an empty world, not an error.
pub fn replay(log_path: &Path) -> Result<State> {
    let mut state = State::default();
    for event in read_log(log_path)? {
        fold(&mut state, &event);
    }
    Ok(state)
}

/// The engine: an on-disk event log plus the State folded from it.
pub struct Core {
    log_path: PathBuf,
    state: State,
}

impl Core {
    /// Open (or create) a core at `log_path`, rebuilding State from any existing
    /// log. The parent directory is created on first write, not here.
    pub fn open(log_path: impl Into<PathBuf>) -> Result<Self> {
        let log_path = log_path.into();
        let state = replay(&log_path)?;
        Ok(Core { log_path, state })
    }

    /// The current world. Reads (`list`, `show`) go through here.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Run a Command end to end: decide → persist Events → fold into State.
    /// Nothing is written unless `decide` succeeds, so a rejected Command leaves
    /// both the log and the State untouched.
    pub fn execute(&mut self, cmd: Command) -> Result<Vec<Event>> {
        let events = decide(&self.state, &cmd)?;
        self.append(&events)?;
        for event in &events {
            fold(&mut self.state, event);
        }
        Ok(events)
    }

    /// True if replaying the log from disk reproduces the in-memory State. This
    /// is the determinism contract, checkable at any time.
    pub fn replay_matches(&self) -> Result<bool> {
        Ok(replay(&self.log_path)? == self.state)
    }

    fn append(&self, events: &[Event]) -> Result<()> {
        if let Some(parent) = self.log_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(storage_err)?;
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .map_err(storage_err)?;
        for event in events {
            let bytes = borsh::to_vec(event).map_err(storage_err)?;
            let len = u32::try_from(bytes.len())
                .map_err(|_| Error::Storage("event record too large".into()))?;
            file.write_all(&len.to_le_bytes()).map_err(storage_err)?;
            file.write_all(&bytes).map_err(storage_err)?;
        }
        Ok(())
    }
}
