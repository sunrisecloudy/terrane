//! terrane-core — the deterministic, replayable engine.
//!
//! The single shape, now pluggable:
//!
//! ```text
//! Request ──registry──▶ capability.decide ──▶ Decision
//!   Commit([EventRecord]) ─┐
//!   Effect(e) ─runner─▶ [EventRecord] ─┴─▶ append to log ─▶ fold ─▶ State
//! ```
//!
//! There is no central command/event enum and no central match. Each
//! [`Capability`](cap::Capability) owns a namespace (`"app"`, `"kv"`, `"net"`,
//! …) and is wholly responsible for its commands, its events, deciding, and
//! folding. You add a command by writing/registering one capability — nothing
//! central changes except the [`default_registry`] line and (if it carries new
//! data) the aggregate [`State`].
//!
//! Dispatch is routed: a command `"app.add"` goes to the `app` capability.
//! Folding is *broadcast*: every recorded event is offered to every capability,
//! so a capability can react to another's events (e.g. `kv` clears an app's data
//! when it sees `"app.removed"`) without any capability knowing the others.
//!
//! ## Effects & determinism
//!
//! An effectful command's [`decide`](cap::Capability::decide) returns a
//! [`Decision::Effect`] describing the work; [`Core`] runs it through an injected
//! [`EffectRunner`] **once**, then records the result as an event. Replay never
//! runs the effect — it only folds the recorded event — so a non-deterministic
//! call (an HTTP GET) is reproduced from the log, not the network.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_domain::{Error, EventRecord, Request, Result};

pub mod cap;

use cap::{
    app::AppState, builder::BuilderState, crdt::CrdtState, harness::HarnessState, kv::KvState,
    model::ModelState, net::NetState, replica::ReplicaState, Capability,
};

/// The whole world the core holds: one slice per capability. Capabilities read
/// across slices (e.g. `kv` checks `state.app`) but each only writes its own.
/// Adding a capability with new data adds a field here.
///
/// Not `Eq`: the `crdt` slice compares by Loro deep value, which can hold floats
/// (`f64`), so only `PartialEq` is available — sufficient for the replay-identity
/// check and `assert_eq!`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub app: AppState,
    pub builder: BuilderState,
    pub harness: HarnessState,
    pub kv: KvState,
    pub net: NetState,
    pub model: ModelState,
    pub crdt: CrdtState,
    pub replica: ReplicaState,
}

/// What a Command resolves to. Pure commands commit Events immediately;
/// effectful commands name an [`Effect`] for the engine to run at the edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Commit(Vec<EventRecord>),
    Effect(Effect),
}

/// A side effect the engine must perform in the outside world. The runner turns
/// it into the Event(s) that get recorded.
///
/// Effects are still a small central enum (they live at the edge and grow slowly,
/// unlike commands). If they proliferate, make them name-tagged like events with
/// a runner registry — the same move we made for commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    HttpGet {
        app: String,
        url: String,
    },
    /// Ask an agent CLI (`claude`, `codex`) a prompt; its output is recorded.
    ModelCall {
        app: String,
        agent: String,
        prompt: String,
    },
    /// Ask a harness CLI to generate a Terrane app bundle draft.
    GenerateAppWithHarness {
        draft_id: String,
        app_id: String,
        name: String,
        harness: String,
        prompt: String,
    },
    /// Ask a harness CLI for JavaScript only, then run that JS once in the
    /// QuickJS backend sandbox with an explicit capability resource allowlist.
    RunHarnessJs {
        run_id: String,
        app_id: String,
        harness: String,
        prompt: String,
    },
    /// Mint this home's stable replica PeerID from OS entropy. The runner records
    /// it once as a `replica.initialized` event; replay reads it back.
    NewReplicaId,
}

/// Performs effects at the edge. Implementors do the real I/O (or, in tests, a
/// local I/O implementation) and return the recorded Event(s).
pub trait EffectRunner {
    fn run(&self, effect: &Effect, state: &State) -> Result<Vec<EventRecord>>;
}

/// A runner that performs no effects — the default for a core opened without
/// one. Asking it to run an effect is an error.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoEffects;

impl EffectRunner for NoEffects {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        Err(Error::InvalidInput(format!(
            "this core has no effect runner; cannot perform {effect:?}"
        )))
    }
}

/// Encode a capability's typed event into a name-tagged [`EventRecord`].
pub fn encode_event<E: BorshSerialize>(kind: &str, event: &E) -> Result<EventRecord> {
    let payload = borsh::to_vec(event).map_err(|e| Error::Storage(e.to_string()))?;
    Ok(EventRecord {
        kind: kind.to_string(),
        payload,
    })
}

/// Decode an [`EventRecord`]'s payload back into a capability's typed event.
pub fn decode_event<E: BorshDeserialize>(record: &EventRecord) -> Result<E> {
    borsh::from_slice::<E>(&record.payload)
        .map_err(|e| Error::Storage(format!("corrupt {} payload: {e}", record.kind)))
}

/// The namespace of a dotted name (`"app.add"` → `"app"`).
pub(crate) fn namespace_of(name: &str) -> Result<&str> {
    name.split_once('.').map(|(ns, _)| ns).ok_or_else(|| {
        Error::InvalidInput(format!("command name must be 'namespace.verb': {name}"))
    })
}

/// A table of capabilities keyed by namespace. Register one to plug in a whole
/// set of commands.
#[derive(Default)]
pub struct Registry {
    caps: BTreeMap<&'static str, Box<dyn Capability>>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    /// Plug a capability in. Its namespace must be unique.
    pub fn register(&mut self, capability: Box<dyn Capability>) {
        self.caps.insert(capability.namespace(), capability);
    }

    pub(crate) fn get(&self, namespace: &str) -> Result<&dyn Capability> {
        self.caps
            .get(namespace)
            .map(AsRef::as_ref)
            .ok_or_else(|| Error::InvalidInput(format!("unknown command namespace: {namespace}")))
    }
}

/// The registry every core opens with: the built-in capabilities.
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Box::new(cap::app::AppCapability));
    registry.register(Box::new(cap::build::BuildCapability));
    registry.register(Box::new(cap::builder::BuilderCapability));
    registry.register(Box::new(cap::harness::HarnessCapability));
    registry.register(Box::new(cap::kv::KvCapability));
    registry.register(Box::new(cap::crdt::CrdtCapability));
    registry.register(Box::new(cap::replica::ReplicaCapability));
    registry.register(Box::new(cap::net::NetCapability));
    registry.register(Box::new(cap::model::ModelCapability));
    registry.register(Box::new(cap::host::HostCapability));
    registry
}

/// Generate the `ctx.resource` reference (a per-namespace method table for every
/// capability that declares a backend surface) from the capabilities' own
/// `resource_api` declarations. This is the single source `docs/APP_API.md`'s
/// resource section is generated from — a test regenerates this and diffs the
/// doc, so the reference cannot drift from the runtime.
pub fn resource_api_markdown() -> String {
    use std::fmt::Write as _;
    let registry = default_registry();
    let mut out = String::new();
    for capability in registry.caps.values() {
        let api = capability.resource_api();
        if api.is_empty() {
            continue;
        }
        let ns = capability.namespace();
        let _ = writeln!(out, "#### `ctx.resource.{ns}`\n");
        let _ = writeln!(out, "| Method | Kind |");
        let _ = writeln!(out, "| --- | --- |");
        for method in &api {
            let _ = writeln!(
                out,
                "| `ctx.resource.{ns}.{}({})` | {} |",
                method.name(),
                method.params().join(", "),
                method.kind()
            );
        }
        let _ = writeln!(out);
    }
    out.trim_end().to_string()
}

/// The full set of `ctx.resource.<ns>.<method>` the capabilities declare — used
/// to assert the live runtime installs exactly this surface and no more.
pub fn declared_resource_surface() -> BTreeSet<String> {
    let registry = default_registry();
    let mut out = BTreeSet::new();
    for capability in registry.caps.values() {
        let ns = capability.namespace();
        for method in capability.resource_api() {
            out.insert(format!("ctx.resource.{ns}.{}", method.name()));
        }
    }
    out
}

/// One method of a capability's backend `ctx.resource` surface, as structured
/// data (for the public-contract export).
pub struct ResourceMethodSurface {
    pub name: &'static str,
    pub kind: &'static str,
    pub params: &'static [&'static str],
}

/// A capability's declared `ctx.resource.<namespace>` surface.
pub struct ResourceNamespaceSurface {
    pub namespace: &'static str,
    pub methods: Vec<ResourceMethodSurface>,
}

/// The full `ctx.resource` surface as structured data — every capability that
/// declares a backend API, with its methods. The structured twin of
/// [`resource_api_markdown`], for emitting the public contract.
pub fn resource_surface() -> Vec<ResourceNamespaceSurface> {
    let registry = default_registry();
    let mut out = Vec::new();
    for capability in registry.caps.values() {
        let api = capability.resource_api();
        if api.is_empty() {
            continue;
        }
        out.push(ResourceNamespaceSurface {
            namespace: capability.namespace(),
            methods: api
                .iter()
                .map(|m| ResourceMethodSurface {
                    name: m.name(),
                    kind: m.kind(),
                    params: m.params(),
                })
                .collect(),
        });
    }
    out
}

/// Every registered capability namespace (`app`, `kv`, `crdt`, …), sorted.
pub fn capability_namespaces() -> Vec<&'static str> {
    default_registry()
        .caps
        .values()
        .map(|c| c.namespace())
        .collect()
}

/// Offer one recorded event to every capability (broadcast fold).
pub(crate) fn apply(registry: &Registry, state: &mut State, record: &EventRecord) -> Result<()> {
    for capability in registry.caps.values() {
        capability.fold(state, record)?;
    }
    Ok(())
}

/// Fold records into a caller-owned State without appending them to any log.
/// Preview and test surfaces use this after a memory-backed backend run.
pub fn fold_records_in_memory(state: &mut State, records: &[EventRecord]) -> Result<()> {
    let registry = default_registry();
    for record in records {
        apply(&registry, state, record)?;
    }
    Ok(())
}

/// Read every [`EventRecord`] from the log, in order. The log is a flat sequence
/// of length-prefixed borsh records: a little-endian `u32` byte length followed
/// by that many bytes of one borsh-encoded `EventRecord`. A missing log is an
/// empty history, not an error.
pub fn read_log(log_path: &Path) -> Result<Vec<EventRecord>> {
    let mut records = Vec::new();
    let file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(records),
        Err(e) => return Err(Error::Storage(e.to_string())),
    };
    let mut reader = BufReader::new(file);
    'records: loop {
        let mut len_buf = [0u8; 4];
        let mut got = 0usize;
        while got < len_buf.len() {
            match reader.read(&mut len_buf[got..]) {
                Ok(0) if got == 0 => break 'records,
                Ok(0) => {
                    return Err(Error::Storage(format!(
                        "truncated log record length: got {got} of 4 bytes"
                    )));
                }
                Ok(n) => got += n,
                Err(e) => return Err(Error::Storage(e.to_string())),
            }
        }
        let len = u32::from_le_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        reader
            .read_exact(&mut buf)
            .map_err(|e| Error::Storage(format!("truncated log record: {e}")))?;
        let record = borsh::from_slice::<EventRecord>(&buf)
            .map_err(|e| Error::Storage(format!("corrupt log record: {e}")))?;
        records.push(record);
    }
    Ok(records)
}

/// The engine: an on-disk event log, the State folded from it, an injected
/// [`EffectRunner`], and the [`Registry`] of capabilities. Pure usage leaves the
/// runner untouched, so it defaults to [`NoEffects`].
pub struct Core<R: EffectRunner = NoEffects> {
    log_path: PathBuf,
    state: State,
    runner: R,
    registry: Registry,
    /// String printed by the most recent `host.run` backend, if any. Not part of
    /// State, never logged or replayed — purely a transport for the host to print.
    last_output: Option<String>,
}

impl Core<NoEffects> {
    /// Open (or create) a pure core at `log_path` — no effects enabled.
    pub fn open(log_path: impl Into<PathBuf>) -> Result<Self> {
        Core::open_with(log_path, NoEffects)
    }
}

impl<R: EffectRunner> Core<R> {
    /// Open (or create) a core at `log_path` with an effect runner, rebuilding
    /// State by folding the existing log through the default registry.
    pub fn open_with(log_path: impl Into<PathBuf>, runner: R) -> Result<Self> {
        let log_path = log_path.into();
        let registry = default_registry();
        let mut state = State::default();
        for record in read_log(&log_path)? {
            apply(&registry, &mut state, &record)?;
        }
        Ok(Core {
            log_path,
            state,
            runner,
            registry,
            last_output: None,
        })
    }

    /// The current world. Reads go through here.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Run a command end to end: route to its capability, decide, then commit
    /// events (running an effect first if the decision calls for one). Nothing is
    /// written unless the command succeeds.
    pub fn dispatch(&mut self, request: Request) -> Result<Vec<EventRecord>> {
        let namespace = namespace_of(&request.name)?;

        // `host.run` re-dispatches the backend's kv.* writes and therefore needs
        // &mut self, which a pure decide (&State) cannot have. Every other command
        // goes through the unchanged decide -> commit path.
        if request.name == "host.run" {
            return self.run_backend(&request.args);
        }

        let decision =
            self.registry
                .get(namespace)?
                .decide(&self.state, &request.name, &request.args)?;
        match decision {
            Decision::Commit(records) => self.commit(records),
            Decision::Effect(effect) => {
                let records = self.runner.run(&effect, &self.state)?;
                self.commit(records)
            }
        }
    }

    /// Execute `host.run app [input…]`: load the app's bundle, run its JS backend
    /// in QuickJS over a sandboxed app-scoped `ctx.resource`, commit the `kv.*`
    /// records it produced (so the global log holds only ordinary events), and
    /// stash the backend's printed string for [`take_last_output`](Self::take_last_output).
    /// JS runs once here, never on replay.
    fn run_backend(&mut self, args: &[String]) -> Result<Vec<EventRecord>> {
        // Reset first, so take_last_output() reflects only this attempt — a
        // failed run leaves no stale output from a previous successful one.
        self.last_output = None;
        let app = cap::arg(args, 0, "app")?;
        let input: Vec<String> = args.get(1..).unwrap_or_default().to_vec();

        let source = self
            .state
            .app
            .apps
            .get(&app)
            .ok_or_else(|| Error::AppNotFound(app.clone()))?
            .source
            .clone()
            .ok_or_else(|| Error::Runtime(format!("app {app} has no --source bundle")))?;

        let result = cap::host::run(&app, &input, &source, self.state.clone())?;
        let records = self.commit(result.records)?;
        self.last_output = Some(result.output);
        Ok(records)
    }

    /// Take the string printed by the most recent `host.run` (if any). Not part
    /// of State; never logged or replayed.
    pub fn take_last_output(&mut self) -> Option<String> {
        self.last_output.take()
    }

    /// Human-readable lines for the event log, asking each event's owning
    /// capability to describe it (falling back to the raw kind + size).
    pub fn log_lines(&self) -> Result<Vec<String>> {
        let mut lines = Vec::new();
        for record in read_log(&self.log_path)? {
            let described = namespace_of(&record.kind)
                .ok()
                .and_then(|ns| self.registry.get(ns).ok())
                .and_then(|cap| cap.describe(&record));
            lines
                .push(described.unwrap_or_else(|| {
                    format!("{} ({} bytes)", record.kind, record.payload.len())
                }));
        }
        Ok(lines)
    }

    /// True if replaying the log reproduces the in-memory State — the
    /// determinism contract, checkable at any time.
    pub fn replay_matches(&self) -> Result<bool> {
        let mut fresh = State::default();
        for record in read_log(&self.log_path)? {
            apply(&self.registry, &mut fresh, &record)?;
        }
        Ok(fresh == self.state)
    }

    /// Persist records to the log, then fold them into State.
    fn commit(&mut self, records: Vec<EventRecord>) -> Result<Vec<EventRecord>> {
        self.append(&records)?;
        for record in &records {
            apply(&self.registry, &mut self.state, record)?;
        }
        Ok(records)
    }

    fn append(&self, records: &[EventRecord]) -> Result<()> {
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
        for record in records {
            let bytes = borsh::to_vec(record).map_err(|e| Error::Storage(e.to_string()))?;
            let len = u32::try_from(bytes.len())
                .map_err(|_| Error::Storage("event record too large".into()))?;
            file.write_all(&len.to_le_bytes())
                .map_err(|e| Error::Storage(e.to_string()))?;
            file.write_all(&bytes)
                .map_err(|e| Error::Storage(e.to_string()))?;
        }
        Ok(())
    }
}
