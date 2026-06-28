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

pub mod domain;
pub mod host_runtime;

pub use terrane_cap_interface::{
    arg, decode_event, encode_event, namespace_of, AppId, CapBus, Capability, CapabilityDoc,
    CapabilityManifestDoc, CommandCtx, Decision, Effect, Error, EventRecord, ExampleDoc,
    InternalNote, LimitDoc, ParamDoc, QueryCtx, QueryValue, Request, ResourceDoc,
    ResourceMethodDoc, Result, SchemaDoc, StateStore,
};

use terrane_cap_app::AppState;
use terrane_cap_builder::BuilderState;
use terrane_cap_crdt::CrdtState;
use terrane_cap_harness::HarnessState;
use terrane_cap_kv::KvState;
use terrane_cap_model::ModelState;
use terrane_cap_net::NetState;
use terrane_cap_replica::ReplicaState;

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

impl StateStore for State {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "app" => Some(&self.app),
            "builder" => Some(&self.builder),
            "harness" => Some(&self.harness),
            "kv" => Some(&self.kv),
            "net" => Some(&self.net),
            "model" => Some(&self.model),
            "crdt" => Some(&self.crdt),
            "replica" => Some(&self.replica),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "app" => Some(&mut self.app),
            "builder" => Some(&mut self.builder),
            "harness" => Some(&mut self.harness),
            "kv" => Some(&mut self.kv),
            "net" => Some(&mut self.net),
            "model" => Some(&mut self.model),
            "crdt" => Some(&mut self.crdt),
            "replica" => Some(&mut self.replica),
            _ => None,
        }
    }
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
}

/// The registry every core opens with: the built-in capabilities.
pub fn default_registry() -> Registry {
    let mut registry = Registry::new();
    registry.register(Box::new(terrane_cap_app::AppCapability));
    registry.register(Box::new(terrane_cap_build::BuildCapability));
    registry.register(Box::new(terrane_cap_builder::BuilderCapability));
    registry.register(Box::new(terrane_cap_harness::HarnessCapability));
    registry.register(Box::new(terrane_cap_kv::KvCapability));
    registry.register(Box::new(terrane_cap_crdt::CrdtCapability));
    registry.register(Box::new(terrane_cap_replica::ReplicaCapability));
    registry.register(Box::new(terrane_cap_net::NetCapability));
    registry.register(Box::new(terrane_cap_model::ModelCapability));
    registry.register(Box::new(host_runtime::HostCapability));
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

/// Capability documentation for registered runtime capabilities plus planned
/// capabilities whose docs are intentionally exposed before runtime injection.
pub fn capability_docs(include_internal: bool) -> Vec<CapabilityDoc> {
    let registry = default_registry();
    let mut docs: Vec<CapabilityDoc> = registry
        .caps
        .values()
        .map(|c| c.doc(include_internal))
        .collect();
    docs.push(relational_db_doc(include_internal));
    docs.sort_by(|a, b| a.namespace.cmp(&b.namespace));
    docs
}

pub fn capability_doc(namespace: &str, include_internal: bool) -> Result<CapabilityDoc> {
    if namespace == "relational_db" {
        return Ok(relational_db_doc(include_internal));
    }
    let registry = default_registry();
    Ok(registry.get(namespace)?.doc(include_internal))
}

fn relational_db_doc(include_internal: bool) -> CapabilityDoc {
    let resource_methods = vec![
        resource_method(
            "defineTable",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param(
                    "specJson",
                    "Table specification JSON.",
                    "table_spec.schema.json",
                ),
            ],
            "Create or evolve a table definition.",
        ),
        resource_method(
            "dropTable",
            "write",
            &[param(
                "table",
                "Stable table name.",
                "table_name.schema.json",
            )],
            "Drop a table and its rows.",
        ),
        resource_method(
            "insert",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("rowJson", "Row JSON.", "row.schema.json"),
            ],
            "Insert or replace one row.",
        ),
        resource_method(
            "patch",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("id", "Primary key value.", ""),
                param("patchJson", "Partial row JSON.", "patch.schema.json"),
            ],
            "Patch one row by primary key.",
        ),
        resource_method(
            "delete",
            "write",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("id", "Primary key value.", ""),
            ],
            "Delete one row by primary key.",
        ),
        resource_method(
            "get",
            "read",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("id", "Primary key value.", ""),
            ],
            "Read one row by primary key.",
        ),
        resource_method(
            "query",
            "read",
            &[
                param("table", "Stable table name.", "table_name.schema.json"),
                param("queryJson", "Structured query JSON.", "query.schema.json"),
            ],
            "Run a structured query without exposing raw SQL.",
        ),
        resource_method("tables", "read", &[], "List table names."),
        resource_method(
            "describeTable",
            "read",
            &[param(
                "table",
                "Stable table name.",
                "table_name.schema.json",
            )],
            "Return a table specification.",
        ),
    ];
    CapabilityDoc {
        namespace: "relational_db".to_string(),
        title: "Relational DB".to_string(),
        summary: "Planned typed table storage and structured queries for app-owned data."
            .to_string(),
        status: "planned".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "relational_db.tableCreate".to_string(),
                "relational_db.tableDrop".to_string(),
                "relational_db.rowInsert".to_string(),
                "relational_db.rowPatch".to_string(),
                "relational_db.rowDelete".to_string(),
                "relational_db.query".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "relational_db.table.created".to_string(),
                "relational_db.table.dropped".to_string(),
                "relational_db.row.upserted".to_string(),
                "relational_db.row.deleted".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods.clone(),
        },
        resources: vec![ResourceDoc {
            namespace: "relational_db".to_string(),
            summary: "App-scoped relational tables planned to be backed by kv records."
                .to_string(),
            methods: resource_methods,
        }],
        schemas: relational_db_schemas(),
        examples: vec![
            ExampleDoc {
                title: "Create a tasks table".to_string(),
                summary: "Define a table with a string primary key and typed fields.".to_string(),
                language: "js".to_string(),
                code: r#"ctx.resource.relational_db.defineTable("tasks", JSON.stringify({
  primaryKey: "id",
  fields: {
    id: { type: "string", required: true },
    title: { type: "string", required: true },
    done: { type: "boolean", required: true, default: false }
  }
}));"#
                    .to_string(),
                expected: "table created".to_string(),
            },
            ExampleDoc {
                title: "Query incomplete tasks".to_string(),
                summary: "Use the structured query subset instead of raw SQL.".to_string(),
                language: "js".to_string(),
                code: r#"ctx.resource.relational_db.query("tasks", JSON.stringify({
  where: { field: "done", op: "eq", value: false },
  orderBy: [{ field: "title", direction: "asc" }],
  limit: 100
}));"#
                    .to_string(),
                expected: r#"[{"id":"task_1","title":"Draft plan","done":false}]"#.to_string(),
            },
        ],
        constraints: vec![
            "Raw SQL is never exposed.".to_string(),
            "Table and field names must be portable identifiers.".to_string(),
            "Tables and rows are app-scoped.".to_string(),
            "Writes must be recorded as deterministic events.".to_string(),
            "Reads are derived from folded state and are not recorded.".to_string(),
            "Migrations are limited to additive fields, field deprecation, and indexes until a migration engine lands.".to_string(),
        ],
        limits: vec![
            limit("maxTablesPerApp", "64", "Keeps generated apps within local-first bounds."),
            limit("maxFieldsPerTable", "128", "Bounds schema validation and generated docs."),
            limit("maxIndexesPerTable", "16", "Bounds query planning metadata."),
            limit("maxQueryLimit", "1000", "Prevents unbounded agent/app reads."),
            limit("maxRowBytes", "65536", "Keeps row payloads practical for local sync."),
            limit("maxSpecBytes", "65536", "Keeps table specs reviewable and portable."),
        ],
        compatibility: vec![
            "This planned doc is exposed before runtime injection; generated apps must check that the runtime actually grants the resource before calling it.".to_string(),
            "The public docs describe relational behavior, not the reserved kv backing layout.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Reserved kv layout".to_string(),
                body: "Reserved keys will use implementation-owned schema and row prefixes. These keys are hidden from app-facing docs unless includeInternal=true.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn resource_method(
    name: &str,
    kind: &str,
    params: &[ParamDoc],
    summary: &str,
) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params: params.to_vec(),
        returns: String::new(),
        summary: summary.to_string(),
        errors: vec![
            "invalid input".to_string(),
            "unknown table".to_string(),
            "unsupported migration".to_string(),
        ],
    }
}

fn param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: true,
        schema_ref: schema_ref.to_string(),
    }
}

fn limit(name: &str, value: &str, reason: &str) -> LimitDoc {
    LimitDoc {
        name: name.to_string(),
        value: value.to_string(),
        reason: reason.to_string(),
    }
}

fn schema(id: &str, title: &str, schema_json: &str) -> SchemaDoc {
    SchemaDoc {
        id: id.to_string(),
        title: title.to_string(),
        media_type: "application/schema+json".to_string(),
        schema_json: schema_json.to_string(),
        public: true,
    }
}

fn relational_db_schemas() -> Vec<SchemaDoc> {
    vec![
        schema(
            "table_name.schema.json",
            "Table name",
            r#"{"$id":"table_name.schema.json","type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"}"#,
        ),
        schema(
            "table_spec.schema.json",
            "TableSpec",
            r#"{"$id":"table_spec.schema.json","type":"object","required":["primaryKey","fields"],"additionalProperties":false,"properties":{"primaryKey":{"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"},"fields":{"type":"object","minProperties":1,"additionalProperties":{"type":"object","required":["type"],"additionalProperties":false,"properties":{"type":{"type":"string","enum":["string","number","boolean","json"]},"required":{"type":"boolean"},"default":{},"indexed":{"type":"boolean"},"unique":{"type":"boolean"},"deprecated":{"type":"boolean"}}}},"indexes":{"type":"array","items":{"type":"object","required":["name","fields"],"additionalProperties":false,"properties":{"name":{"type":"string","pattern":"^[A-Za-z_][A-Za-z0-9_]*$"},"fields":{"type":"array","items":{"type":"string"},"minItems":1},"unique":{"type":"boolean"}}}}}}"#,
        ),
        schema(
            "row.schema.json",
            "Row",
            r#"{"$id":"row.schema.json","type":"object","minProperties":1}"#,
        ),
        schema(
            "patch.schema.json",
            "Patch",
            r#"{"$id":"patch.schema.json","type":"object","minProperties":1}"#,
        ),
        schema(
            "query.schema.json",
            "Query",
            r##"{"$id":"query.schema.json","type":"object","additionalProperties":false,"properties":{"where":{"$ref":"#/$defs/filter"},"orderBy":{"type":"array","items":{"type":"object","required":["field"],"additionalProperties":false,"properties":{"field":{"type":"string"},"direction":{"type":"string","enum":["asc","desc"]}}}},"limit":{"type":"integer","minimum":0,"maximum":1000},"offset":{"type":"integer","minimum":0}},"$defs":{"filter":{"oneOf":[{"type":"object","required":["field","op","value"],"additionalProperties":false,"properties":{"field":{"type":"string"},"op":{"type":"string","enum":["eq","ne","lt","lte","gt","gte","in","contains"]},"value":{}}},{"type":"object","required":["and"],"additionalProperties":false,"properties":{"and":{"type":"array","items":{"$ref":"#/$defs/filter"},"minItems":1}}},{"type":"object","required":["or"],"additionalProperties":false,"properties":{"or":{"type":"array","items":{"$ref":"#/$defs/filter"},"minItems":1}}}]}}}"##,
        ),
        schema(
            "query_result.schema.json",
            "QueryResult",
            r#"{"$id":"query_result.schema.json","type":"array","items":{"type":"object"}}"#,
        ),
    ]
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
    kv_storage_plan: cap::kv::KvStoragePlan,
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
        let kv_storage_plan = cap::kv::storage_plan(&state)?;
        let storage_home = storage_home(&log_path);
        cap::kv::sync_full_storage(&storage_home, &state.kv)?;
        Ok(Core {
            log_path,
            state,
            kv_storage_plan,
            runner,
            registry,
            last_output: None,
        })
    }

    /// The current world. Reads go through here.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Core-facing storage projection plan owned by the `kv` capability.
    pub fn kv_storage_plan(&self) -> &cap::kv::KvStoragePlan {
        &self.kv_storage_plan
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

        let bus = RegistryBus::new(&self.registry, &self.state);
        let ctx = CommandCtx {
            state: &self.state,
            bus: &bus,
        };
        let decision = self
            .registry
            .get(namespace)?
            .decide(ctx, &request.name, &request.args)?;
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
        let app = arg(args, 0, "app")?;
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

        let result = host_runtime::run(&app, &input, &source, self.state.clone())?;
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
        let before_kv = self.state.kv.clone();
        self.append(&records)?;
        for record in &records {
            apply(&self.registry, &mut self.state, record)?;
        }
        self.kv_storage_plan = cap::kv::storage_plan(&self.state)?;
        cap::kv::sync_storage_after_commit(
            &storage_home(&self.log_path),
            &before_kv,
            &self.state.kv,
        )?;
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

fn storage_home(log_path: &Path) -> PathBuf {
    log_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}
