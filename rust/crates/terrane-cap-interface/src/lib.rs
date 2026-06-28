//! Shared capability ABI for Terrane built-in and external capability crates.

use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

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
    Runtime(RuntimeRequest),
}

/// A request for a runtime capability to execute an app backend once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeRequest {
    pub app: String,
    pub input: Vec<String>,
}

/// The non-record output of one runtime execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOutput {
    pub output: String,
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

/// Canonical capability documentation. Edge surfaces render this into MCP
/// detail, CLI help, generated skills, and public contract docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDoc {
    pub namespace: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub version: String,
    pub audience: Vec<String>,
    pub manifest: CapabilityManifestDoc,
    pub resources: Vec<ResourceDoc>,
    pub schemas: Vec<SchemaDoc>,
    pub examples: Vec<ExampleDoc>,
    pub constraints: Vec<String>,
    pub limits: Vec<LimitDoc>,
    pub compatibility: Vec<String>,
    pub internal: Vec<InternalNote>,
}

impl CapabilityDoc {
    pub fn from_manifest(namespace: &str, manifest: CapManifest, include_internal: bool) -> Self {
        let resource_methods: Vec<ResourceMethodDoc> = manifest
            .resources
            .iter()
            .map(ResourceMethodDoc::from_resource_method)
            .collect();
        let resources = if resource_methods.is_empty() {
            Vec::new()
        } else {
            vec![ResourceDoc {
                namespace: namespace.to_string(),
                summary: format!("Backend resource surface for `{namespace}`."),
                methods: resource_methods.clone(),
            }]
        };
        Self {
            namespace: namespace.to_string(),
            title: namespace.to_string(),
            summary: format!("Capability namespace `{namespace}`."),
            status: "stable".to_string(),
            version: "0.1.0".to_string(),
            audience: vec![
                "app-author".to_string(),
                "agent".to_string(),
                "host-implementer".to_string(),
            ],
            manifest: CapabilityManifestDoc {
                commands: manifest
                    .commands
                    .iter()
                    .map(|command| command.name.to_string())
                    .collect(),
                queries: manifest
                    .queries
                    .iter()
                    .map(|query| query.name.to_string())
                    .collect(),
                events: manifest
                    .events
                    .iter()
                    .map(|event| event.kind.to_string())
                    .collect(),
                subscriptions: manifest
                    .subscriptions
                    .iter()
                    .map(|subscription| subscription.kind.to_string())
                    .collect(),
                resource_methods,
            },
            resources,
            schemas: Vec::new(),
            examples: Vec::new(),
            constraints: Vec::new(),
            limits: Vec::new(),
            compatibility: Vec::new(),
            internal: if include_internal {
                vec![InternalNote {
                    title: "Generated from manifest".to_string(),
                    body: "This fallback doc was generated from Capability::manifest()."
                        .to_string(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    pub fn without_internal(mut self) -> Self {
        self.internal.clear();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityManifestDoc {
    pub commands: Vec<String>,
    pub queries: Vec<String>,
    pub events: Vec<String>,
    pub subscriptions: Vec<String>,
    pub resource_methods: Vec<ResourceMethodDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDoc {
    pub namespace: String,
    pub summary: String,
    pub methods: Vec<ResourceMethodDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMethodDoc {
    pub name: String,
    pub kind: String,
    pub params: Vec<ParamDoc>,
    pub returns: String,
    pub summary: String,
    pub errors: Vec<String>,
}

impl ResourceMethodDoc {
    fn from_resource_method(method: &ResourceMethod) -> Self {
        Self {
            name: method.name().to_string(),
            kind: method.kind().to_string(),
            params: method
                .params()
                .iter()
                .map(|name| ParamDoc {
                    name: (*name).to_string(),
                    summary: String::new(),
                    required: true,
                    schema_ref: String::new(),
                })
                .collect(),
            returns: String::new(),
            summary: format!("{} resource method `{}`.", method.kind(), method.name()),
            errors: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDoc {
    pub name: String,
    pub summary: String,
    pub required: bool,
    pub schema_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDoc {
    pub id: String,
    pub title: String,
    pub media_type: String,
    pub schema_json: String,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleDoc {
    pub title: String,
    pub summary: String,
    pub language: String,
    pub code: String,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitDoc {
    pub name: String,
    pub value: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalNote {
    pub title: String,
    pub body: String,
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

/// A runtime engine's controlled access to Terrane resources.
pub trait RuntimeHost {
    fn resource_methods(&self, namespace: &str) -> Result<Vec<ResourceMethod>>;

    fn read_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue>;

    fn write_resource(&mut self, namespace: &str, method: &str, args: &[String]) -> Result<()>;

    fn take_records(&mut self) -> Vec<EventRecord>;
}

/// Shareable runtime host handle. Runtime engines capture this inside guest-code
/// callbacks while core keeps ownership of commit/replay.
#[derive(Clone)]
pub struct RuntimeHostHandle {
    inner: Rc<RefCell<Box<dyn RuntimeHost>>>,
}

impl RuntimeHostHandle {
    pub fn new(host: Box<dyn RuntimeHost>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(host)),
        }
    }

    pub fn resource_methods(&self, namespace: &str) -> Result<Vec<ResourceMethod>> {
        self.inner.borrow().resource_methods(namespace)
    }

    pub fn read_resource(
        &self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        self.inner
            .borrow_mut()
            .read_resource(namespace, method, args)
    }

    pub fn write_resource(&self, namespace: &str, method: &str, args: &[String]) -> Result<()> {
        self.inner
            .borrow_mut()
            .write_resource(namespace, method, args)
    }

    pub fn take_records(&self) -> Vec<EventRecord> {
        self.inner.borrow_mut().take_records()
    }
}

/// Context handed to runtime capabilities.
#[derive(Clone)]
pub struct RuntimeCtx {
    pub source: String,
    pub app_name: String,
    pub host: RuntimeHostHandle,
}

/// A self-contained slice of engine behaviour.
pub trait Capability {
    fn namespace(&self) -> &'static str;

    fn manifest(&self) -> CapManifest {
        CapManifest::empty()
    }

    fn doc(&self, include_internal: bool) -> CapabilityDoc {
        CapabilityDoc::from_manifest(self.namespace(), self.manifest(), include_internal)
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

    fn run_runtime(&self, ctx: RuntimeCtx, request: RuntimeRequest) -> Result<RuntimeOutput> {
        let _ = ctx;
        let _ = request;
        Err(Error::InvalidInput(format!(
            "{} is not a runtime capability",
            self.namespace()
        )))
    }
}

/// A value a resource read hands back to backend JS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadValue {
    OptString(Option<String>),
    StringMap(BTreeMap<String, String>),
    StringList(Vec<String>),
}

/// One method a capability exposes on `ctx.resource.<namespace>`.
#[derive(Debug, Clone, PartialEq, Eq)]
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

#[cfg(test)]
mod tests;
