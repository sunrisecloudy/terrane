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
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use terrane_domain::{AppRecord, Command, Error, Event, Result, State};

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

/// Rebuild State by folding every Event in the log from empty. A missing log is
/// an empty world, not an error.
pub fn replay(log_path: &Path) -> Result<State> {
    let mut state = State::default();
    let file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(state),
        Err(e) => return Err(Error::Storage(e.to_string())),
    };
    for (i, line) in BufReader::new(file).lines().enumerate() {
        let line = line.map_err(|e| Error::Storage(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Event = serde_json::from_str(&line)
            .map_err(|e| Error::Storage(format!("corrupt log entry on line {}: {e}", i + 1)))?;
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
                std::fs::create_dir_all(parent).map_err(|e| Error::Storage(e.to_string()))?;
            }
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|e| Error::Storage(e.to_string()))?;
        for event in events {
            let line =
                serde_json::to_string(event).map_err(|e| Error::Storage(e.to_string()))?;
            writeln!(file, "{line}").map_err(|e| Error::Storage(e.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn add(id: &str, name: &str) -> Command {
        Command::AddApp {
            id: id.into(),
            name: name.into(),
            source: None,
        }
    }

    #[test]
    fn executes_and_replays_identically() {
        let dir = tempdir().unwrap();
        let log = dir.path().join("log.jsonl");

        let mut core = Core::open(&log).unwrap();
        core.execute(add("notes", "Notes")).unwrap();
        core.execute(add("tasks", "Task Workbench")).unwrap();
        core.execute(Command::RemoveApp { id: "notes".into() }).unwrap();

        // The in-memory State must equal a fresh replay of the log.
        assert!(core.replay_matches().unwrap());
        let replayed = replay(&log).unwrap();
        assert_eq!(replayed.apps.len(), 1);
        assert!(replayed.apps.contains_key("tasks"));

        // A brand-new Core opened on the same log rebuilds the same world.
        let reopened = Core::open(&log).unwrap();
        assert_eq!(reopened.state(), &replayed);
    }

    #[test]
    fn source_round_trips_through_the_log() {
        let dir = tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let mut core = Core::open(&log).unwrap();
        core.execute(Command::AddApp {
            id: "notes".into(),
            name: "Notes".into(),
            source: Some("apps/notes".into()),
        })
        .unwrap();
        let reopened = Core::open(&log).unwrap();
        assert_eq!(
            reopened.state().apps["notes"].source.as_deref(),
            Some("apps/notes")
        );
    }

    #[test]
    fn rejects_duplicate_and_missing() {
        let dir = tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let mut core = Core::open(&log).unwrap();

        core.execute(add("notes", "Notes")).unwrap();
        assert_eq!(
            core.execute(add("notes", "Notes Again")),
            Err(Error::AppExists("notes".into()))
        );
        assert_eq!(
            core.execute(Command::RemoveApp { id: "ghost".into() }),
            Err(Error::AppNotFound("ghost".into()))
        );

        // Rejected commands wrote nothing: still exactly one app.
        assert_eq!(core.state().apps.len(), 1);
        assert!(core.replay_matches().unwrap());
    }

    #[test]
    fn kv_resource_records_and_cascades() {
        let dir = tempdir().unwrap();
        let log = dir.path().join("log.jsonl");
        let mut core = Core::open(&log).unwrap();
        core.execute(add("notes", "Notes")).unwrap();

        // Writing to an app that doesn't exist is rejected.
        assert_eq!(
            core.execute(Command::KvSet {
                app: "ghost".into(),
                key: "k".into(),
                value: "v".into()
            }),
            Err(Error::AppNotFound("ghost".into()))
        );

        core.execute(Command::KvSet {
            app: "notes".into(),
            key: "theme".into(),
            value: "dark".into(),
        })
        .unwrap();
        assert_eq!(core.state().data["notes"]["theme"], "dark");
        assert!(core.replay_matches().unwrap());

        // Deleting a missing key errors; deleting a present key works.
        assert_eq!(
            core.execute(Command::KvDelete {
                app: "notes".into(),
                key: "ghost".into()
            }),
            Err(Error::KeyNotFound("notes".into(), "ghost".into()))
        );

        // Removing the app cascades: its data is gone from a fresh replay too.
        core.execute(Command::KvSet {
            app: "notes".into(),
            key: "lang".into(),
            value: "en".into(),
        })
        .unwrap();
        core.execute(Command::RemoveApp { id: "notes".into() })
            .unwrap();
        assert!(core.state().data.is_empty());
        assert!(replay(&log).unwrap().data.is_empty());
    }

    #[test]
    fn rejects_empty_fields() {
        let dir = tempdir().unwrap();
        let mut core = Core::open(dir.path().join("log.jsonl")).unwrap();
        assert!(matches!(
            core.execute(add("", "x")),
            Err(Error::InvalidInput(_))
        ));
        assert!(matches!(
            core.execute(add("x", "")),
            Err(Error::InvalidInput(_))
        ));
    }
}
