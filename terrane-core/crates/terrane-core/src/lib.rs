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
//! *core*.
//!
//! ## Effects
//!
//! Some commands need the outside world (the network, later a model). For those,
//! [`decide`] stays pure: it returns a [`Decision::Effect`] *describing* the work
//! instead of inventing a result. [`Core`] runs that effect through an injected
//! [`EffectRunner`] **once, at execute time**, and records the runner's result
//! as an Event. Replay never runs the effect — it only folds the recorded Event.
//! So a non-deterministic call (an HTTP GET) becomes deterministic on replay,
//! because the body is read from the log, not the network.

use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use terrane_domain::{AppRecord, Command, Error, Event, FetchResponse, Result, State};

/// Map any I/O error into a domain `Storage` error.
fn storage_err(e: std::io::Error) -> Error {
    Error::Storage(e.to_string())
}

/// What a Command resolves to. Pure commands [`Commit`](Decision::Commit) their
/// Events immediately; effectful commands name an [`Effect`](Decision::Effect)
/// for the engine to run at the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Commit(Vec<Event>),
    Effect(Effect),
}

/// A side effect the engine must perform in the outside world. The runner turns
/// it into the Event(s) that get recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    HttpGet { app: String, url: String },
}

/// Performs effects at the edge. Implementors do the real I/O (or, in tests, a
/// deterministic fake) and return the Event(s) that record the outcome.
pub trait EffectRunner {
    fn run(&self, effect: &Effect) -> Result<Vec<Event>>;
}

/// A runner that performs no effects — the default for a core opened without one
/// (pure catalog/kv usage). Asking it to run an effect is an error.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEffects;

impl EffectRunner for NoEffects {
    fn run(&self, effect: &Effect) -> Result<Vec<Event>> {
        Err(Error::InvalidInput(format!(
            "this core has no effect runner; cannot perform {effect:?}"
        )))
    }
}

/// Decide what a Command resolves to, validating it against current State.
/// Pure: same `(state, cmd)` always yields the same result, and it never
/// performs the effect itself — it only *names* one.
pub fn decide(state: &State, cmd: &Command) -> Result<Decision> {
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
            Ok(Decision::Commit(vec![Event::AppAdded {
                id: id.clone(),
                name: name.clone(),
                source: source.clone(),
            }]))
        }
        Command::RemoveApp { id } => {
            if !state.apps.contains_key(id) {
                return Err(Error::AppNotFound(id.clone()));
            }
            Ok(Decision::Commit(vec![Event::AppRemoved { id: id.clone() }]))
        }
        Command::KvSet { app, key, value } => {
            if !state.apps.contains_key(app) {
                return Err(Error::AppNotFound(app.clone()));
            }
            if key.trim().is_empty() {
                return Err(Error::InvalidInput("key must not be empty".into()));
            }
            Ok(Decision::Commit(vec![Event::KvSet {
                app: app.clone(),
                key: key.clone(),
                value: value.clone(),
            }]))
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
            Ok(Decision::Commit(vec![Event::KvDeleted {
                app: app.clone(),
                key: key.clone(),
            }]))
        }
        Command::Fetch { app, url } => {
            // Validate purely; the result is produced by the runner at the edge.
            if !state.apps.contains_key(app) {
                return Err(Error::AppNotFound(app.clone()));
            }
            if url.trim().is_empty() {
                return Err(Error::InvalidInput("url must not be empty".into()));
            }
            Ok(Decision::Effect(Effect::HttpGet {
                app: app.clone(),
                url: url.clone(),
            }))
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
            // Removing an app cascades to all of its resources.
            state.data.remove(id);
            state.fetches.remove(id);
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
        Event::Fetched {
            app,
            url,
            status,
            body,
        } => {
            state.fetches.entry(app.clone()).or_default().insert(
                url.clone(),
                FetchResponse {
                    status: *status,
                    body: body.clone(),
                },
            );
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

/// The engine: an on-disk event log, the State folded from it, and an
/// [`EffectRunner`] used to perform effectful commands at the edge. Pure usage
/// (catalog/kv) leaves the runner untouched, so it defaults to [`NoEffects`].
pub struct Core<R: EffectRunner = NoEffects> {
    log_path: PathBuf,
    state: State,
    runner: R,
}

impl Core<NoEffects> {
    /// Open (or create) a pure core at `log_path` — no effects enabled.
    pub fn open(log_path: impl Into<PathBuf>) -> Result<Self> {
        Core::open_with(log_path, NoEffects)
    }
}

impl<R: EffectRunner> Core<R> {
    /// Open (or create) a core at `log_path` with an effect runner, rebuilding
    /// State from any existing log. The parent directory is created on first
    /// write, not here.
    pub fn open_with(log_path: impl Into<PathBuf>, runner: R) -> Result<Self> {
        let log_path = log_path.into();
        let state = replay(&log_path)?;
        Ok(Core {
            log_path,
            state,
            runner,
        })
    }

    /// The current world. Reads (`list`, `show`) go through here.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Run a Command end to end. Pure commands commit their Events directly;
    /// effectful commands run their effect through the runner *once*, then commit
    /// the recorded result. Nothing is written unless the command succeeds, so a
    /// rejected command leaves both the log and the State untouched.
    pub fn execute(&mut self, cmd: Command) -> Result<Vec<Event>> {
        match decide(&self.state, &cmd)? {
            Decision::Commit(events) => self.commit(events),
            Decision::Effect(effect) => {
                let events = self.runner.run(&effect)?;
                self.commit(events)
            }
        }
    }

    /// True if replaying the log from disk reproduces the in-memory State. This
    /// is the determinism contract, checkable at any time.
    pub fn replay_matches(&self) -> Result<bool> {
        Ok(replay(&self.log_path)? == self.state)
    }

    /// Persist Events to the log, then fold them into State.
    fn commit(&mut self, events: Vec<Event>) -> Result<Vec<Event>> {
        self.append(&events)?;
        for event in &events {
            fold(&mut self.state, event);
        }
        Ok(events)
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
