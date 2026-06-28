//! Shared capability ABI for Terrane built-in and external capability crates.

use std::any::Any;
use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

/// Identifier for a saved app. Caller-supplied and stable.
pub type AppId = String;

/// A command as it arrives at the core: a namespaced name like `"app.add"` plus
/// the caller's argument tokens.
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

/// A recorded event on the wire.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct EventRecord {
    pub kind: String,
    pub payload: Vec<u8>,
}

/// Typed errors. No panics on real paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    AppExists(AppId),
    AppNotFound(AppId),
    KeyNotFound(AppId, String),
    InvalidInput(String),
    Storage(String),
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

/// Convenience alias used throughout the engine and capability crates.
pub type Result<T> = std::result::Result<T, Error>;

/// What a command resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Commit(Vec<EventRecord>),
    Effect(Effect),
}

/// A side effect the engine must perform in the outside world.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    HttpGet {
        app: String,
        url: String,
    },
    ModelCall {
        app: String,
        agent: String,
        prompt: String,
    },
    GenerateAppWithHarness {
        draft_id: String,
        app_id: String,
        name: String,
        harness: String,
        prompt: String,
    },
    RunHarnessJs {
        run_id: String,
        app_id: String,
        harness: String,
        prompt: String,
    },
    NewReplicaId,
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

/// The namespace of a dotted name (`"app.add"` -> `"app"`).
pub fn namespace_of(name: &str) -> Result<&str> {
    name.split_once('.').map(|(ns, _)| ns).ok_or_else(|| {
        Error::InvalidInput(format!("command name must be 'namespace.verb': {name}"))
    })
}

/// A typed state store implemented by the host engine's aggregate state.
pub trait StateStore {
    fn get(&self, namespace: &str) -> Option<&dyn Any>;
    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any>;
}

pub fn state_ref<'a, T: 'static>(state: &'a dyn StateStore, namespace: &str) -> Result<&'a T> {
    state
        .get(namespace)
        .and_then(|slice| slice.downcast_ref::<T>())
        .ok_or_else(|| Error::Runtime(format!("missing or invalid {namespace} state slice")))
}

pub fn state_mut<'a, T: 'static>(
    state: &'a mut dyn StateStore,
    namespace: &str,
) -> Result<&'a mut T> {
    state
        .get_mut(namespace)
        .and_then(|slice| slice.downcast_mut::<T>())
        .ok_or_else(|| Error::Runtime(format!("missing or invalid {namespace} state slice")))
}

/// A command this capability owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
}

/// An event kind this capability emits/owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSpec {
    pub kind: &'static str,
}

/// A read-only query this capability exposes to other capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuerySpec {
    pub name: &'static str,
}

/// An event kind this capability reacts to without owning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPattern {
    pub kind: &'static str,
}

/// The declarative surface a capability exposes to the registry.
#[derive(Default)]
pub struct CapManifest {
    pub commands: Vec<CommandSpec>,
    pub events: Vec<EventSpec>,
    pub queries: Vec<QuerySpec>,
    pub resources: Vec<ResourceMethod>,
    pub subscriptions: Vec<EventPattern>,
}

impl CapManifest {
    pub fn empty() -> Self {
        Self::default()
    }
}

/// A read-only value returned by a capability query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryValue {
    Bool(bool),
    U64(Option<u64>),
}

/// Read-only access from one capability into another.
pub trait CapBus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue>;
}

/// Context handed to command decisions.
#[derive(Clone, Copy)]
pub struct CommandCtx<'a> {
    pub state: &'a dyn StateStore,
    pub bus: &'a dyn CapBus,
}

/// Context handed to read-only capability queries.
#[derive(Clone, Copy)]
pub struct QueryCtx<'a> {
    pub state: &'a dyn StateStore,
    pub bus: &'a dyn CapBus,
}

/// Context handed to backend resource reads.
#[derive(Clone, Copy)]
pub struct ResourceReadCtx<'a> {
    pub state: &'a dyn StateStore,
    pub bus: &'a dyn CapBus,
    pub app: &'a str,
}

/// A self-contained slice of engine behaviour.
pub trait Capability {
    fn namespace(&self) -> &'static str;

    fn manifest(&self) -> CapManifest {
        CapManifest::empty()
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision>;

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()>;

    fn describe(&self, record: &EventRecord) -> Option<String> {
        let _ = record;
        None
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        let _ = ctx;
        let _ = args;
        Err(Error::InvalidInput(format!("unknown query: {name}")))
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let _ = ctx;
        let _ = args;
        Err(Error::InvalidInput(format!(
            "unknown resource read: {}.{name}",
            self.namespace()
        )))
    }

    fn resource_api(&self) -> Vec<ResourceMethod> {
        self.manifest().resources
    }
}

/// A value a resource read hands back to backend JS.
pub enum ReadValue {
    OptString(Option<String>),
    StringMap(BTreeMap<String, String>),
    StringList(Vec<String>),
}

/// One method a capability exposes on `ctx.resource.<namespace>`.
pub enum ResourceMethod {
    Write {
        name: &'static str,
        params: &'static [&'static str],
    },
    Read {
        name: &'static str,
        params: &'static [&'static str],
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

pub fn app_exists(bus: &dyn CapBus, app: &str) -> Result<bool> {
    match bus.query("app", "exists", &[app.to_string()])? {
        QueryValue::Bool(value) => Ok(value),
        other => Err(Error::Runtime(format!(
            "app.exists returned unexpected value: {other:?}"
        ))),
    }
}

pub fn ensure_app_exists(bus: &dyn CapBus, app: &str) -> Result<()> {
    if app_exists(bus, app)? {
        Ok(())
    } else {
        Err(Error::AppNotFound(app.to_string()))
    }
}

pub fn replica_peer(bus: &dyn CapBus) -> Result<Option<u64>> {
    match bus.query("replica", "peer", &[])? {
        QueryValue::U64(peer) => Ok(peer),
        other => Err(Error::Runtime(format!(
            "replica.peer returned unexpected value: {other:?}"
        ))),
    }
}

/// Fetch a positional argument or fail with a clear message.
pub fn arg(args: &[String], index: usize, what: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| Error::InvalidInput(format!("missing {what}")))
}

pub fn extract_json_object<'a>(raw: &'a str, source: &str) -> Result<&'a str> {
    let trimmed = raw.trim();
    if trimmed.starts_with('{') && trimmed.ends_with('}') {
        return Ok(trimmed);
    }
    let start = raw
        .find('{')
        .ok_or_else(|| Error::InvalidInput(format!("{source} did not contain JSON")))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| Error::InvalidInput(format!("{source} did not contain complete JSON")))?;
    if end <= start {
        return Err(Error::InvalidInput(format!(
            "{source} JSON range is invalid"
        )));
    }
    Ok(&raw[start..=end])
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max).collect();
        format!("{head}...")
    }
}
