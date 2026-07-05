//! terrane-core — the deterministic, replayable engine.
//!
//! The single shape, now pluggable:
//!
//! ```text
//! Request ──registry──▶ capability.decide ──▶ Decision
//!   Commit([EventRecord]) ─┐
//!   Effect(e) ─runner─▶ [EventRecord] ─┤
//!   Runtime(r) ─runtime cap─▶ [EventRecord] ─┴─▶ append to log ─▶ fold ─▶ State
//! ```
//!
//! There is no central command/event enum and no central match. Each
//! [`Capability`] owns a namespace (`"app"`, `"kv"`, `"net"`,
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
//! An effectful command's [`decide`](Capability::decide) returns a
//! [`Decision::Effect`] describing the work; [`Core`] runs it through an injected
//! [`EffectRunner`] **once**, then records the result as an event. Replay never
//! runs the effect — it only folds the recorded event — so a non-deterministic
//! call (an HTTP GET) is reproduced from the log, not the network.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use borsh::{BorshDeserialize, BorshSerialize};

pub mod domain;
pub mod filelock;
mod planned_docs;
pub use terrane_cap_interface::{
    arg, decode_event, encode_event, namespace_of, AppId, CapBus, Capability, CapabilityDoc,
    CapabilityManifestDoc, CommandAuthority, CommandCtx, Decision, Effect, Error, EventRecord,
    ExampleDoc, ExecutionPrincipal, GrantResourceSpec, InternalNote, LimitDoc, LiveHost, ParamDoc,
    QueryCtx, QueryValue, ReadValue, RecordedCallCap, Request, ResourceDoc, ResourceMethod,
    ResourceMethodDoc, ResourceReadCtx, Result, RuntimeCtx, RuntimeHost, RuntimeHostHandle,
    RuntimeOutput, RuntimeRequest, SchemaDoc, StateStore, LOCAL_ORG, LOCAL_OWNER_SUBJECT,
    LOCAL_SOURCE, NAMESPACE_SELECTOR_SCHEMA_ID,
};

const LOG_HEADER: &[u8] = b"TRNLOG\x01\n";
const PRE_ACTOR_BACKUP_NAME: &str = "log.bin.pre-actor";

#[derive(BorshSerialize, BorshDeserialize)]
struct LegacyEventRecord {
    kind: String,
    payload: Vec<u8>,
}

use terrane_cap_agent::AgentState;
use terrane_cap_app::AppState;
use terrane_cap_auth::AuthState;
use terrane_cap_builder::BuilderState;
use terrane_cap_crdt::CrdtState;
use terrane_cap_harness::HarnessState;
use terrane_cap_kv::{KvState, KvStoragePlan};
use terrane_cap_local_model::LocalModelState;
use terrane_cap_model::ModelState;
use terrane_cap_native::NativeState;
use terrane_cap_net::NetState;
use terrane_cap_replica::ReplicaState;
use terrane_cap_scheduler::SchedulerState;
use terrane_cap_stt::SttState;
use terrane_cap_time::TimeState;

/// The whole world the core holds: one slice per capability. Capabilities read
/// across slices (e.g. `kv` checks `state.app`) but each only writes its own.
/// Adding a capability with new data adds a field here.
///
/// Not `Eq`: the `crdt` slice compares by Loro deep value, which can hold floats
/// (`f64`), so only `PartialEq` is available — sufficient for the replay-identity
/// check and `assert_eq!`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct State {
    pub agent: AgentState,
    pub app: AppState,
    pub auth: AuthState,
    pub builder: BuilderState,
    pub harness: HarnessState,
    pub kv: KvState,
    pub net: NetState,
    pub model: ModelState,
    pub local_model: LocalModelState,
    pub native: NativeState,
    pub scheduler: SchedulerState,
    pub stt: SttState,
    pub crdt: CrdtState,
    pub replica: ReplicaState,
    pub time: TimeState,
}

impl StateStore for State {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "agent" => Some(&self.agent),
            "app" => Some(&self.app),
            "auth" => Some(&self.auth),
            "builder" => Some(&self.builder),
            "harness" => Some(&self.harness),
            "kv" => Some(&self.kv),
            "net" => Some(&self.net),
            "model" => Some(&self.model),
            "local-model" => Some(&self.local_model),
            "native" => Some(&self.native),
            "scheduler" => Some(&self.scheduler),
            "stt" => Some(&self.stt),
            "crdt" => Some(&self.crdt),
            "replica" => Some(&self.replica),
            "time" => Some(&self.time),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "agent" => Some(&mut self.agent),
            "app" => Some(&mut self.app),
            "auth" => Some(&mut self.auth),
            "builder" => Some(&mut self.builder),
            "harness" => Some(&mut self.harness),
            "kv" => Some(&mut self.kv),
            "net" => Some(&mut self.net),
            "model" => Some(&mut self.model),
            "local-model" => Some(&mut self.local_model),
            "native" => Some(&mut self.native),
            "scheduler" => Some(&mut self.scheduler),
            "stt" => Some(&mut self.stt),
            "crdt" => Some(&mut self.crdt),
            "replica" => Some(&mut self.replica),
            "time" => Some(&mut self.time),
            _ => None,
        }
    }
}

/// Performs effects at the edge. Implementors do the real I/O (or, in tests, a
/// local I/O implementation) and return the recorded Event(s).
pub trait EffectRunner {
    fn run(&self, effect: &Effect, state: &State) -> Result<Vec<EventRecord>>;

    /// Edge access for live (non-recorded) reads — system metrics and the like.
    /// Returns `None` for runners that observe nothing outside the log; the
    /// default. Hosts with real I/O override this so `ctx.resource.*` reads that
    /// need a live sample can reach the outside world without recording anything.
    fn live(&self) -> Option<&dyn LiveHost> {
        None
    }
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
        self.try_register(capability)
            .expect("capability registry should be valid");
    }

    /// Try to plug a capability in. Its namespace must be unique.
    pub fn try_register(&mut self, capability: Box<dyn Capability>) -> Result<()> {
        let namespace = capability.namespace();
        if self.caps.contains_key(namespace) {
            return Err(Error::InvalidInput(format!(
                "duplicate capability namespace: {namespace}"
            )));
        }
        self.caps.insert(namespace, capability);
        Ok(())
    }

    pub(crate) fn get(&self, namespace: &str) -> Result<&dyn Capability> {
        self.caps
            .get(namespace)
            .map(AsRef::as_ref)
            .ok_or_else(|| Error::InvalidInput(format!("unknown command namespace: {namespace}")))
    }

    /// Validate the registry-wide declaration surface: command/query names and
    /// emitted event kinds have one owner, and subscriptions reference declared
    /// events without claiming ownership themselves.
    pub fn validate(&self) -> Result<()> {
        let mut commands = BTreeMap::<&'static str, &'static str>::new();
        let mut queries = BTreeMap::<&'static str, &'static str>::new();
        let mut events = BTreeMap::<&'static str, &'static str>::new();
        let mut subscriptions = Vec::<(&'static str, &'static str)>::new();

        for capability in self.caps.values() {
            let namespace = capability.namespace();
            let manifest = capability.manifest();

            for command in &manifest.commands {
                validate_dotted_owner(namespace, "command", command.name)?;
                insert_unique(&mut commands, command.name, namespace, "command")?;
            }
            for query in &manifest.queries {
                validate_dotted_owner(namespace, "query", query.name)?;
                insert_unique(&mut queries, query.name, namespace, "query")?;
            }
            for event in &manifest.events {
                validate_dotted_owner(namespace, "event", event.kind)?;
                insert_unique(&mut events, event.kind, namespace, "event")?;
            }
            validate_grant_resources(namespace, &manifest.resources, &manifest.grant_resources)?;
            for subscription in &manifest.subscriptions {
                subscriptions.push((namespace, subscription.kind));
            }
        }

        for (subscriber, kind) in subscriptions {
            if !events.contains_key(kind) {
                return Err(Error::InvalidInput(format!(
                    "{subscriber} subscribes to undeclared event: {kind}"
                )));
            }
        }

        Ok(())
    }
}

fn insert_unique(
    owners: &mut BTreeMap<&'static str, &'static str>,
    name: &'static str,
    owner: &'static str,
    label: &str,
) -> Result<()> {
    if let Some(previous) = owners.insert(name, owner) {
        return Err(Error::InvalidInput(format!(
            "duplicate {label} declaration: {name} owned by both {previous} and {owner}"
        )));
    }
    Ok(())
}

fn validate_grant_resources(
    namespace: &'static str,
    resources: &[ResourceMethod],
    specs: &[GrantResourceSpec],
) -> Result<()> {
    if resources.is_empty() {
        if specs.is_empty() {
            return Ok(());
        }
        return Err(Error::InvalidInput(format!(
            "{namespace} declares grant resource specs without ctx.resource methods"
        )));
    }

    if specs.is_empty() {
        return Err(Error::InvalidInput(format!(
            "{namespace} exposes ctx.resource.{namespace} without grant resource specs"
        )));
    }

    let mut selector_schemas = BTreeSet::new();
    let mut covered_verbs = BTreeSet::new();
    let mut has_namespace_v1 = false;

    for spec in specs {
        if spec.namespace != namespace {
            return Err(Error::InvalidInput(format!(
                "grant resource spec for {} declared by {namespace}",
                spec.namespace
            )));
        }
        validate_safe_token("grant selector schema id", spec.selector_schema_id)?;
        if !selector_schemas.insert(spec.selector_schema_id) {
            return Err(Error::InvalidInput(format!(
                "duplicate grant selector schema for {namespace}: {}",
                spec.selector_schema_id
            )));
        }
        if spec.selector_schema_json.trim().is_empty() {
            return Err(Error::InvalidInput(format!(
                "grant selector schema {} for {namespace} is empty",
                spec.selector_schema_id
            )));
        }
        if spec.verbs.is_empty() {
            return Err(Error::InvalidInput(format!(
                "grant resource spec {} for {namespace} has no verbs",
                spec.selector_schema_id
            )));
        }
        if !(spec.compatibility.backward && spec.compatibility.forward) {
            return Err(Error::InvalidInput(format!(
                "grant resource spec {} for {namespace} must be backward and forward compatible",
                spec.selector_schema_id
            )));
        }
        if spec.selector_schema_id == NAMESPACE_SELECTOR_SCHEMA_ID {
            has_namespace_v1 = true;
        }
        for verb in spec.verbs {
            validate_safe_token("grant resource verb", verb)?;
            covered_verbs.insert(*verb);
        }
    }

    if !has_namespace_v1 {
        return Err(Error::InvalidInput(format!(
            "{namespace} exposes ctx.resource.{namespace} without a namespace.v1 grant spec"
        )));
    }

    for method in resources {
        if !covered_verbs.contains(method.kind()) {
            return Err(Error::InvalidInput(format!(
                "{namespace}.{} has no grant resource spec for {} access",
                method.name(),
                method.kind()
            )));
        }
    }

    Ok(())
}

fn validate_safe_token(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "{label} is unsafe: {value:?}; use ASCII letters, digits, '.', '-' or '_'"
        )));
    }
    Ok(())
}

fn validate_dotted_owner(owner: &str, label: &str, name: &str) -> Result<()> {
    let Some((namespace, _)) = name.split_once('.') else {
        return Err(Error::InvalidInput(format!(
            "{label} declaration must be 'namespace.name': {name}"
        )));
    };
    if namespace != owner {
        return Err(Error::InvalidInput(format!(
            "{label} declaration {name} does not belong to capability {owner}"
        )));
    }
    Ok(())
}

/// A read-only query bus over an arbitrary registry/state pair.
pub struct RegistryBus<'a> {
    registry: &'a Registry,
    state: &'a dyn StateStore,
}

impl<'a> RegistryBus<'a> {
    pub fn new(registry: &'a Registry, state: &'a dyn StateStore) -> Self {
        Self { registry, state }
    }
}

impl CapBus for RegistryBus<'_> {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue> {
        let capability = self
            .registry
            .caps
            .get(cap)
            .map(AsRef::as_ref)
            .ok_or_else(|| Error::InvalidInput(format!("unknown query capability: {cap}")))?;
        let ctx = QueryCtx {
            state: self.state,
            bus: self,
        };
        capability.query(ctx, name, args)
    }

    fn grant_resource_spec(
        &self,
        namespace: &str,
        selector_schema_id: &str,
    ) -> Result<Option<GrantResourceSpec>> {
        let Some(capability) = self.registry.caps.get(namespace).map(AsRef::as_ref) else {
            return Ok(None);
        };
        Ok(capability.grant_resource_specs().into_iter().find(|spec| {
            spec.namespace == namespace && spec.selector_schema_id == selector_schema_id
        }))
    }
}

/// The registry every core opens with: the built-in capabilities.
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Box::new(terrane_cap_agent::AgentCapability));
    registry.register(Box::new(terrane_cap_app::AppCapability));
    registry.register(Box::new(terrane_cap_auth::AuthCapability));
    registry.register(Box::new(terrane_cap_build::BuildCapability));
    registry.register(Box::new(terrane_cap_builder::BuilderCapability));
    registry.register(Box::new(terrane_cap_harness::HarnessCapability));
    registry.register(Box::new(terrane_cap_kv::KvCapability));
    registry.register(Box::new(terrane_cap_relational_db::RelationalDbCapability));
    registry.register(Box::new(terrane_cap_search::SearchCapability));
    registry.register(Box::new(terrane_cap_crdt::CrdtCapability));
    registry.register(Box::new(terrane_cap_crypto::CryptoCapability));
    registry.register(Box::new(terrane_cap_replica::ReplicaCapability));
    registry.register(Box::new(terrane_cap_net::NetCapability));
    registry.register(Box::new(terrane_cap_model::ModelCapability));
    registry.register(Box::new(terrane_cap_local_model::LocalModelCapability));
    registry.register(Box::new(terrane_cap_native::NativeCapability));
    registry.register(Box::new(terrane_cap_scheduler::SchedulerCapability));
    registry.register(Box::new(terrane_cap_stt::SttCapability));
    registry.register(Box::new(terrane_cap_time::TimeCapability));
    registry.register(Box::new(terrane_cap_sysinfo::SysinfoCapability));
    registry.register(Box::new(terrane_cap_js_runtime::JsRuntimeCapability));
    registry.register(Box::new(terrane_cap_wasm_runtime::WasmRuntimeCapability));
    registry
        .validate()
        .expect("default capability registry should be valid");
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

/// Capability-owned grant resource specs, sorted by namespace then schema id.
pub fn grant_resource_specs() -> Vec<GrantResourceSpec> {
    let registry = default_registry();
    let mut specs = Vec::new();
    for capability in registry.caps.values() {
        specs.extend(capability.grant_resource_specs());
    }
    specs.sort_by(|a, b| {
        a.namespace
            .cmp(b.namespace)
            .then_with(|| a.selector_schema_id.cmp(b.selector_schema_id))
    });
    specs
}

/// Resource namespaces that generated apps may request because a registered
/// capability owns a grant spec for them.
pub fn grant_resource_namespaces() -> Vec<&'static str> {
    let mut out: Vec<_> = grant_resource_specs()
        .into_iter()
        .map(|spec| spec.namespace)
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Every command declared by registered runtime capabilities, sorted.
pub fn command_names() -> Vec<&'static str> {
    let registry = default_registry();
    let mut out = Vec::new();
    for capability in registry.caps.values() {
        out.extend(
            capability
                .manifest()
                .commands
                .into_iter()
                .map(|spec| spec.name),
        );
    }
    out.sort_unstable();
    out
}

/// Every query declared by registered runtime capabilities, sorted.
pub fn query_names() -> Vec<&'static str> {
    let registry = default_registry();
    let mut out = Vec::new();
    for capability in registry.caps.values() {
        out.extend(
            capability
                .manifest()
                .queries
                .into_iter()
                .map(|spec| spec.name),
        );
    }
    out.sort_unstable();
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

/// Capability documentation for registered runtime capabilities.
pub fn capability_docs(include_internal: bool) -> Vec<CapabilityDoc> {
    let registry = default_registry();
    let mut docs: Vec<CapabilityDoc> = registry
        .caps
        .values()
        .map(|c| c.doc(include_internal))
        .collect();
    docs.extend(planned_docs::all(include_internal));
    docs.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    docs
}

pub fn capability_doc(namespace: &str, include_internal: bool) -> Result<CapabilityDoc> {
    if let Some(doc) = planned_docs::get(namespace, include_internal) {
        return Ok(doc);
    }
    let registry = default_registry();
    Ok(registry.get(namespace)?.doc(include_internal))
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

/// Resource bridge used by runtime capabilities. It owns a working State so
/// guest-code writes are visible to later reads during the same run, while the
/// real log is untouched until core commits the collected records.
pub struct RuntimeResourceHost {
    app: String,
    principal: ExecutionPrincipal,
    state: State,
    registry: Registry,
    temporary_allowed_resources: BTreeSet<String>,
    recorded: Vec<RecordedWrite>,
    /// Per-(namespace.method) count of recorded `Decision::Effect` calls in this
    /// run, used to enforce [`Capability::recorded_call_per_run_limit`]. A host
    /// is fresh per backend run, so this is naturally per-run scoped and never
    /// persisted or replayed.
    recorded_call_counts: BTreeMap<String, usize>,
    /// Runs `Decision::Effect` from `ResourceMethod::Call` invocations; calls
    /// are refused when the host was built without one.
    runner: Option<std::sync::Arc<dyn EffectRunner>>,
}

impl RuntimeResourceHost {
    pub fn new(app: impl Into<String>, base_state: State) -> Self {
        Self::new_with_principal(app, base_state, ExecutionPrincipal::local_owner())
    }

    pub fn new_with_principal(
        app: impl Into<String>,
        base_state: State,
        principal: ExecutionPrincipal,
    ) -> Self {
        Self {
            app: app.into(),
            principal,
            state: base_state,
            registry: default_registry(),
            temporary_allowed_resources: BTreeSet::new(),
            recorded: Vec::new(),
            recorded_call_counts: BTreeMap::new(),
            runner: None,
        }
    }

    /// Attach an effect runner so `ResourceMethod::Call` invocations can run
    /// their effects (recorded like any write).
    pub fn with_runner(mut self, runner: std::sync::Arc<dyn EffectRunner>) -> Self {
        self.runner = Some(runner);
        self
    }

    pub fn new_with_temporary_resource_grants(
        app: impl Into<String>,
        base_state: State,
        principal: ExecutionPrincipal,
        resources: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut host = Self::new_with_principal(app, base_state, principal);
        host.temporary_allowed_resources = resources.into_iter().collect();
        host
    }
}

struct RecordedWrite {
    record: EventRecord,
    coalesce_key: Option<String>,
    is_set: bool,
}

impl RuntimeHost for RuntimeResourceHost {
    fn resource_methods(&self, namespace: &str) -> Result<Vec<ResourceMethod>> {
        let capability = self.registry.get(namespace)?;
        let manifest = capability.manifest();
        if !manifest.resources.is_empty() && manifest.grant_resources.is_empty() {
            return Err(Error::InvalidInput(format!(
                "{namespace} exposes ctx.resource.{namespace} without grant resource specs"
            )));
        }
        if self.temporary_allowed_resources.contains(namespace)
            || dev_allow_requested_resources()
            || terrane_cap_auth::namespace_granted(
                &self.state,
                &self.principal,
                &self.app,
                namespace,
            )?
        {
            return Ok(manifest.resources);
        }
        Ok(Vec::new())
    }

    fn read_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let capability = self.registry.get(namespace)?;
        let bus = RegistryBus::new(&self.registry, &self.state);
        capability.read_resource(
            ResourceReadCtx {
                state: &self.state,
                bus: &bus,
                app: &self.app,
                host: self.runner.as_deref().and_then(|runner| runner.live()),
            },
            method,
            args,
        )
    }

    fn write_resource(&mut self, namespace: &str, method: &str, args: &[String]) -> Result<()> {
        let name = format!("{namespace}.{method}");
        let mut scoped_args = Vec::with_capacity(args.len() + 1);
        scoped_args.push(self.app.clone());
        scoped_args.extend(args.iter().cloned());

        let coalesce_key = (namespace == "kv" && matches!(method, "set" | "rm"))
            .then(|| scoped_args.get(1).cloned())
            .flatten();
        let is_set = namespace == "kv" && method == "set";

        let bus = RegistryBus::new(&self.registry, &self.state);
        let ctx = CommandCtx {
            state: &self.state,
            bus: &bus,
        };
        let decision = self
            .registry
            .get(namespace)?
            .decide(ctx, &name, &scoped_args)?;
        let records = match decision {
            Decision::Commit(records) => records,
            Decision::Effect(_) | Decision::TransientEffect(_) | Decision::Runtime(_) => {
                return Err(Error::Runtime(format!(
                    "{name}: effects and runtime calls are not allowed inside a runtime"
                )));
            }
        };
        for record in &records {
            apply(&self.registry, &mut self.state, record)?;
        }
        for record in records {
            self.recorded.push(RecordedWrite {
                record,
                coalesce_key: coalesce_key.clone(),
                is_set,
            });
        }
        Ok(())
    }

    fn call_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let Some(runner) = self.runner.clone() else {
            return Err(Error::Runtime(format!(
                "{namespace}.{method}: resource calls are not available in this runtime host"
            )));
        };
        let name = format!("{namespace}.{method}");
        let mut scoped_args = Vec::with_capacity(args.len() + 1);
        scoped_args.push(self.app.clone());
        scoped_args.extend(args.iter().cloned());

        let bus = RegistryBus::new(&self.registry, &self.state);
        let ctx = CommandCtx {
            state: &self.state,
            bus: &bus,
        };
        let capability = self.registry.get(namespace)?;
        let recorded_call_limit = capability.recorded_call_per_run_limit(method);
        let decision = capability.decide(ctx, &name, &scoped_args)?;
        let records = match decision {
            Decision::Commit(records) => records,
            // The one place effects are legal inside a runtime: run once now;
            // replay folds the recorded events without re-running the app.
            Decision::Effect(effect) => {
                enforce_recorded_call_per_run_limit(
                    &mut self.recorded_call_counts,
                    namespace,
                    method,
                    recorded_call_limit,
                )?;
                runner.run(&effect, &self.state)?
            }
            // A live query: run for its result and hand it back, but record
            // NOTHING — nothing is persisted, folded, or replayed. Used for
            // transient reads (e.g. net.get) whose response must never touch the
            // event log.
            Decision::TransientEffect(effect) => {
                let records = runner.run(&effect, &self.state)?;
                return self.registry.get(namespace)?.resource_call_output(
                    &self.state,
                    &self.app,
                    method,
                    &records,
                );
            }
            Decision::Runtime(_) => {
                return Err(Error::Runtime(format!(
                    "{name}: nested runtime calls are not allowed inside a runtime"
                )));
            }
        };
        for record in &records {
            apply(&self.registry, &mut self.state, record)?;
        }
        let output = self.registry.get(namespace)?.resource_call_output(
            &self.state,
            &self.app,
            method,
            &records,
        )?;
        for record in records {
            self.recorded.push(RecordedWrite {
                record,
                coalesce_key: None,
                is_set: false,
            });
        }
        Ok(output)
    }

    fn take_records(&mut self) -> Vec<EventRecord> {
        coalesce(std::mem::take(&mut self.recorded))
    }
}

fn coalesce(writes: Vec<RecordedWrite>) -> Vec<EventRecord> {
    let mut keep = vec![true; writes.len()];
    for i in 0..writes.len() {
        if !writes[i].is_set {
            continue;
        }
        let Some(key) = writes[i].coalesce_key.as_deref() else {
            continue;
        };
        if writes[i + 1..]
            .iter()
            .any(|w| w.coalesce_key.as_deref() == Some(key))
        {
            keep[i] = false;
        }
    }
    writes
        .into_iter()
        .zip(keep)
        .filter_map(|(write, keep)| keep.then_some(write.record))
        .collect()
}

#[cfg(debug_assertions)]
fn dev_allow_requested_resources() -> bool {
    std::env::var("TERRANE_DEV_ALLOW_REQUESTED_RESOURCES").is_ok_and(|value| value == "1")
}

#[cfg(not(debug_assertions))]
fn dev_allow_requested_resources() -> bool {
    false
}

/// Enforce a capability's declared per-run cap on recorded `Decision::Effect`
/// calls. `counts` is the runtime host's per-run tally (keyed by
/// `"<namespace>.<method>"`); `None` means the capability did not declare a cap
/// and the call proceeds uncapped. The (limit+1)-th recorded call is rejected
/// with a typed error naming the capability's escape hint.
fn enforce_recorded_call_per_run_limit(
    counts: &mut BTreeMap<String, usize>,
    namespace: &str,
    method: &str,
    limit: Option<RecordedCallCap>,
) -> Result<()> {
    let Some(rcap) = limit else {
        return Ok(());
    };
    let key = format!("{namespace}.{method}");
    let count = counts.entry(key).or_insert(0);
    if *count >= rcap.limit {
        return Err(Error::InvalidInput(format!(
            "{namespace}.{method}: per-run recorded-call limit ({}) reached; {}",
            rcap.limit, rcap.escape_hint
        )));
    }
    *count += 1;
    Ok(())
}

/// Read every [`EventRecord`] from the log, in order. New logs start with a
/// fixed magic/version header, followed by length-prefixed borsh records: a
/// little-endian `u32` byte length followed by that many bytes of one
/// borsh-encoded `EventRecord`. A missing log is an empty history, not an error.
pub fn read_log(log_path: &Path) -> Result<Vec<EventRecord>> {
    read_new_log(log_path)
}

fn old_log_error() -> Error {
    Error::Storage(
        "old-format event log: run `terrane migrate-log` before opening this home".into(),
    )
}

fn read_new_log(log_path: &Path) -> Result<Vec<EventRecord>> {
    let file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Storage(e.to_string())),
    };
    let mut reader = BufReader::new(file);
    match read_header(&mut reader)? {
        HeaderStatus::MissingEmpty => return Ok(Vec::new()),
        HeaderStatus::Present => {}
        HeaderStatus::OldFormat => return Err(old_log_error()),
    }
    read_framed_records(&mut reader, |buf| {
        borsh::from_slice::<EventRecord>(buf)
            .map_err(|e| Error::Storage(format!("corrupt log record: {e}")))
    })
}

enum HeaderStatus {
    Present,
    OldFormat,
    MissingEmpty,
}

fn read_header(reader: &mut BufReader<std::fs::File>) -> Result<HeaderStatus> {
    let mut header = vec![0u8; LOG_HEADER.len()];
    let mut got = 0usize;
    while got < header.len() {
        match reader.read(&mut header[got..]) {
            Ok(0) if got == 0 => return Ok(HeaderStatus::MissingEmpty),
            Ok(0) => return Ok(HeaderStatus::OldFormat),
            Ok(n) => got += n,
            Err(e) => return Err(Error::Storage(e.to_string())),
        }
    }
    if header == LOG_HEADER {
        Ok(HeaderStatus::Present)
    } else {
        Ok(HeaderStatus::OldFormat)
    }
}

fn read_legacy_log(log_path: &Path) -> Result<Vec<EventRecord>> {
    let file = match std::fs::File::open(log_path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::Storage(e.to_string())),
    };
    let mut reader = BufReader::new(file);
    match read_header(&mut reader)? {
        HeaderStatus::MissingEmpty => return Ok(Vec::new()),
        HeaderStatus::Present => {
            return Err(Error::Storage(
                "event log already uses the actor format".into(),
            ))
        }
        HeaderStatus::OldFormat => {}
    }
    let file = std::fs::File::open(log_path).map_err(|e| Error::Storage(e.to_string()))?;
    let mut reader = BufReader::new(file);
    read_framed_records(&mut reader, |buf| {
        let legacy = borsh::from_slice::<LegacyEventRecord>(buf)
            .map_err(|e| Error::Storage(format!("corrupt legacy log record: {e}")))?;
        Ok(EventRecord {
            kind: legacy.kind,
            payload: legacy.payload,
            actor: LOCAL_OWNER_SUBJECT.to_string(),
        })
    })
}

fn read_framed_records<T>(
    reader: &mut BufReader<std::fs::File>,
    mut decode: impl FnMut(&[u8]) -> Result<T>,
) -> Result<Vec<T>> {
    let mut records = Vec::new();
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
        records.push(decode(&buf)?);
    }
    Ok(records)
}

fn write_new_log(log_path: &Path, records: &[EventRecord]) -> Result<()> {
    if let Some(parent) = log_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Storage(e.to_string()))?;
        }
    }
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path)
        .map_err(|e| Error::Storage(e.to_string()))?;
    file.write_all(LOG_HEADER)
        .map_err(|e| Error::Storage(e.to_string()))?;
    write_record_frames(&mut file, records)
}

fn write_record_frames(file: &mut std::fs::File, records: &[EventRecord]) -> Result<()> {
    let mut frame = Vec::new();
    for record in records {
        let bytes = borsh::to_vec(record).map_err(|e| Error::Storage(e.to_string()))?;
        let len = u32::try_from(bytes.len())
            .map_err(|_| Error::Storage("event record too large".into()))?;
        frame.extend_from_slice(&len.to_le_bytes());
        frame.extend_from_slice(&bytes);
    }
    file.write_all(&frame)
        .map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}

pub fn migrate_log(log_path: &Path) -> Result<usize> {
    let old_records = read_legacy_log(log_path)?;
    let registry = default_registry();
    let mut old_state = State::default();
    for record in &old_records {
        apply(&registry, &mut old_state, record)?;
    }

    let migrated_records: Vec<EventRecord> = old_records
        .into_iter()
        .map(|mut record| {
            record.actor = LOCAL_OWNER_SUBJECT.to_string();
            record
        })
        .collect();
    let mut migrated_state = State::default();
    for record in &migrated_records {
        apply(&registry, &mut migrated_state, record)?;
    }
    if old_state != migrated_state {
        return Err(Error::Storage(
            "migrated actor log did not replay to the same state".into(),
        ));
    }

    let tmp_path = log_path.with_extension("bin.actor-migrating");
    write_new_log(&tmp_path, &migrated_records)?;
    let backup_path = log_path.with_file_name(PRE_ACTOR_BACKUP_NAME);
    if backup_path.exists() {
        return Err(Error::Storage(format!(
            "backup already exists: {}; move it before rerunning migrate-log",
            backup_path.display()
        )));
    }
    if log_path.exists() {
        std::fs::rename(log_path, &backup_path).map_err(|e| {
            Error::Storage(format!(
                "move {} to {}: {e}",
                log_path.display(),
                backup_path.display()
            ))
        })?;
    }
    std::fs::rename(&tmp_path, log_path).map_err(|e| {
        Error::Storage(format!(
            "move {} to {}: {e}",
            tmp_path.display(),
            log_path.display()
        ))
    })?;
    Ok(migrated_records.len())
}

/// The engine: an on-disk event log, the State folded from it, an injected
/// [`EffectRunner`], and the [`Registry`] of capabilities. Pure usage leaves the
/// runner untouched, so it defaults to [`NoEffects`].
pub struct Core<R: EffectRunner + 'static = NoEffects> {
    log_path: PathBuf,
    state: State,
    kv_storage_plan: KvStoragePlan,
    runner: std::sync::Arc<R>,
    registry: Registry,
    /// String printed by the most recent runtime backend, if any. Not part of
    /// State, never logged or replayed — purely a transport for the host to print.
    last_output: Option<String>,
    /// Exclusive cross-process guard on this home, held for the `Core`'s lifetime
    /// so a second process cannot corrupt the shared log. In-process opens share
    /// it (see [`filelock`]).
    _home_lock: Arc<filelock::LockHandle>,
}

impl Core<NoEffects> {
    /// Open (or create) a pure core at `log_path` — no effects enabled.
    pub fn open(log_path: impl Into<PathBuf>) -> Result<Self> {
        Core::open_with(log_path, NoEffects)
    }
}

impl<R: EffectRunner + 'static> Core<R> {
    /// Open (or create) a core at `log_path` with an effect runner, rebuilding
    /// State by folding the existing log through the default registry.
    pub fn open_with(log_path: impl Into<PathBuf>, runner: R) -> Result<Self> {
        let log_path = log_path.into();
        // Take the cross-process write guard before reading, so we replay and
        // then own the log without a second writer racing us.
        let home_lock = filelock::acquire(&log_path)?;
        let registry = default_registry();
        let mut state = State::default();
        for record in read_log(&log_path)? {
            apply(&registry, &mut state, &record)?;
        }
        let kv_storage_plan = terrane_cap_kv::storage_plan(&state)?;
        let storage_home = storage_home(&log_path);
        terrane_cap_kv::sync_full_storage(&storage_home, &state.kv)?;
        terrane_cap_auth::sync_reserved_projection(
            &storage_home,
            &state.kv.storage.binding_for(None),
            &state.auth,
        )?;
        Ok(Core {
            log_path,
            state,
            kv_storage_plan,
            runner: Arc::new(runner),
            registry,
            last_output: None,
            _home_lock: home_lock,
        })
    }

    /// The current world. Reads go through here.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Core-facing storage projection plan owned by the `kv` capability.
    pub fn kv_storage_plan(&self) -> &KvStoragePlan {
        &self.kv_storage_plan
    }

    /// Run a command end to end: route to its capability, decide, then commit
    /// events (running an effect first if the decision calls for one). Nothing is
    /// written unless the command succeeds.
    pub fn dispatch(&mut self, request: Request) -> Result<Vec<EventRecord>> {
        self.last_output = None;
        let namespace = namespace_of(&request.name)?.to_string();
        let principal = request.principal.clone();
        let decision = self.decide(request)?;
        match decision {
            Decision::Commit(records) => self.commit(records, &principal),
            Decision::Effect(effect) => {
                let records = self.runner.run(&effect, &self.state)?;
                self.commit(records, &principal)
            }
            // Transient effects only make sense as a resource call (they return a
            // value without recording); a top-level command must not use one.
            Decision::TransientEffect(_) => Err(Error::InvalidInput(format!(
                "{namespace}: transient effects are only valid as resource calls"
            ))),
            Decision::Runtime(request) => self.run_runtime(&namespace, request, principal),
        }
    }

    /// Route a command to its owning capability and return the decision without
    /// committing, running effects, or invoking runtimes.
    pub fn decide(&self, request: Request) -> Result<Decision> {
        admit_command(&request)?;
        let namespace = namespace_of(&request.name)?;
        let bus = RegistryBus::new(&self.registry, &self.state);
        let ctx = CommandCtx {
            state: &self.state,
            bus: &bus,
        };
        self.registry
            .get(namespace)?
            .decide(ctx, &request.name, &request.args)
    }

    /// Run a read-only capability query against the current state.
    pub fn query(&self, capability: &str, query: &str, args: &[String]) -> Result<QueryValue> {
        let name = match query.split_once('.') {
            Some((namespace, name)) if namespace == capability => name,
            Some((namespace, _)) => {
                return Err(Error::InvalidInput(format!(
                    "query {query} does not belong to capability {capability} (got {namespace})"
                )));
            }
            None => query,
        };
        let bus = RegistryBus::new(&self.registry, &self.state);
        bus.query(capability, name, args)
    }

    fn run_runtime(
        &mut self,
        namespace: &str,
        request: RuntimeRequest,
        principal: ExecutionPrincipal,
    ) -> Result<Vec<EventRecord>> {
        self.last_output = None;
        let app = self
            .state
            .app
            .apps
            .get(&request.app)
            .ok_or_else(|| Error::AppNotFound(request.app.clone()))?;
        let source = app
            .source
            .clone()
            .ok_or_else(|| Error::Runtime(format!("app {} has no --source bundle", app.id)))?;
        let source_files = match terrane_cap_kv::app_bundle_app_id(&source) {
            Some(source_app) if source_app == app.id => {
                let files = terrane_cap_kv::app_bundle_files(&self.state, &app.id)?;
                if files.is_empty() {
                    return Err(Error::Runtime(format!(
                        "app {} has kv bundle source but no stored bundle files",
                        app.id
                    )));
                }
                Some(files)
            }
            Some(source_app) => {
                return Err(Error::Runtime(format!(
                    "app {} points at kv bundle for different app {}",
                    app.id, source_app
                )))
            }
            None => None,
        };
        let host = RuntimeHostHandle::new(Box::new(
            RuntimeResourceHost::new_with_principal(
                request.app.clone(),
                self.state.clone(),
                principal.clone(),
            )
            .with_runner(self.runner.clone()),
        ));
        let ctx = RuntimeCtx {
            source,
            source_files,
            app_name: app.name.clone(),
            host: host.clone(),
        };
        let result = self.registry.get(namespace)?.run_runtime(ctx, request)?;
        let records = self.commit(host.take_records(), &principal)?;
        self.last_output = Some(result.output);
        Ok(records)
    }

    /// Take the string printed by the most recent runtime run (if any). Not part
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
            let detail = described
                .unwrap_or_else(|| format!("{} ({} bytes)", record.kind, record.payload.len()));
            lines.push(format!("{} {detail}", record.actor));
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
    fn commit(
        &mut self,
        records: Vec<EventRecord>,
        principal: &ExecutionPrincipal,
    ) -> Result<Vec<EventRecord>> {
        let actor = principal.actor();
        let records: Vec<EventRecord> = records
            .into_iter()
            .map(|mut record| {
                record.actor = actor.clone();
                record
            })
            .collect();
        let before_kv = self.state.kv.clone();
        let before_auth = self.state.auth.clone();
        self.append(&records)?;
        for record in &records {
            apply(&self.registry, &mut self.state, record)?;
        }
        self.kv_storage_plan = terrane_cap_kv::storage_plan(&self.state)?;
        let home = storage_home(&self.log_path);
        terrane_cap_kv::sync_storage_after_commit(&home, &before_kv, &self.state.kv)?;
        let before_auth_binding = before_kv.storage.binding_for(None);
        let after_auth_binding = self.state.kv.storage.binding_for(None);
        if before_auth != self.state.auth || before_auth_binding != after_auth_binding {
            terrane_cap_auth::sync_reserved_projection_after_commit(
                &home,
                &before_auth_binding,
                &after_auth_binding,
                &self.state.auth,
            )?;
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
            .read(true)
            .append(true)
            .open(&self.log_path)
            .map_err(|e| Error::Storage(e.to_string()))?;
        let is_empty = file
            .metadata()
            .map_err(|e| Error::Storage(e.to_string()))?
            .len()
            == 0;
        if is_empty {
            file.write_all(LOG_HEADER)
                .map_err(|e| Error::Storage(e.to_string()))?;
        } else {
            let check =
                std::fs::File::open(&self.log_path).map_err(|e| Error::Storage(e.to_string()))?;
            let mut reader = BufReader::new(check);
            match read_header(&mut reader)? {
                HeaderStatus::Present => {}
                HeaderStatus::MissingEmpty => {}
                HeaderStatus::OldFormat => return Err(old_log_error()),
            }
        }
        // Frame the whole batch in one buffer and write it once: with O_APPEND
        // plus the home lock, a single `write_all` cannot interleave a length from
        // one writer with bytes from another (no torn records).
        write_record_frames(&mut file, records)
    }
}

fn admit_command(request: &Request) -> Result<()> {
    // Trusted-host only:
    // - `auth.*` and `kv.public.*` mutate cross-app/platform data (CLI + FFI both
    //   dispatch as trusted_host; app backends reach KV only through
    //   RuntimeResourceHost::write_resource, which calls capability.decide()
    //   directly and never routes through admit_command).
    // - the host-owned edge of `stt` (session lifecycle, segment append,
    //   retention, purge). Apps may call only `stt.select` and `stt.stop`, so
    //   those are deliberately not gated here.
    let trusted_only = request.name.starts_with("auth.")
        || request.name.starts_with("kv.public.")
        || request.name == "stt.session.open"
        || request.name == "stt.segment.append"
        || request.name == "stt.session.close-host"
        || request.name == "stt.retention.trim"
        || request.name == "stt.session.purge";
    if trusted_only && !request.authority.is_trusted_host() {
        return Err(Error::InvalidInput(format!(
            "{} requires trusted host authority",
            request.name
        )));
    }
    Ok(())
}

fn storage_home(log_path: &Path) -> PathBuf {
    log_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
