//! [`WorkspaceCore`]: the command/event facade that wires the entire M0a spine.
//!
//! prd-merged/01 CR-A1..A5 (Command / Event / Response; every command carries an
//! [`ActorContext`] and passes policy before touching state) + prd-merged/04
//! P-04 command catalog (the string command names in `forge/spec/commands.md`).
//!
//! A single [`handle`](WorkspaceCore::handle) turns a [`CoreCommand`] into a
//! [`CoreResponse`], driving the jewel end-to-end:
//!
//!   `applet.install` (TS → SWC transpile + policy scan → store) and
//!   `runtime.run` (QuickJS → capability-checked `ctx` → SQLite write → UI patch
//!   → recorded [`RunRecord`]) and `runtime.replay` (deterministic re-execution,
//!   asserted byte-identical) and `query.execute` (read the records projection).
//!
//! The code-hash that flows through this facade is the single canonical
//! `forge_domain::code_hash` (`sha256:`): the pipeline computes it over the
//! transpiled JS, and the runtime records exactly that hash on the
//! [`RunRecord`] (it hashes the same `js_code` bytes the pipeline produced), so a
//! stored run's `code_hash` is provably the pipeline's hash — the TS → SWC → run
//! provenance chain (reviews 010/012/013/014).

use crate::bridge::StorageHostBridge;
use crate::event::EventSink;
use forge_domain::{
    AppletId, CoreCommand, CoreError, CoreResponse, Manifest, Result, Role, RunId, RunRecord,
};
use forge_runtime::{record_run, replay, NullBridge, Program as RuntimeProgram};
use forge_schema::{CollectionDef, FieldDef, FieldType, SchemaChange, SchemaRegistry};
use forge_signing::{verify_package, Package, PublisherTrust, TrustOutcome};
use forge_storage::{
    CreateIndexKind, ExportOptions, IndexDef, IndexManager, IndexState, RunLogPolicy, Store,
    EXPORT_FORMAT_VERSION,
};

/// Reserved KV namespace prefix for core-owned metadata (applet manifests +
/// compiled programs + workspace meta). Applet `ctx.storage` namespaces are
/// `applet/<id>` (see [`StorageHostBridge`]), which never collide with this
/// `__forge/...` prefix.
const META_NS: &str = "__forge/meta";

/// The KV key (within [`META_NS`]) holding the workspace's monotone run counter.
/// Bumped once per `runtime.run` to mint a unique per-execution `run_id` while
/// the replay *seeds* stay a deterministic function of `(code_hash, input)`
/// (review 031 finding 2 / CR-9 "every execution persists").
const RUN_COUNTER_KEY: &str = "run_counter";

/// The KV key (within [`META_NS`]) holding the persisted trusted `db.read` grant
/// table (actor id → readable collections). Persisted so a scoped grant survives
/// reopening the workspace file instead of fail-opening to read-all (review 050).
const DB_READ_GRANTS_KEY: &str = "db_read_grants";

/// The KV key (within [`META_NS`]) holding the persisted [`SchemaRegistry`]
/// (serialized JSON). The dynamic schema is workspace state (DL-7/DL-8): a
/// collection/field defined via `schema.apply_change` must survive reopening the
/// workspace file, so the registry is loaded on [`WorkspaceCore::open`] /
/// [`in_memory`](WorkspaceCore::in_memory) the same way the `db.read` grant table
/// is (it mirrors [`load_db_read_grants`]).
const SCHEMA_REGISTRY_KEY: &str = "schema_registry";

/// The compiled, installed form of an applet: its manifest plus the transpiled
/// JS the runtime executes and the canonical `code_hash` the pipeline produced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InstalledApplet {
    manifest: Manifest,
    /// Transpiled ES-module JavaScript (the runtime's `Program.source`).
    js_code: String,
    /// `forge_domain::code_hash(js_code)` — the provenance + replay key.
    code_hash: String,
    /// Monotone install version (bumps on re-install/upgrade).
    version: u32,
    /// The signing/trust result recorded at install time (SC-15 / MP-4). An
    /// install that carried a verified Ed25519 package records the verified
    /// publisher + key id here so a later command can report the package's trust;
    /// an install with no signature records [`InstallTrust::Unsigned`]. Older
    /// records (installed before signing) deserialize to `Unsigned` via the serde
    /// default, so the field is backward-compatible with the existing meta store.
    #[serde(default)]
    trust: InstallTrust,
}

/// The signing/trust provenance recorded for an installed applet (SC-15 / MP-4).
///
/// M0a is *signing-ready, not mandatory*: an install MAY carry an Ed25519-signed
/// package, in which case the platform VERIFIES it before trusting/installing and
/// records the [`Signed`](InstallTrust::Signed) result; an install with no
/// signature proceeds [`Unsigned`](InstallTrust::Unsigned). A failed verification
/// never lands here — the install is rejected before any record is written.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum InstallTrust {
    /// No signature accompanied the install (the M0a default; allowed because
    /// signing is not yet mandatory). The install response surfaces `unsigned`.
    #[default]
    Unsigned,
    /// The install carried an Ed25519-signed package whose signature verified
    /// over the canonical `terrane/sig/v1` preimage and whose live files/manifest
    /// still match the signed hashes. Records the verified publisher identity so a
    /// later command can report the package's trust.
    Signed {
        /// The verified publisher id (`manifest.publisher` in the signed package),
        /// when the package declared one.
        publisher: Option<String>,
        /// The signing key id (`manifest.keyId`) the package was signed under.
        key_id: Option<String>,
        /// Whether the marketplace-policy trust layer (publisher trust set) was
        /// also enforced for this install (`true`) or skipped — the M0a default of
        /// crypto + integrity only (`false`).
        publisher_trust_enforced: bool,
    },
}

impl InstallTrust {
    /// A compact JSON view of the trust result for the install response + meta.
    /// `Unsigned` surfaces `{ "status": "unsigned" }`; `Signed` surfaces the
    /// verified publisher / key id so a shell can report the package's trust
    /// without re-reading the stored applet.
    fn to_json(&self) -> serde_json::Value {
        match self {
            InstallTrust::Unsigned => serde_json::json!({ "status": "unsigned" }),
            InstallTrust::Signed {
                publisher,
                key_id,
                publisher_trust_enforced,
            } => serde_json::json!({
                "status": "signed",
                "publisher": publisher,
                "key_id": key_id,
                "publisher_trust_enforced": publisher_trust_enforced,
            }),
        }
    }
}

/// The workspace facade. Owns the SQLite [`Store`], a [`SchemaRegistry`], and an
/// [`EventSink`]; a [`forge_runtime::QuickJsEngine`] is constructed per run
/// inside the runtime's `record_run`/`replay`.
pub struct WorkspaceCore {
    store: Store,
    /// The dynamic schema registry (DL-7/DL-8). Loaded from `__forge/meta`
    /// (`schema_registry`) on open so a defined collection/field survives reopen,
    /// and re-persisted after every accepted `schema.apply_change` (mirrors the
    /// `db.read` grant table). The schema crate owns every additive/compatibility
    /// rule; this facade only exposes + persists it.
    registry: SchemaRegistry,
    /// The dynamic-index manager (DL-5). In-memory metadata that decides physical
    /// index DDL and planner eligibility; the physical structures live in the
    /// SQLite file. Reconstructed on open from the registry's `indexed` fields
    /// (DL-8 → DL-5: a field marked `indexed` owns a storage index), so a
    /// `schema.apply_change` that minted an indexed field keeps that index after
    /// reopen.
    indexes: IndexManager,
    events: EventSink,
    workspace_id: String,
    /// Trusted `db.read` grant table: actor id → the collections that actor may
    /// read (`"*"` = read-all). This is the SOURCE OF TRUTH for the
    /// collection-scoped `db.read` capability and is set only through the trusted
    /// [`grant_db_read`](WorkspaceCore::grant_db_read) seam (workspace
    /// configuration / membership), never from a request payload (review 048
    /// finding 1). An actor with NO entry falls back to its role-derived read
    /// scope, so the existing owner-permits-all spine is unaffected.
    db_read_grants: std::collections::BTreeMap<String, Vec<String>>,
    /// Factory for the `ctx.net.fetch` [`HttpClient`](forge_runtime::HttpClient)
    /// (prd-merged/07 SC-5, prd-merged/01 CR-3 `net`). Each `runtime.run` builds a
    /// fresh client from this factory and hands it to the run's
    /// [`StorageHostBridge`]. The default factory yields a
    /// [`NoNetworkClient`](crate::bridge::NoNetworkClient) — so CI, the demo, and
    /// any caller that does not opt in are network-free and fail closed
    /// (`PlatformUnavailable`). A host/shell injects a real client via
    /// [`set_http_client_factory`](WorkspaceCore::set_http_client_factory); tests
    /// inject a mock. A factory (rather than one shared client) is used because an
    /// [`HttpClient`](forge_runtime::HttpClient) trait object is not `Clone`, and
    /// each run needs its own bridge-owned client.
    http_client_factory: HttpClientFactory,
    /// Factory for the `ctx.net.fetch` secret store (prd-merged/07 SC-13). Each
    /// `runtime.run` builds a fresh [`SecretStore`](forge_runtime::SecretStore)
    /// from this factory and hands it to the run's [`StorageHostBridge`], so the
    /// host can inject a `secret_ref` header's resolved value at the HTTP edge.
    /// The default factory yields an EMPTY in-memory store — so any secret_ref
    /// fails closed until a host/shell injects a real (OS-keychain-backed) store
    /// via [`set_secret_store_factory`](WorkspaceCore::set_secret_store_factory).
    /// A factory (not one shared store) is used because the run's bridge owns its
    /// store and the trait object is not `Clone`.
    secret_store_factory: SecretStoreFactory,
}

/// A factory that produces a fresh `ctx.net.fetch`
/// [`HttpClient`](forge_runtime::HttpClient) per run. See
/// [`WorkspaceCore::set_http_client_factory`].
type HttpClientFactory = Box<dyn Fn() -> Box<dyn forge_runtime::HttpClient>>;

/// A factory that produces a fresh `ctx.net.fetch`
/// [`SecretStore`](forge_runtime::SecretStore) per run. See
/// [`WorkspaceCore::set_secret_store_factory`].
type SecretStoreFactory = Box<dyn Fn() -> Box<dyn forge_runtime::SecretStore>>;

impl WorkspaceCore {
    /// Open (or create) a file-backed workspace at `path` (`workspace.open`
    /// semantics; the single portable SQLite file, DECISIONS E1).
    pub fn open(path: impl AsRef<std::path::Path>, workspace_id: impl Into<String>) -> Result<Self> {
        let store = Store::open(path)?;
        Self::from_store(store, workspace_id)
    }

    /// Open an in-memory workspace (tests/scratch).
    pub fn in_memory(workspace_id: impl Into<String>) -> Result<Self> {
        let store = Store::open_in_memory()?;
        Self::from_store(store, workspace_id)
    }

    /// Build a [`WorkspaceCore`] over an already-opened [`Store`], loading the
    /// persisted workspace state from the file: the `db.read` grant table (review
    /// 050) and the dynamic [`SchemaRegistry`] (DL-7/DL-8), then reconstructing the
    /// dynamic-index manager from the registry's `indexed` fields (DL-8 → DL-5).
    /// Shared by [`open`](Self::open) / [`in_memory`](Self::in_memory) so every
    /// entry point loads identical state.
    fn from_store(store: Store, workspace_id: impl Into<String>) -> Result<Self> {
        let db_read_grants = load_db_read_grants(&store)?;
        let registry = load_schema_registry(&store)?;
        let indexes = rebuild_indexes_from_registry(&store, &registry)?;
        Ok(WorkspaceCore {
            store,
            registry,
            indexes,
            events: EventSink::new(),
            workspace_id: workspace_id.into(),
            db_read_grants,
            // Fail-closed default: no live network. A host/shell opts in by
            // calling `set_http_client_factory` (review: keep the network seam
            // injectable so CI/the demo never reach the network).
            http_client_factory: Box::new(|| Box::new(crate::bridge::NoNetworkClient)),
            // Fail-closed default: an EMPTY secret store (every secret_ref denied)
            // until a host/shell injects a real one via `set_secret_store_factory`.
            secret_store_factory: Box::new(|| {
                Box::new(forge_runtime::InMemorySecretStore::new())
            }),
        })
    }

    /// Inject the factory that builds the `ctx.net.fetch`
    /// [`HttpClient`](forge_runtime::HttpClient) for each `runtime.run`
    /// (prd-merged/07 SC-5, prd-merged/01 CR-3 `net`). A host/shell wires its real
    /// (out-of-crate) client here; tests inject a mock. Until this is called the
    /// workspace uses [`NoNetworkClient`](crate::bridge::NoNetworkClient), which
    /// refuses every request with `PlatformUnavailable` — so CI/the demo, which
    /// never set this, stay network-free.
    ///
    /// The factory is invoked once per run so each run's bridge owns its own
    /// client (an [`HttpClient`](forge_runtime::HttpClient) trait object is not
    /// `Clone`). The egress decision is still the runtime's: the injected client is
    /// consulted only for an *allowed*, *record-mode* fetch (replay serves the
    /// recording; a denied fetch never reaches the client).
    pub fn set_http_client_factory(
        &mut self,
        factory: impl Fn() -> Box<dyn forge_runtime::HttpClient> + 'static,
    ) {
        self.http_client_factory = Box::new(factory);
    }

    /// Inject the factory that builds the `ctx.net.fetch`
    /// [`SecretStore`](forge_runtime::SecretStore) for each `runtime.run`
    /// (prd-merged/07 SC-13). A host/shell wires its real OS-keychain-backed store
    /// here; tests inject an in-memory store. Until this is called the workspace
    /// uses an EMPTY in-memory store, so any `secret_ref` header fails closed.
    ///
    /// The factory is invoked once per run so each run's bridge owns its own store
    /// (the trait object is not `Clone`). The store is consulted ONLY for an
    /// allowed, record-mode fetch whose matched net rule allowlists the secret
    /// header — the host resolves + injects the value into the outgoing request
    /// inside the recording closure, so the value never reaches the trace, the
    /// applet, or any log (SC-13); replay serves the recording and needs no store.
    pub fn set_secret_store_factory(
        &mut self,
        factory: impl Fn() -> Box<dyn forge_runtime::SecretStore> + 'static,
    ) {
        self.secret_store_factory = Box::new(factory);
    }

    /// Configure the TRUSTED `db.read` grant scope for `actor` (workspace
    /// membership / capability provisioning). `scope` is the list of collections
    /// the actor may read; `"*"` grants read-all. This is the only way a caller's
    /// `db.read` scope is set — `query.execute` reads it from here, never from the
    /// request payload, so a shell cannot widen its own read scope by editing the
    /// command body (review 048 finding 1). Passing an empty `scope` provisions an
    /// actor that holds the `db.read` role but is granted NO collection.
    ///
    /// The grant table is **persisted** to the workspace file (review 050): a
    /// scoped actor stays scoped after `open(...)`, instead of silently reverting
    /// to role-derived read-all (a fail-open regression).
    pub fn grant_db_read(
        &mut self,
        actor: impl Into<String>,
        scope: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        self.db_read_grants
            .insert(actor.into(), scope.into_iter().map(Into::into).collect());
        let bytes = serde_json::to_vec(&self.db_read_grants)
            .map_err(|e| CoreError::StorageError(format!("serialize db.read grants: {e}")))?;
        self.store
            .kv_set(META_NS, DB_READ_GRANTS_KEY, &bytes, "application/json")?;
        Ok(())
    }

    /// The event sink (observability): all `run.*` / `ui.patch` events land here.
    pub fn events(&self) -> &EventSink {
        &self.events
    }

    /// Mutable access to the event sink (e.g. to drain it between turns).
    pub fn events_mut(&mut self) -> &mut EventSink {
        &mut self.events
    }

    /// The schema registry backing `schema.*` / record validation.
    pub fn registry(&self) -> &SchemaRegistry {
        &self.registry
    }

    /// The dynamic-index manager backing the registry's `indexed` fields (DL-5).
    /// Read-only access for tests / the index-aware query planner.
    pub fn indexes(&self) -> &IndexManager {
        &self.indexes
    }

    /// Borrow the underlying store (read-only access for tests / queries).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Mutable access to the underlying store, for in-process tooling and tests
    /// (e.g. the testkit injecting/​tampering a record to exercise replay
    /// divergence). This is **not** a shell-facing path: the CR-A1 boundary
    /// forbids *shells* from mutating SQLite directly, which they do not get via
    /// the binding layer; in-process callers that hold a `WorkspaceCore` already
    /// own the workspace.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    // ---------------------------------------------------------------- dispatch

    /// Handle one [`CoreCommand`], returning a [`CoreResponse`]
    /// (prd-merged/01 CR-A1/CR-A2). Unknown command names return a
    /// `ValidationError` response rather than panicking (CR-A5 graceful reject).
    ///
    /// Every command carries an [`ActorContext`]; CR-A3 requires that context to
    /// pass policy *before* the command touches state. This happens in two
    /// layers: a **command-level RBAC gate** ([`authorize`]) runs here, before
    /// dispatch, rejecting an actor whose role is not permitted to issue the
    /// command per `forge/spec/commands.md` (e.g. a Viewer cannot
    /// `applet.install`, a Runner cannot `runtime.replay`); then the
    /// **capability gate** is enforced at host-call time inside
    /// `runtime.run`/`replay` (CR-A3 / CR-4), where the applet's manifest
    /// capabilities are checked per `ctx.*` call.
    pub fn handle(&mut self, cmd: CoreCommand) -> CoreResponse {
        let request_id = cmd.request_id.clone();
        let result = authorize(&cmd).and_then(|()| match cmd.name.as_str() {
            "workspace.create" => self.cmd_workspace_create(&cmd),
            "workspace.open" => self.cmd_workspace_open(&cmd),
            "applet.install" => self.cmd_applet_install(&cmd),
            "runtime.run" => self.cmd_runtime_run(&cmd),
            "runtime.replay" => self.cmd_runtime_replay(&cmd),
            "query.execute" => self.cmd_query_execute(&cmd),
            "schema.apply_change" => self.cmd_schema_apply_change(&cmd),
            "schema.validate_compatibility" => self.cmd_schema_validate_compatibility(&cmd),
            "schema.rebuild_indexes" => self.cmd_schema_rebuild_indexes(&cmd),
            "workspace.export" => self.cmd_workspace_export(&cmd),
            "workspace.import" => self.cmd_workspace_import(&cmd),
            other => Err(CoreError::ValidationError(format!(
                "unknown command {other:?} (CR-A5: client should negotiate capability)"
            ))),
        });
        match result {
            Ok(payload) => CoreResponse::ok(request_id, payload),
            Err(error) => CoreResponse::err(request_id, error),
        }
    }

    // ---------------------------------------------------------------- commands

    /// `workspace.create` — in M0a the store is created on open, so this reports
    /// the workspace identity + the base logical version (CR-A2; M0b adds
    /// templates/owner wiring).
    fn cmd_workspace_create(&mut self, _cmd: &CoreCommand) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "root_version": 0,
        }))
    }

    /// `workspace.open` — report workspace metadata + the current logical clock
    /// (CR-A2). The file is already open (this core wraps one workspace file).
    fn cmd_workspace_open(&mut self, _cmd: &CoreCommand) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "logical_clock": self.events.len(),
        }))
    }

    /// `applet.install` — compile each source (static policy scan + SWC
    /// transpile; reject forbidden constructs), validate the manifest, and store
    /// the manifest + transpiled program (CR-A2, CR-13/CR-14, SC-15).
    ///
    /// Payload: `{ applet_id, manifest, sources: { "<path>": "<ts>" }, signature? }`.
    /// The manifest's `entrypoint` selects which source is the runnable program.
    ///
    /// SC-15 / MP-4 — package signing/trust (M0a: *signing-ready, not mandatory*):
    /// the install MAY carry an optional Ed25519-signed package under a
    /// `signature` field (the prd-merged/08 MP-4 package shape
    /// `{ package: { manifest, files, hashes }, signature, public_key,
    /// publisher_trust? }`, identical to the T012 fixtures). When present the
    /// platform VERIFIES it via [`forge_signing::verify_package`] BEFORE trusting
    /// or installing the applet:
    ///
    ///   - a CRYPTO / integrity / policy failure REJECTS the install with
    ///     `ValidationError("package signature invalid: ...")` — nothing is stored;
    ///   - the verified package is BOUND to the install payload (review 080 #1):
    ///     its files must be the same `path -> content` set as `sources`, so a
    ///     valid signature can only bless the exact code being compiled/stored;
    ///   - on success the verified publisher / key id + trust layer is recorded in
    ///     the install metadata ([`InstallTrust::Signed`]) so a later command can
    ///     report the package's trust.
    ///
    /// When NO `signature` is present the install proceeds [`InstallTrust::Unsigned`]
    /// (the M0a default) — the existing demo path is untouched and the response
    /// simply reports `unsigned`. The signature check runs BEFORE compilation so a
    /// tampered/untrusted package never reaches the transpiler or the store.
    fn cmd_applet_install(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let manifest: Manifest = take_field(cmd, "manifest")?;
        manifest.validate()?;

        let sources = cmd
            .payload
            .get("sources")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                CoreError::ValidationError("applet.install requires a `sources` object".into())
            })?;
        if sources.is_empty() {
            return Err(CoreError::ValidationError(
                "applet.install `sources` must not be empty".into(),
            ));
        }

        // SC-15 / MP-4: verify the package signature when one is carried, BEFORE
        // any state is touched, and BIND it to the actual install sources so a
        // valid signature can only bless the exact code being installed (review
        // 080 #1). `Unsigned` when the install carries no signature.
        let trust = verify_install_signature(cmd, sources)?;

        // Compile every source so a forbidden construct in ANY file rejects the
        // whole install (CR-13: the static policy scan is layer one). Capture
        // each compiled program; the entrypoint's program is the runnable one.
        let mut warnings = Vec::new();
        let mut entry_program: Option<forge_pipeline::Program> = None;
        for (path, src) in sources {
            let ts = src.as_str().ok_or_else(|| {
                CoreError::ValidationError(format!("source {path:?} must be a string"))
            })?;
            // compile() runs enforce_policy (PermissionDenied on eval/Function/…)
            // THEN transpiles; a forbidden construct never reaches transpile.
            let program = forge_pipeline::compile(ts)?;
            if path == &manifest.entrypoint {
                entry_program = Some(program);
            }
        }
        let entry_program = entry_program.ok_or_else(|| {
            CoreError::ValidationError(format!(
                "manifest.entrypoint {:?} is not among the provided sources",
                manifest.entrypoint
            ))
        })?;

        // Bump version if re-installing.
        let version = self
            .load_applet(applet_id.as_str())
            .ok()
            .flatten()
            .map(|a| a.version + 1)
            .unwrap_or(1);

        let installed = InstalledApplet {
            manifest,
            js_code: entry_program.js_code,
            code_hash: entry_program.code_hash,
            version,
            trust: trust.clone(),
        };
        self.store_applet(applet_id.as_str(), &installed)?;

        if sources.len() > 1 {
            warnings.push(format!(
                "{} non-entrypoint source(s) compiled but only the entrypoint is runnable in M0a",
                sources.len() - 1
            ));
        }

        self.events.emit(
            Some(applet_id.clone()),
            "applet.installed",
            serde_json::json!({
                "applet_id": applet_id,
                "version": version,
                "trust": trust.to_json(),
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "version": version,
            "code_hash": installed.code_hash,
            "warnings": warnings,
            // SC-15: the verified trust result for this install — `unsigned`, or
            // `signed` with the verified publisher / key id (the package passed
            // crypto + integrity, and the policy layer when enforced).
            "trust": trust.to_json(),
        }))
    }

    /// `runtime.run` — load the compiled program, run it via the QuickJS engine
    /// in record mode with a [`StorageHostBridge`] + the applet's policy
    /// (manifest capabilities), save the [`RunRecord`], emit
    /// `run.started`/`ui.patch`/`run.completed`, and return the run summary +
    /// `AppResult` (CR-A2, CR-8, CR-9).
    ///
    /// Payload: `{ applet_id, input, random_seed?, time_start? }`.
    ///
    /// `random_seed`/`time_start` are **optional** deterministic-seam overrides
    /// (review 032 finding 1). When present they pin the run's RNG/clock seeds to
    /// exact values — the conformance corpus uses this to drive a scenario
    /// recorded under specific seeds (e.g. `seeded_random` under seed 7) through
    /// this facade rather than a parallel direct path. When absent the seeds are a
    /// deterministic function of `(code_hash, input)` (the default that makes
    /// independent re-runs of the demo replay identically; review 031 finding 2).
    fn cmd_runtime_run(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let input = cmd.payload.get("input").cloned().unwrap_or(serde_json::Value::Null);
        let seed_override = run_seed_override(cmd)?;
        let installed = self
            .load_applet(applet_id.as_str())?
            .ok_or_else(|| {
                CoreError::ValidationError(format!("applet {applet_id} is not installed"))
            })?;

        self.events.emit(
            Some(applet_id.clone()),
            "run.started",
            serde_json::json!({ "applet_id": applet_id, "code_hash": installed.code_hash }),
        );

        let program = RuntimeProgram::new(applet_id.clone(), installed.js_code.clone());
        // Sanity: the runtime hashes the SAME js bytes the pipeline did, so its
        // recorded code_hash equals the pipeline's. Asserted in the integration
        // test; here we surface a clear error if that invariant ever breaks.
        if program.code_hash() != installed.code_hash {
            return Err(CoreError::RuntimeError(format!(
                "code_hash provenance broken: runtime {} != pipeline {}",
                program.code_hash(),
                installed.code_hash
            )));
        }

        // Replay seeds are a deterministic function of (code_hash, input): a
        // re-run with the SAME applet code and input reproduces the SAME seeded
        // time/random seams, so the two records replay byte-identically. A
        // different input yields different seeds (a genuinely different run).
        // An explicit `(random_seed, time_start)` in the payload overrides this
        // default so a conformance scenario recorded under fixed seeds can be
        // reproduced through the facade (review 032 finding 1). Either way the
        // chosen seeds are recorded on the run, so replay stays byte-identical.
        let (random_seed, time_start) =
            seed_override.unwrap_or_else(|| derive_seeds(&installed.code_hash, &input));

        // Mint a UNIQUE per-execution run id (review 031 finding 2 / CR-9). The
        // counter is persisted in workspace meta so each execution is saved and
        // loadable separately, even when code+input (and therefore the seeds)
        // match a prior run. This bump happens before the run so the id is
        // reserved even if the run itself fails.
        let invocation = self.next_run_counter()?;

        // Build this run's `ctx.net.fetch` client from the injected factory BEFORE
        // borrowing the store mutably for the bridge (the factory closure borrows
        // `&self`; the bridge borrows `&mut self.store`). Default = NoNetworkClient
        // (fail-closed, no live network) unless a host/shell injected a real client
        // via `set_http_client_factory`.
        let http_client = (self.http_client_factory)();
        // Build this run's secret store from the injected factory too (SC-13): the
        // host resolves a `secret_ref` header's value against it at the HTTP edge,
        // inside the runtime's recording closure. Default = empty (fail-closed).
        let secret_store = (self.secret_store_factory)();

        // Run in record mode against the live Store-backed bridge. The bridge
        // performs the SQLite writes / UI diffs; the runtime's HostContext gates
        // each ctx.* call against the manifest policy BEFORE the bridge runs —
        // including the SC-5 net egress check, so a denied ctx.net.fetch never
        // reaches the injected client.
        let mut bridge = StorageHostBridge::with_http_client(
            &mut self.store,
            applet_id.as_str(),
            http_client,
        )
        .with_secret_store(secret_store);
        let mut run = record_run(
            &program,
            &installed.manifest,
            &cmd.actor,
            &input,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // Drain the captured UI renders + logs before dropping the bridge (which
        // releases the &mut Store borrow so we can save the run).
        let ui_renders = std::mem::take(&mut bridge.ui_renders);
        drop(bridge);

        // Override the runtime's deterministic run_id with the unique per-core
        // identity so two executions of the same applet+input persist as two
        // distinct, independently loadable records (the seeds/trace are
        // unchanged, so each still replays identically to itself).
        run.run_id = unique_run_id(&run.code_hash, invocation);

        // Pin the replay artifact PER RUN: persist the EXACT compiled program +
        // manifest this run executed, keyed by `run_id`, so replay reconstructs
        // the program AND the manifest (engine limits/capabilities) from what this
        // run actually used — never from whatever is installed now (review 031
        // finding 3 / review 036 finding 2; CR-9 version-pinned replay).
        //
        // Review 036 finding 2: the prior pin was keyed by `code_hash` alone, so
        // reinstalling the SAME JS under a different manifest (tighter `limits`,
        // changed legacy caps) overwrote `program/<code_hash>` and stranded older
        // runs' context — replay then used the new manifest's engine limits. The
        // per-run key is unique to this execution, so no reinstall (same code or
        // not) can overwrite it. The content-addressed `program/<code_hash>` pin
        // is kept as a fallback for legacy runs recorded before per-run pinning —
        // and is now WRITE-ONCE (review 038 finding 3), so even a same-JS reinstall
        // under a different manifest can no longer overwrite the fallback a legacy
        // run depends on.
        self.store_run_program(run.run_id.as_str(), &installed)?;
        self.store_program(&installed)?;

        // Persist the deterministic run record (replay source, CR-9). save_run
        // re-validates the canonical code_hash.
        self.store.save_run(&run)?;

        // Emit a ui.patch event per render (the UI tree patch link).
        for (i, render) in ui_renders.iter().enumerate() {
            self.events.emit(
                Some(applet_id.clone()),
                "ui.patch",
                serde_json::json!({
                    "applet_id": applet_id,
                    "render_index": i,
                    "tree": render.tree,
                    "patches": render.patches,
                }),
            );
        }

        let summary = run_summary(&run);
        let (ok, app_result) = outcome_fields(&run);
        let event_kind = if ok { "run.completed" } else { "run.failed" };
        self.events.emit(
            Some(applet_id.clone()),
            event_kind,
            serde_json::json!({ "run_id": run.run_id, "ok": ok }),
        );

        Ok(serde_json::json!({
            "run_id": run.run_id,
            "code_hash": run.code_hash,
            "ok": ok,
            "result": app_result,
            "summary": summary,
            // The ordered host-call method trace the run issued (`db.insert`,
            // `ui.render`, …). Surfaced so a shell / conformance harness can
            // assert the exact effect sequence through the facade rather than
            // re-reading the persisted RunRecord (review 032 finding 1).
            "host_call_methods": run.calls.iter().map(|c| c.method.clone()).collect::<Vec<_>>(),
            "ui_renders": ui_renders.iter().map(|r| r.tree.clone()).collect::<Vec<_>>(),
        }))
    }

    /// `runtime.replay` — load the stored [`RunRecord`], replay it deterministically
    /// (the recorder serves recorded responses; the live bridge is a
    /// [`NullBridge`] that must never be consulted), and assert the replay is
    /// byte-identical to the original (CR-A2, CR-9). Divergence → `RuntimeError`.
    ///
    /// Payload: `{ run_id }`.
    fn cmd_runtime_replay(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let run_id = cmd
            .payload
            .get("run_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::ValidationError("runtime.replay requires `run_id`".into()))?
            .to_string();

        let original = self
            .store
            .load_run(&run_id)?
            .ok_or_else(|| CoreError::ValidationError(format!("run {run_id} not found")))?;

        // Version-pinned replay (review 031 finding 3, review 036 finding 2):
        // reconstruct the program + manifest from the artifact recorded for THIS
        // execution, not the currently installed applet. Resolution order:
        //   1. the PER-RUN pin (`program/run/<run_id>`) — unique to this run, so a
        //      reinstall under a different manifest cannot overwrite or alter it
        //      (the review 036 finding 2 case);
        //   2. the content-addressed `program/<code_hash>` pin — covers runs
        //      recorded before per-run pinning existed;
        //   3. the currently installed applet — last-resort legacy fallback, and
        //      only if its code_hash still matches the recorded one.
        let replay_artifact = match self.load_run_program(&run_id)? {
            Some(p) => p,
            None => match self.load_program(&original.code_hash)? {
                Some(p) => p,
                None => {
                    let installed =
                        self.load_applet(original.applet_id.as_str())?.ok_or_else(|| {
                            CoreError::ValidationError(format!(
                                "no recorded program for run {run_id} (code_hash {}) and applet {} is not installed; cannot replay",
                                original.code_hash, original.applet_id
                            ))
                        })?;
                    if installed.code_hash != original.code_hash {
                        return Err(CoreError::ValidationError(format!(
                            "no recorded program for run {run_id}; installed applet {} is a different version (code_hash {} != recorded {}); cannot replay",
                            original.applet_id, installed.code_hash, original.code_hash
                        )));
                    }
                    installed
                }
            },
        };
        let installed = replay_artifact;

        let program = RuntimeProgram::new(original.applet_id.clone(), installed.js_code.clone());
        let mut null = NullBridge::new();
        let replayed = replay(&original, &program, &installed.manifest, &cmd.actor, &mut null)?;

        // The strict replay check: canonical provenance on both records AND
        // byte-identical traces, surfaced as a RuntimeError on divergence.
        original.assert_replay_of(&replayed)?;

        self.events.emit(
            Some(original.applet_id.clone()),
            "run.replayed",
            serde_json::json!({ "run_id": run_id, "ok": true }),
        );

        Ok(serde_json::json!({
            "ok": true,
            "run_id": run_id,
            "fingerprint": replayed.replay_fingerprint(),
            "replays_identically": original.replays_identically(&replayed),
        }))
    }

    /// `query.execute` — list every record in `collection` from the projection
    /// (CR-A2, DL-15 subset). Payload: `{ collection, grants? }`.
    ///
    /// `forge/spec/commands.md:21` requires **"Role plus db.read capability"**,
    /// and `forge/spec/capabilities.md:23` models `db.read` as a *collection-scoped*
    /// grant (`resource: collection:<name>`). Two independent layers gate the read
    /// (review 036/038 finding 1):
    ///
    ///   1. the command-level [`authorize`] role gate (a `Runner` is
    ///      execution-only and cannot read data) — `PermissionDenied`; then
    ///   2. the **collection-scoped `db.read` capability** ([`require_db_read`]):
    ///      the target `collection` must fall within the caller's granted
    ///      `db.read` scope (`payload.grants.db.read`, the same grant shape the
    ///      `forge/fixtures/query/reject_ungranted_collection.json` vector pins).
    ///      A collection outside the granted scope is `CapabilityRequired` —
    ///      enforced **before** `list_records` touches state — even for a role that
    ///      cleared layer 1. This is the caller boundary `forge-storage` defers to
    ///      (the projection scans any collection unguarded; the grant lives here).
    fn cmd_query_execute(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let collection = cmd
            .payload
            .get("collection")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError("query.execute requires `collection`".into())
            })?;
        // Capability gate (CR-A3 / DL-15): role first, then the collection-scoped
        // db.read grant. The grant scope is read from the workspace's TRUSTED grant
        // table (keyed by the actor), never from the request payload (review 048
        // finding 1), so a caller cannot widen its own read scope. Denied before
        // any projection is read.
        let trusted_scope = self.db_read_grants.get(cmd.actor.actor.as_str()).cloned();
        require_db_read(cmd, collection, trusted_scope.as_deref())?;
        let records = self.store.list_records(collection)?;
        let rows: Vec<serde_json::Value> = records
            .into_iter()
            .map(|env| {
                serde_json::json!({
                    "id": env.entity_id,
                    "fields": env.fields,
                })
            })
            .collect();
        Ok(serde_json::json!({ "collection": collection, "rows": rows }))
    }

    // ------------------------------------------------------- schema.* (DL-7/8)

    /// `schema.apply_change` — apply one additive [`SchemaChange`] to the dynamic
    /// registry, persist the new registry, and return the new collection/registry
    /// summary (CR-A2; `forge/spec/commands.md`: Owner/Maintainer, DL-8).
    ///
    /// Payload: `{ change }` — a serialized [`SchemaChange`] (the `op`-tagged
    /// snake_case shape the schema crate defines). The schema crate is the
    /// authority: it mints stable actor-scoped field ids (DL-7), enforces
    /// additive-only evolution, and **rejects** a destructive/incompatible change
    /// with [`CoreError::SchemaCompatibilityError`] (e.g. re-adding a collection,
    /// duplicate field name, narrowing a type) — we surface that verbatim and the
    /// registry is left unchanged (we only persist on success).
    ///
    /// DL-8 → DL-5: when an `add_field` marks the field `indexed`, we CREATE the
    /// corresponding storage index over the field's freshly minted **stable**
    /// `field_id` ([`Store::create_index`]) so the dynamic index follows the
    /// schema. A `Text` field gets an FTS5 shadow table; any other type gets a
    /// JSON1 expression (`Value`) index.
    fn cmd_schema_apply_change(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let change: SchemaChange = take_field(cmd, "change")?;

        // Apply to a COPY first so a rejected change leaves the live registry (and
        // therefore the persisted state) untouched. The schema crate returns the
        // SchemaCompatibilityError for destructive/incompatible changes.
        let mut next = self.registry.clone();
        next.apply_change(change.clone())?;

        // DL-8 → DL-5: a newly added `indexed` field gets its storage index built
        // over the stable field id the schema crate just minted.
        //
        // Create the index BEFORE persisting/swapping the registry (review 066): a
        // schema-minted field id interpolates the actor id (`f_<actor>_<seq>`), and
        // an actor id with characters outside the storage identifier charset (e.g.
        // `alice@example.com`) makes `create_index` fail. If we persisted first, the
        // rejected change would still be on disk and `rebuild_indexes_from_registry`
        // would fail on EVERY future open — poisoning the workspace. By creating the
        // index against the candidate registry first, an invalid field id rejects the
        // whole `apply_change` (`QueryError`) with the live + persisted registry
        // untouched.
        let mut created_index: Option<String> = None;
        if let SchemaChange::AddField { collection, indexed: true, .. } = &change {
            if let Some((field_id, kind)) = indexed_field_to_create(&next, collection) {
                let id = self.store.create_index(
                    &mut self.indexes,
                    collection,
                    &field_id,
                    kind,
                )?;
                created_index = Some(id);
            }
        }

        // The index (if any) was created successfully — now durably commit the
        // evolved registry. Persist BEFORE swapping the in-memory copy so the
        // durable schema and the in-memory one never diverge.
        self.persist_registry(&next)?;
        self.registry = next;

        self.events.emit(
            None,
            "schema.changed",
            serde_json::json!({ "workspace_id": self.workspace_id, "op": change_op(&change) }),
        );

        Ok(serde_json::json!({
            "op": change_op(&change),
            "registry": registry_summary(&self.registry),
            "created_index": created_index,
        }))
    }

    /// `schema.validate_compatibility` — prove the CURRENT registry is a
    /// forward-compatible, additive-only evolution of a baseline (CR-A2;
    /// `forge/spec/commands.md`: Owner/Maintainer/Editor/Auditor, DL-8).
    ///
    /// Payload: `{ against? }` — an optional baseline [`SchemaRegistry`] (the
    /// serialized form) the current registry must be a forward evolution of. When
    /// omitted the baseline is the empty registry (every registry is trivially a
    /// compatible evolution of empty), so the command doubles as a structural
    /// self-check. The supplied baseline is re-validated
    /// ([`SchemaRegistry::validated`]) so a hand-built/tampered baseline can't
    /// smuggle in a future-colliding id.
    ///
    /// Returns `{ ok, warnings }`. `ok: false` carries the
    /// [`CoreError::SchemaCompatibilityError`] message as the single warning rather
    /// than failing the command, so a UI can show the incompatibility without the
    /// request itself erroring (the destructive *apply* path is the one that hard-
    /// rejects).
    fn cmd_schema_validate_compatibility(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let baseline = match cmd.payload.get("against") {
            None | Some(serde_json::Value::Null) => SchemaRegistry::new(),
            Some(v) => {
                let parsed: SchemaRegistry = serde_json::from_value(v.clone()).map_err(|e| {
                    CoreError::ValidationError(format!(
                        "schema.validate_compatibility `against` is malformed: {e}"
                    ))
                })?;
                // Re-validate the untrusted baseline before comparing against it.
                parsed.validated()?
            }
        };
        match self.registry.validate_compatibility(&baseline) {
            Ok(()) => Ok(serde_json::json!({ "ok": true, "warnings": [] })),
            Err(e) => Ok(serde_json::json!({ "ok": false, "warnings": [e.to_string()] })),
        }
    }

    /// `schema.rebuild_indexes` — rebuild the storage indexes for the registry's
    /// `indexed` fields purely from canonical `records` (CR-A2;
    /// `forge/spec/commands.md`: Owner/Maintainer, DL-5/DL-6).
    ///
    /// Payload: `{ collection?, index_ids? }` — optional filters that narrow the
    /// rebuild to one collection and/or a set of index ids; absent → rebuild every
    /// registered index. The registry is the source of truth for *which* fields
    /// are indexed, so we first (re)register a definition for each `indexed` field
    /// (DL-8 → DL-5), then drop+recreate each selected physical structure from
    /// canonical records via [`Store::build_indexes`] (DL-6 rebuild-source-of-
    /// truth: never reads prior index pages / FTS rows).
    fn cmd_schema_rebuild_indexes(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let collection_filter = match cmd.payload.get("collection") {
            None | Some(serde_json::Value::Null) => None,
            Some(serde_json::Value::String(s)) => Some(s.clone()),
            Some(other) => {
                return Err(CoreError::ValidationError(format!(
                    "schema.rebuild_indexes `collection` must be a string, got {other}"
                )))
            }
        };
        let index_id_filter = parse_index_ids(cmd)?;

        // Re-register a definition for every `indexed` field so the manager
        // reflects the current registry (idempotent: create_index replaces same-
        // kind defs). Building from canonical records means the physical structure
        // is correct even if records predate the index (DL-6).
        let rebuilt = self.rebuild_registry_indexes(
            collection_filter.as_deref(),
            index_id_filter.as_deref(),
        )?;

        Ok(serde_json::json!({
            "rebuilt": rebuilt,
            "rebuilt_count": rebuilt.len(),
        }))
    }

    /// (Re)build the storage indexes the registry declares (its `indexed` fields),
    /// optionally narrowed to one `collection` and/or a set of `index_ids`.
    /// Returns the ids of the indexes that were (re)built, in stable order. Each
    /// index is created from canonical records (DL-6), so a field indexed after
    /// rows already exist is populated correctly.
    fn rebuild_registry_indexes(
        &mut self,
        collection_filter: Option<&str>,
        index_id_filter: Option<&[String]>,
    ) -> Result<Vec<String>> {
        let mut rebuilt = Vec::new();
        for (collection, field_id, kind) in indexed_fields(&self.registry) {
            if let Some(want) = collection_filter {
                if collection != want {
                    continue;
                }
            }
            // Compute the deterministic index id for the id_filter check WITHOUT
            // creating it first (so a filtered-out index is never built). The
            // public IndexDef constructor derives the same canonical name
            // Store::create_index will use, and also validates the identifiers.
            if let Some(ids) = index_id_filter {
                let index_id =
                    IndexDef::new(collection.clone(), field_id.clone(), kind.into(), IndexState::Active)?
                        .index_id;
                if !ids.iter().any(|w| w == &index_id) {
                    continue;
                }
            }
            // create_index drops + recreates the physical structure from canonical
            // records and (re)registers the Active definition — the DL-6 rebuild.
            let id = self.store.create_index(&mut self.indexes, &collection, &field_id, kind)?;
            rebuilt.push(id);
        }
        Ok(rebuilt)
    }

    /// Persist the registry to the workspace file (`__forge/meta` /
    /// `schema_registry`) as serialized JSON, mirroring the `db.read` grant
    /// persistence. So a defined schema survives reopen (DL-7/DL-8).
    fn persist_registry(&self, registry: &SchemaRegistry) -> Result<()> {
        let bytes = serde_json::to_vec(registry)
            .map_err(|e| CoreError::StorageError(format!("serialize schema registry: {e}")))?;
        self.store
            .kv_set(META_NS, SCHEMA_REGISTRY_KEY, &bytes, "application/json")
    }

    // -------------------------------------------------- workspace export/import

    /// `workspace.export` — write this workspace's **portable single-file bundle**
    /// (DL-24) and report what travelled vs. what was excluded.
    ///
    /// Payload: `{ path, include_run_logs? }`.
    ///   - `path` (string, required): write the bundle to this filesystem path
    ///     (the canonical DL-24 single SQLite file; refuses to overwrite an
    ///     existing file). The typed [`export_to_file`](Self::export_to_file)
    ///     API is the same path for in-process callers.
    ///   - `include_run_logs` (bool, default false): when true the bundle also
    ///     carries `runs` + `run_logs` (a debug/backup bundle). Run logs are
    ///     policy-dependent and excluded by default for privacy (DL-24).
    ///
    /// PORTABLE workspace state travels with the bundle: the reserved `__forge/meta`
    /// kv — applet manifests + compiled programs (so the imported workspace can RUN
    /// its applets), the persisted `db.read` grant table (workspace policy), and the
    /// `run_counter` sequence — plus applet `ctx.storage`, the CRDT chunks/snapshots
    /// (the source of truth), the oplog, and the records projection. SECRETS and
    /// device-local state are NEVER exported (the storage-layer
    /// [`is_local_only_namespace`](forge_storage::is_local_only_namespace) guard
    /// drops `secret/` / `provider/` / `device/` / `local/` namespaces).
    fn cmd_workspace_export(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let path = cmd
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError(
                    "workspace.export requires a `path` to write the bundle to".into(),
                )
            })?;
        let include_run_logs = bool_field(cmd, "include_run_logs")?;
        self.export_to_file(path, include_run_logs)?;

        // The descriptor of what travelled: applet manifests + the grant table are
        // portable workspace config; the run_counter is portable sequence state;
        // secrets/device-local namespaces are dropped by the storage guard.
        let applets = self.store.kv_list(META_NS, "applet/")?;
        let included = serde_json::json!({
            "meta": ["export_format_version", "forge_storage_schema_version", "workspace_id"],
            "applet_manifests_and_programs": applets.len(),
            "db_read_grants": !self.db_read_grants.is_empty(),
            "schema_registry": self.store.kv_get(META_NS, SCHEMA_REGISTRY_KEY)?.is_some(),
            "run_counter": self.store.kv_get(META_NS, RUN_COUNTER_KEY)?.is_some(),
            "records_projection": true,
            "crdt_chunks_and_snapshots": true,
            "oplog": true,
            "applet_storage_kv": true,
            "runs_and_run_logs": include_run_logs,
        });
        let excluded = serde_json::json!({
            "secrets": "never exported (secret/ provider/ credentials/ namespaces)",
            "device_local": "never exported (device/ local/ namespaces)",
            "runs_and_run_logs": if include_run_logs { "included by policy" } else { "excluded by default (privacy)" },
        });

        Ok(serde_json::json!({
            "export_format_version": EXPORT_FORMAT_VERSION,
            "workspace_id": self.workspace_id,
            "path": path,
            "include_run_logs": include_run_logs,
            "included": included,
            "excluded": excluded,
        }))
    }

    /// Write this workspace's portable DL-24 bundle to `path` (typed API; the
    /// `workspace.export` command is a thin wrapper). `include_run_logs` opts the
    /// `runs`/`run_logs` tables into the bundle (a debug/backup bundle); the
    /// default omits them for privacy.
    pub fn export_to_file(
        &self,
        path: impl AsRef<std::path::Path>,
        include_run_logs: bool,
    ) -> Result<()> {
        self.store.export_workspace(path, &self.export_options(include_run_logs))
    }

    /// The [`ExportOptions`] for this workspace under the given run-log policy
    /// (stamps the bundle with this workspace's id).
    fn export_options(&self, include_run_logs: bool) -> ExportOptions {
        ExportOptions {
            workspace_id: self.workspace_id.clone(),
            run_logs: if include_run_logs {
                RunLogPolicy::Include
            } else {
                RunLogPolicy::Exclude
            },
        }
    }

    /// `workspace.import` — load a portable bundle into **this fresh workspace**,
    /// rebuild the records projection from the imported CRDT chunks (DL-6, so the
    /// projection is byte-identical to the source), reload workspace config (the
    /// `db.read` grant table), and report what was reconstructed.
    ///
    /// Payload: `{ path }` (a bundle file). This workspace MUST be fresh (empty):
    /// an import reconstructs a whole workspace, it does not merge into a populated
    /// one — a non-empty target is rejected with `ValidationError`.
    ///
    /// After import the workspace can RUN its imported applets (their manifests +
    /// compiled programs travelled in `__forge/meta`) and its records match the
    /// source exactly. Secrets did not travel, so an applet that depends on a
    /// secret ref needs the secret rebound before it runs (DL-24).
    fn cmd_workspace_import(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let path = cmd
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError("workspace.import requires a `path` to a bundle".into())
            })?;

        // Refuse to import over a populated workspace: a bundle reconstructs a
        // fresh workspace, never a merge (matches the storage-layer contract and
        // avoids silently shadowing existing state).
        if !self.is_empty_workspace()? {
            return Err(CoreError::ValidationError(
                "workspace.import requires a fresh (empty) workspace; this workspace already has \
                 records, applets, or oplog history"
                    .into(),
            ));
        }

        self.import_from_file_in_place(path)?;

        let applets = self.store.kv_list(META_NS, "applet/")?;
        let collections = self.list_collections()?;
        let record_count = self.total_record_count(&collections)?;

        self.events.emit(
            None,
            "workspace.imported",
            serde_json::json!({
                "workspace_id": self.workspace_id,
                "applets": applets.len(),
                "records": record_count,
            }),
        );

        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "imported_applets": applets,
            "collections": collections,
            "records": record_count,
            "db_read_grants": self.db_read_grants.len(),
        }))
    }

    /// Import a bundle file into THIS workspace **in place**: open the bundle
    /// (itself a self-describing workspace file), copy its syncable state into the
    /// store `self` already holds and rebuild the projection from the imported
    /// chunks (DL-6), then reload the portable grant table so an imported scoped
    /// grant is in effect immediately. A fresh [`IndexManager`] is sufficient —
    /// indexes are physical structures rebuilt from canonical records, not part of
    /// the portable contract yet.
    ///
    /// Review 062 P1 #1: the import writes into `self.store` via
    /// [`Store::import_workspace_in_place`], so when this workspace is **file-backed**
    /// the imported tables are committed to the SAME file on disk and survive a
    /// drop + reopen of that path. The prior implementation imported into a separate
    /// in-memory store and swapped `self.store` to it, which reported success but
    /// lost everything on reopen of the original (still-empty) target file.
    fn import_from_file_in_place(&mut self, path: &str) -> Result<()> {
        let bundle = open_bundle(path)?;
        let indexes = IndexManager::new();
        self.store.import_workspace_in_place(&bundle, &indexes)?;
        self.db_read_grants = load_db_read_grants(&self.store)?;
        // The dynamic schema travelled in the portable kv: reload the registry and
        // reconstruct the indexes from its `indexed` fields so the imported
        // workspace's schema + indexes are immediately in force (DL-7/DL-8/DL-5).
        self.registry = load_schema_registry(&self.store)?;
        self.indexes = rebuild_indexes_from_registry(&self.store, &self.registry)?;
        Ok(())
    }

    /// Build a fresh imported [`WorkspaceCore`] from a bundle file (the typed API
    /// the CLI / next stage uses). The returned core is the imported workspace,
    /// ready to query and to run its imported applets; the portable `db.read` grant
    /// table is loaded into it.
    pub fn import_from_file(
        path: impl AsRef<std::path::Path>,
        workspace_id: impl Into<String>,
    ) -> Result<Self> {
        let bundle = open_bundle(path)?;
        let indexes = IndexManager::new();
        let store = Store::import_workspace_in_memory(&bundle, &indexes)?;
        // The schema registry travels in the portable `__forge/meta` kv, so the
        // imported workspace loads its registry + reconstructs its indexes exactly
        // like a normal open (DL-7/DL-8 schema is workspace state, DL-24 portable).
        Self::from_store(store, workspace_id)
    }

    /// True iff this workspace holds **no importable state at all** — the
    /// precondition for [`cmd_workspace_import`], so an import never silently merges
    /// into (or shadows) a populated workspace.
    ///
    /// Review 062 P1 #2: this delegates to the storage-level
    /// [`Store::is_empty_target`], which checks EVERY table/namespace a bundle would
    /// populate — the records projection, the CRDT source of truth
    /// (`crdt_chunks`/`crdt_snapshots`) + `oplog`, the policy-gated `runs`/`run_logs`,
    /// and every **portable** `kv` row (applet manifests/programs, the persisted
    /// `db.read` grant table, the `run_counter`). The prior check only looked at
    /// projected records, `applet/` meta, and the oplog, so a grants-only or
    /// kv-only workspace passed the "fresh" check and could have its state silently
    /// overwritten on import.
    fn is_empty_workspace(&self) -> Result<bool> {
        self.store.is_empty_target()
    }

    /// The distinct collection names present in the records projection, ordered.
    /// Read straight off the store's connection (a read-only accessor); used for
    /// the import report and the empty-workspace check.
    fn list_collections(&self) -> Result<Vec<String>> {
        let conn = self.store.connection();
        let mut stmt = conn
            .prepare("SELECT DISTINCT collection FROM records ORDER BY collection")
            .map_err(|e| CoreError::StorageError(format!("list collections: {e}")))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| CoreError::StorageError(format!("list collections: {e}")))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| CoreError::StorageError(format!("list collections: {e}")))?);
        }
        Ok(out)
    }

    /// Total live + tombstoned record count across `collections` (for the import
    /// report). `list_records` returns every projected row in a collection.
    fn total_record_count(&self, collections: &[String]) -> Result<usize> {
        let mut total = 0usize;
        for c in collections {
            total += self.store.list_records(c)?.len();
        }
        Ok(total)
    }

    // ----------------------------------------------------------- applet store

    /// Persist an installed applet (manifest + compiled program) in the reserved
    /// meta KV namespace.
    fn store_applet(&mut self, applet_id: &str, installed: &InstalledApplet) -> Result<()> {
        let bytes = serde_json::to_vec(installed).map_err(|e| {
            CoreError::StorageError(format!("applet serialize failed: {e}"))
        })?;
        self.store
            .kv_set(META_NS, &applet_key(applet_id), &bytes, "application/json")
    }

    /// Load an installed applet by id, if present.
    fn load_applet(&self, applet_id: &str) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &applet_key(applet_id))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("applet deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }

    // -------------------------------------------------- replay program pinning

    /// Persist the content-addressed replay fallback (`program/<code_hash>`),
    /// **write-once** (review 038 finding 3 / 036 finding 2).
    ///
    /// This artifact is the legacy fallback for runs recorded *before* per-run
    /// pinning (every modern run also gets a per-run pin via
    /// [`store_run_program`](Self::store_run_program), which is never overwritten).
    /// Because it is keyed by `code_hash` alone it does NOT capture the manifest a
    /// particular run used, so blindly overwriting it on every run let a later
    /// same-JS reinstall under a *different* manifest (e.g. tighter `limits`)
    /// replace the artifact a pre-per-run-pin run depends on — stranding that run,
    /// which would then replay under the wrong engine limits.
    ///
    /// Write-once fixes that: the first run to hash to a given `code_hash` pins the
    /// fallback (manifest + JS) and a later run with the **same** code_hash never
    /// overwrites it with a *different* manifest. Re-pinning identical content is an
    /// idempotent no-op (so a same-code, same-manifest re-run is unaffected); an
    /// identical-manifest re-pin is also a no-op. A legacy run keyed to this hash
    /// therefore always replays against the manifest first recorded for it.
    fn store_program(&mut self, installed: &InstalledApplet) -> Result<()> {
        // Write-once: if a fallback already exists for this code_hash, keep it.
        // A different manifest must not clobber the original (the stranding bug);
        // an identical one is a no-op either way.
        if self.load_program(&installed.code_hash)?.is_some() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(installed).map_err(|e| {
            CoreError::StorageError(format!("program serialize failed: {e}"))
        })?;
        self.store
            .kv_set(META_NS, &program_key(&installed.code_hash), &bytes, "application/json")
    }

    /// Load the program recorded for a given `code_hash`, if one was pinned.
    fn load_program(&self, code_hash: &str) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &program_key(code_hash))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("program deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }

    /// Persist the exact compiled program + manifest a run executed, keyed by the
    /// run's unique `run_id` (review 036 finding 2). Unlike the content-addressed
    /// [`store_program`], this key is unique to the execution, so reinstalling the
    /// same JS under a different manifest (tighter limits / changed caps) cannot
    /// overwrite an older run's pinned context.
    fn store_run_program(&mut self, run_id: &str, installed: &InstalledApplet) -> Result<()> {
        let bytes = serde_json::to_vec(installed).map_err(|e| {
            CoreError::StorageError(format!("run program serialize failed: {e}"))
        })?;
        self.store
            .kv_set(META_NS, &run_program_key(run_id), &bytes, "application/json")
    }

    /// Load the per-run pinned program for `run_id`, if one was recorded (runs
    /// recorded before per-run pinning have none → fall back to the code_hash pin).
    fn load_run_program(&self, run_id: &str) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &run_program_key(run_id))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("run program deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }

    // -------------------------------------------------- per-execution counter

    /// Atomically read-bump-write the persisted workspace run counter, returning
    /// the value assigned to this invocation. Monotone across the workspace's
    /// lifetime (persisted in meta), so each `runtime.run` mints a distinct
    /// `run_id` even for an identical applet+input pair (review 031 finding 2).
    ///
    /// Review 036 finding 3: the read+bump+write run inside ONE SQLite transaction
    /// ([`Store::next_counter`]), so the reservation is atomic. Two `WorkspaceCore`
    /// instances over the same file can no longer reserve the same invocation
    /// number — the second transaction observes the first's committed value — so no
    /// audit record is silently replaced via a `run_id` collision.
    fn next_run_counter(&mut self) -> Result<u64> {
        self.store.next_counter(META_NS, RUN_COUNTER_KEY)
    }
}

/// Load the persisted trusted `db.read` grant table from the workspace file
/// (review 050). Absent / empty → an empty table (no configured scopes), which
/// preserves the owner-permits-all M0a default for actors with no grant entry.
fn load_db_read_grants(
    store: &Store,
) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    match store.kv_get(META_NS, DB_READ_GRANTS_KEY)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::StorageError(format!("deserialize db.read grants: {e}"))
        }),
        None => Ok(std::collections::BTreeMap::new()),
    }
}

/// Load the persisted [`SchemaRegistry`] from the workspace file (DL-7/DL-8).
/// Absent → an empty registry (the M0a default for a fresh workspace). A present
/// registry is re-validated via [`SchemaRegistry::validated`] so a tampered/
/// corrupt persisted registry surfaces a [`CoreError::SchemaCompatibilityError`]
/// instead of silently loading a structurally-invalid schema. Mirrors
/// [`load_db_read_grants`].
fn load_schema_registry(store: &Store) -> Result<SchemaRegistry> {
    match store.kv_get(META_NS, SCHEMA_REGISTRY_KEY)? {
        Some(bytes) => {
            let registry: SchemaRegistry = serde_json::from_slice(&bytes).map_err(|e| {
                CoreError::StorageError(format!("deserialize schema registry: {e}"))
            })?;
            registry.validated()
        }
        None => Ok(SchemaRegistry::new()),
    }
}

/// Reconstruct the dynamic-index manager from the registry's `indexed` fields
/// (DL-8 → DL-5). For each non-deprecated `indexed` field, (re)create the storage
/// index from canonical `records`; the expression-index DDL is `IF NOT EXISTS`
/// and SQLite keeps it in the file across reopen, so this re-registers the
/// in-memory definition and re-derives FTS shadow rows. Called on every open so a
/// schema-defined index is a live planner candidate without an explicit rebuild.
fn rebuild_indexes_from_registry(
    store: &Store,
    registry: &SchemaRegistry,
) -> Result<IndexManager> {
    let mut indexes = IndexManager::new();
    for (collection, field_id, kind) in indexed_fields(registry) {
        store.create_index(&mut indexes, &collection, &field_id, kind)?;
    }
    Ok(indexes)
}

/// Every `(collection, field_id, kind)` the registry declares as `indexed`,
/// skipping deprecated fields (a hidden field's index is not maintained). The
/// kind is derived from the field type ([`index_kind_for`]). Stable iteration
/// order (registry collections are a `BTreeMap`, fields are declaration-ordered).
fn indexed_fields(registry: &SchemaRegistry) -> Vec<(String, String, CreateIndexKind)> {
    let mut out = Vec::new();
    for (name, col) in registry.collections() {
        out.extend(collection_indexed_fields(name, col));
    }
    out
}

/// The `indexed` (non-deprecated) fields of one collection as
/// `(collection, field_id, kind)`.
fn collection_indexed_fields(
    name: &str,
    col: &CollectionDef,
) -> Vec<(String, String, CreateIndexKind)> {
    col.fields()
        .iter()
        .filter(|f| f.indexed() && !f.deprecated())
        .map(|f| (name.to_string(), f.field_id().to_string(), index_kind_for(f)))
        .collect()
}

/// The dynamic-index kind for a field (DL-5): a `Text` field gets a full-text
/// (`Fts`) shadow table; every other type gets an equality/range/order (`Value`)
/// expression index. The nullable wrapper is peeled so `Nullable(Text)` is still
/// full-text.
fn index_kind_for(field: &FieldDef) -> CreateIndexKind {
    match field.ty().inner() {
        FieldType::Text => CreateIndexKind::Fts,
        _ => CreateIndexKind::Value,
    }
}

/// The (stable field id, index kind) for the last-added field of `collection` in
/// `registry`, iff that field is marked `indexed` (DL-8 → DL-5). Takes the
/// registry explicitly so `schema.apply_change` can probe the CANDIDATE registry
/// and create the index BEFORE persisting (review 066 atomicity).
fn indexed_field_to_create(
    registry: &SchemaRegistry,
    collection: &str,
) -> Option<(String, CreateIndexKind)> {
    let col = registry.collection(collection)?;
    let field = col.fields().last()?;
    if !field.indexed() {
        return None;
    }
    Some((field.field_id().to_string(), index_kind_for(field)))
}

/// The serde `op` tag for a [`SchemaChange`] (for the response/event payload).
fn change_op(change: &SchemaChange) -> &'static str {
    match change {
        SchemaChange::AddCollection { .. } => "add_collection",
        SchemaChange::AddField { .. } => "add_field",
        SchemaChange::RenameField { .. } => "rename_field",
        SchemaChange::WidenField { .. } => "widen_field",
        SchemaChange::DeprecateField { .. } => "deprecate_field",
        SchemaChange::EnforceRequired { .. } => "enforce_required",
    }
}

/// A compact JSON summary of the registry for the `schema.apply_change` response:
/// each collection with its fields' stable ids, names, types, and flags. Lets a
/// shell confirm the minted ids / evolved state without re-reading the persisted
/// registry.
fn registry_summary(registry: &SchemaRegistry) -> serde_json::Value {
    let collections: serde_json::Map<String, serde_json::Value> = registry
        .collections()
        .map(|(name, col)| {
            let fields: Vec<serde_json::Value> = col
                .fields()
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "field_id": f.field_id(),
                        "name": f.name(),
                        "ty": f.ty(),
                        "indexed": f.indexed(),
                        "deprecated": f.deprecated(),
                        "required": f.required(),
                        "enforced": f.enforced(),
                    })
                })
                .collect();
            (name.to_string(), serde_json::json!({ "fields": fields }))
        })
        .collect();
    serde_json::json!({ "collections": collections })
}

/// Parse the optional `index_ids` filter for `schema.rebuild_indexes`: an array
/// of index-id strings, or absent for "all". A present-but-malformed value is a
/// `ValidationError`.
fn parse_index_ids(cmd: &CoreCommand) -> Result<Option<Vec<String>>> {
    match cmd.payload.get("index_ids") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::Array(arr)) => {
            let mut out = Vec::with_capacity(arr.len());
            for entry in arr {
                let s = entry.as_str().ok_or_else(|| {
                    CoreError::ValidationError(
                        "schema.rebuild_indexes `index_ids` entries must be strings".into(),
                    )
                })?;
                out.push(s.to_string());
            }
            Ok(Some(out))
        }
        Some(other) => Err(CoreError::ValidationError(format!(
            "schema.rebuild_indexes `index_ids` must be an array of strings, got {other}"
        ))),
    }
}

/// Open a DL-24 bundle file as a [`Store`] for import. The bundle is itself a
/// self-describing workspace SQLite file (DECISIONS E1), so opening it as a
/// `Store` is valid; the version header is validated inside
/// [`Store::import_workspace_in_memory`] before any state is copied. A missing
/// path is a `ValidationError` (a clear "no such bundle" rather than a raw
/// SQLite error) so the command surfaces a caller-actionable message.
fn open_bundle(path: impl AsRef<std::path::Path>) -> Result<Store> {
    let path = path.as_ref();
    if !path.exists() {
        return Err(CoreError::ValidationError(format!(
            "import bundle {} does not exist",
            path.display()
        )));
    }
    Store::open(path)
}

/// Read an optional boolean command field, defaulting to `false` when absent.
/// A present-but-non-boolean value is a `ValidationError` rather than a silent
/// default, so a malformed flag is surfaced.
fn bool_field(cmd: &CoreCommand, field: &str) -> Result<bool> {
    match cmd.payload.get(field) {
        None | Some(serde_json::Value::Null) => Ok(false),
        Some(serde_json::Value::Bool(b)) => Ok(*b),
        Some(other) => Err(CoreError::ValidationError(format!(
            "{} `{field}` must be a boolean, got {other}",
            cmd.name
        ))),
    }
}

/// KV key for an applet's installed record within [`META_NS`].
fn applet_key(applet_id: &str) -> String {
    format!("applet/{applet_id}")
}

/// KV key for a pinned replay program within [`META_NS`], keyed by `code_hash`.
/// Content-addressed, so the same code reinstalled under a new applet version
/// still maps to the one program every run that hashed to it can replay against.
/// Kept as a fallback for runs recorded before per-run pinning (review 036
/// finding 2). It does NOT capture the manifest a specific run used, so the
/// write is **write-once** ([`store_program`](WorkspaceCore::store_program)):
/// the first run to hash to it pins the fallback and a later same-code reinstall
/// under a different manifest can no longer overwrite it (review 038 finding 3).
fn program_key(code_hash: &str) -> String {
    format!("program/{code_hash}")
}

/// KV key for the PER-RUN pinned replay program within [`META_NS`], keyed by the
/// unique `run_id` (review 036 finding 2). Unique per execution, so no reinstall
/// can overwrite the program + manifest an older run replays against.
fn run_program_key(run_id: &str) -> String {
    format!("program/run/{run_id}")
}

/// Read the optional explicit `(random_seed, time_start)` seam override from a
/// `runtime.run` payload (review 032 finding 1).
///
/// Returns `Ok(None)` when neither field is present (the default
/// `(code_hash, input)`-derived seeds apply). If *either* is present, *both*
/// must be: a half-specified override is a malformed command (a scenario that
/// pins one seam but lets the other drift is not reproducible), rejected with
/// `ValidationError` rather than silently defaulting the missing seam. Each
/// field must be a non-negative integer that fits `u64`.
fn run_seed_override(cmd: &CoreCommand) -> Result<Option<(u64, u64)>> {
    let random_seed = cmd.payload.get("random_seed");
    let time_start = cmd.payload.get("time_start");
    match (random_seed, time_start) {
        (None, None) => Ok(None),
        (Some(r), Some(t)) => {
            let random_seed = seed_field("random_seed", r)?;
            let time_start = seed_field("time_start", t)?;
            // The logical clock represents time as `i64` (LogicalClock::new casts
            // `time_start as i64`), so a value above `i64::MAX` would wrap to a
            // negative start that `ctx.time.now()` could never have produced
            // honestly. Reject it rather than record an unrepresentable seam
            // (review 037 finding 2).
            if time_start > i64::MAX as u64 {
                return Err(CoreError::ValidationError(format!(
                    "runtime.run `time_start` must fit i64 (<= {}), got {time_start}",
                    i64::MAX
                )));
            }
            Ok(Some((random_seed, time_start)))
        }
        (Some(_), None) | (None, Some(_)) => Err(CoreError::ValidationError(
            "runtime.run seed override must set BOTH `random_seed` and `time_start` or neither"
                .into(),
        )),
    }
}

/// Parse a `u64` seed field from the command payload, rejecting non-integer /
/// out-of-range values with a precise `ValidationError`.
fn seed_field(name: &str, value: &serde_json::Value) -> Result<u64> {
    value.as_u64().ok_or_else(|| {
        CoreError::ValidationError(format!(
            "runtime.run `{name}` must be a non-negative integer that fits u64, got {value}"
        ))
    })
}

/// Derive the deterministic replay seeds `(random_seed, time_start)` from the
/// run's `(code_hash, input)` (review 031 finding 2). The same code + input
/// always produces the same seeds, so re-runs replay byte-identically and the
/// "deterministic across independent runs" property holds; a different input
/// produces different seeds (a genuinely different deterministic run).
///
/// This is a stable, non-cryptographic split of the canonical `code_hash`
/// (which already digests the program) mixed with a digest of the input. It is
/// not security-sensitive — only determinism matters — so a fixed FNV-style
/// fold over the canonical inputs is sufficient and dependency-free.
fn derive_seeds(code_hash: &str, input: &serde_json::Value) -> (u64, u64) {
    // Canonical JSON for the input (serde_json sorts object keys), so equal
    // inputs fold to the same digest regardless of construction order.
    let input_repr = input.to_string();
    let random_seed = fnv1a64(code_hash.as_bytes()) ^ fnv1a64(input_repr.as_bytes());
    // A second, independent fold (salted) for the time seam so the two seeds are
    // not trivially correlated. Mask the sign bit so the value always fits `i64`:
    // the logical clock stores time as `i64` (LogicalClock::new casts), so a
    // derived seed above `i64::MAX` would wrap negative and disagree with the
    // recorded seam (review 037/039 finding 2 — same bound as the explicit
    // `time_start` override, applied to the derived path too).
    let time_start = (fnv1a64(input_repr.as_bytes()).wrapping_mul(0x100000001b3)
        ^ fnv1a64(code_hash.as_bytes()))
        & (i64::MAX as u64);
    (random_seed, time_start)
}

/// A small FNV-1a 64-bit fold. Deterministic and dependency-free; used only to
/// derive replay seeds (not for security or collision resistance).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Mint a unique, inspectable per-execution `run_id` from the run's `code_hash`
/// and the workspace's monotone invocation counter (review 031 finding 2). The
/// counter guarantees uniqueness even when two executions share code+input (and
/// therefore seeds); the short hash prefix keeps the id self-describing.
fn unique_run_id(code_hash: &str, invocation: u64) -> RunId {
    // Strip the `alg:` tag, then take a short prefix of the digest body.
    let digest = code_hash.split_once(':').map(|(_, body)| body).unwrap_or(code_hash);
    let short = &digest[..8.min(digest.len())];
    RunId::new(format!("run_{short}_{invocation:06}"))
}

/// Command-level RBAC gate (prd-merged/01 CR-A3): reject a command whose
/// actor role is not permitted to issue it *before* any handler touches state.
///
/// The role matrix is the "Roles" column of `forge/spec/commands.md` for the
/// M0a command set. An unauthorized actor returns [`CoreError::PermissionDenied`];
/// an unknown command is left for the dispatcher to reject with a
/// `ValidationError` (so capability negotiation, not authorization, owns that
/// message). This is the first of the two CR-A3 layers; the per-`ctx.*`
/// capability/policy gate still runs at host-call time inside the runtime.
fn authorize(cmd: &CoreCommand) -> Result<()> {
    let role = cmd.actor.role;
    // `None` ⇒ no command-level role restriction (the command is reachable by
    // any authenticated actor, or it is an unknown name the dispatcher rejects).
    let allowed: Option<&[Role]> = match cmd.name.as_str() {
        // Owner-only workspace lifecycle (commands.md: workspace.create → Owner).
        "workspace.create" => Some(&[Role::Owner]),
        // Read-level metadata: every member role may open/inspect the workspace.
        "workspace.open" => Some(&[
            Role::Owner,
            Role::Maintainer,
            Role::Editor,
            Role::Viewer,
            Role::Auditor,
        ]),
        // Installing/compiling an applet is a maintainer+ operation (SC-15):
        // Viewer/Auditor/Runner/Editor cannot install.
        "applet.install" => Some(&[Role::Owner, Role::Maintainer]),
        // Triggering execution: the run-capable roles (CR-8). Viewer/Auditor are
        // read-only/oversight and cannot run code.
        "runtime.run" => Some(&[Role::Owner, Role::Maintainer, Role::Editor, Role::Runner]),
        // Replay is an audit/oversight operation (CR-9): Auditor/Maintainer/Owner.
        // A bare Runner can run but not replay (per commands.md).
        "runtime.replay" => Some(&[Role::Owner, Role::Maintainer, Role::Auditor]),
        // Reading the records projection requires a read-capable role (db.read).
        "query.execute" => Some(&[
            Role::Owner,
            Role::Maintainer,
            Role::Editor,
            Role::Viewer,
            Role::Auditor,
        ]),
        // Schema evolution (commands.md: schema.apply_change → Owner, Maintainer;
        // DL-8). An additive schema change mutates workspace state, so it is a
        // maintainer+ operation — a Viewer/Editor/Auditor cannot apply one.
        "schema.apply_change" => Some(&[Role::Owner, Role::Maintainer]),
        // Validating compatibility is a read-only check (commands.md:
        // schema.validate_compatibility → Owner, Maintainer, Editor, Auditor).
        "schema.validate_compatibility" => {
            Some(&[Role::Owner, Role::Maintainer, Role::Editor, Role::Auditor])
        }
        // Rebuilding indexes is a maintenance op (commands.md:
        // schema.rebuild_indexes → Owner, Maintainer; DL-5).
        "schema.rebuild_indexes" => Some(&[Role::Owner, Role::Maintainer]),
        // Exporting the portable workspace bundle (DL-24, commands.md:
        // workspace.export → Owner, Maintainer, Auditor). The Auditor may take a
        // backup/debug bundle (including run logs by policy) without otherwise
        // mutating the workspace.
        "workspace.export" => Some(&[Role::Owner, Role::Maintainer, Role::Auditor]),
        // Importing a bundle reconstructs a workspace (commands.md:
        // workspace.import → Owner). Owner-only because an import writes the whole
        // syncable state (applets, records, grants) into the target.
        "workspace.import" => Some(&[Role::Owner]),
        _ => None,
    };
    match allowed {
        Some(roles) if !roles.contains(&role) => Err(CoreError::PermissionDenied(format!(
            "actor role {role:?} is not permitted to issue {:?} (see forge/spec/commands.md)",
            cmd.name
        ))),
        _ => Ok(()),
    }
}

/// True iff `role` carries the `db.read` capability at the command level.
///
/// `forge/spec/commands.md` lists the data-read membership roles (the same set
/// that may `workspace.open` / `file.history` / read projections): Owner,
/// Maintainer, Editor, Viewer, Auditor. The execution-only `Runner` and the
/// code-review `Reviewer` are NOT data readers, so they lack `db.read` even
/// though `Runner` may `runtime.run`. This mirrors the manifest `db.read` grant
/// the runtime enforces per `ctx.db.*` call, lifted to the workspace command.
fn role_has_db_read(role: Role) -> bool {
    matches!(
        role,
        Role::Owner | Role::Maintainer | Role::Editor | Role::Viewer | Role::Auditor
    )
}

/// Collection-scoped `db.read` capability gate for `query.execute` (review
/// 036/038/048 finding 1; `forge/spec/commands.md:21` "Role plus db.read
/// capability" + `forge/spec/capabilities.md:23` `resource: collection:<name>`).
///
/// Two independent checks, both required:
///
///   1. **Role** — the actor's role must carry `db.read` ([`role_has_db_read`]).
///      A `Runner` (execution-only) fails here with `PermissionDenied`.
///   2. **Scope** — the target `collection` must be within the caller's granted
///      `db.read` scope. `trusted_scope` is the workspace-side grant for this
///      actor (`Some(&["tasks"])`, `Some(&["*"])` for read-all, or `Some(&[])` for
///      "no collection granted"), resolved by the caller from the TRUSTED grant
///      table — **never** from the request payload (review 048 finding 1). A
///      collection outside the granted scope is `CapabilityRequired` with a
///      message naming `db.read collection:<name>`, so a role that cleared check 1
///      is still denied when it was not granted that specific collection (this is
///      what makes the capability layer load-bearing rather than redundant with
///      the role gate, AND unforgeable from the command body).
///
/// Back-compat: when the actor has **no** trusted grant entry (`None`), the scope
/// defaults to the role-derived read scope (read-all for a `db.read`-capable
/// role), so the existing owner-permits-all spine — which provisions no grants —
/// keeps working. To exercise a narrowed scope, configure it through
/// [`WorkspaceCore::grant_db_read`].
///
/// Defense in depth: a request payload that smuggles its own `grants.db.read`
/// scope is treated as untrusted input. It can only ever *narrow* (it cannot
/// widen the trusted grant), and a payload grant that tries to exceed the trusted
/// scope is rejected as a `PermissionDenied` self-escalation attempt rather than
/// silently honored.
fn require_db_read(cmd: &CoreCommand, collection: &str, trusted_scope: Option<&[String]>) -> Result<()> {
    // Layer 1: role.
    if !role_has_db_read(cmd.actor.role) {
        return Err(CoreError::PermissionDenied(format!(
            "actor role {:?} lacks the db.read capability required to query {collection:?} (forge/spec/commands.md: query.execute = Role plus db.read)",
            cmd.actor.role
        )));
    }

    // A payload-supplied `grants.db.read` is untrusted: validate its shape and
    // ensure it does not attempt to exceed the trusted grant. It is NEVER the
    // authorization source.
    reject_payload_self_escalation(cmd, trusted_scope)?;

    // Layer 2: collection-scoped grant, evaluated against the TRUSTED scope only.
    match trusted_scope {
        // No trusted grant entry → role-derived read-all (back-compat).
        None => Ok(()),
        Some(scope) if scope_grants(scope, collection) => Ok(()),
        Some(_) => Err(CoreError::CapabilityRequired(format!(
            "db.read collection:{collection} is not within the caller's granted db.read scope (forge/spec/capabilities.md: db.read is collection-scoped)"
        ))),
    }
}

/// Reject a request whose payload `grants.db.read` tries to grant the caller MORE
/// than its trusted scope (a self-escalation). The payload grant is never used to
/// authorize; this only refuses an attempt to widen access through the command
/// body, and validates the grant shape. A payload that is absent, malformed, or a
/// subset of the trusted scope passes (the trusted scope still decides access).
fn reject_payload_self_escalation(cmd: &CoreCommand, trusted_scope: Option<&[String]>) -> Result<()> {
    let payload_scope = match payload_db_read_scope(cmd)? {
        None => return Ok(()),
        Some(scope) => scope,
    };
    // With no trusted entry the actor is role-derived read-all, so any payload
    // scope is a (redundant) narrowing — nothing to escalate past.
    let Some(trusted) = trusted_scope else {
        return Ok(());
    };
    // Read-all trusted scope can never be exceeded.
    if trusted.iter().any(|s| s == "*") {
        return Ok(());
    }
    // Any payload entry not covered by the trusted scope is an escalation attempt.
    for entry in &payload_scope {
        if !scope_grants(trusted, entry) {
            return Err(CoreError::PermissionDenied(format!(
                "query.execute payload requested db.read collection:{entry} beyond the actor's trusted grant; the db.read scope is set by the workspace, not the request (review 048)"
            )));
        }
    }
    Ok(())
}

/// Parse a payload-supplied `db.read` scope from `payload.grants.db.read`, if
/// present. `Ok(None)` means no scope was supplied; `Ok(Some(scopes))` is the
/// (untrusted) list of collection names (`"*"` = read-all). A malformed `grants`
/// shape is a `ValidationError` rather than a silently-ignored grant.
fn payload_db_read_scope(cmd: &CoreCommand) -> Result<Option<Vec<String>>> {
    let grants = match cmd.payload.get("grants") {
        None => return Ok(None),
        Some(g) => g,
    };
    // `grants.db.read` — absent at any level means "no db.read scope supplied".
    let read = grants.get("db").and_then(|db| db.get("read"));
    let read = match read {
        None => return Ok(None),
        Some(r) => r,
    };
    let arr = read.as_array().ok_or_else(|| {
        CoreError::ValidationError(
            "query.execute `grants.db.read` must be an array of collection names".into(),
        )
    })?;
    let mut scopes = Vec::with_capacity(arr.len());
    for entry in arr {
        let s = entry.as_str().ok_or_else(|| {
            CoreError::ValidationError(
                "query.execute `grants.db.read` entries must be collection-name strings".into(),
            )
        })?;
        scopes.push(s.to_string());
    }
    Ok(Some(scopes))
}

/// True iff `collection` is granted by `scope` — either an exact collection-name
/// match or the read-all wildcard `"*"`.
fn scope_grants(scope: &[String], collection: &str) -> bool {
    scope.iter().any(|s| s == "*" || s == collection)
}

/// Verify the optional package signature carried on an `applet.install`
/// (SC-15 / MP-4), returning the [`InstallTrust`] to record.
///
/// The optional `signature` payload field is the prd-merged/08 MP-4 signed
/// package — the exact T012 fixture shape:
///
/// ```json
/// "signature": {
///   "package": { "manifest": {…}, "files": [{path, content, sha256}], "hashes": {…} },
///   "signature": "ed25519:…",
///   "public_key": "ed25519:…" | "<PEM SubjectPublicKeyInfo>",
///   "publisher_trust": { "publisher": "...", "status": "unknown" | …, "valid_until": "…" }
/// }
/// ```
///
/// When the field is ABSENT the install is [`InstallTrust::Unsigned`] (the M0a
/// default — signing is not yet mandatory). When PRESENT the package is verified
/// with [`forge_signing::verify_package`] over the canonical `terrane/sig/v1`
/// preimage:
///
///   - any failure — crypto (bad/garbage/wrong-key signature), `package_hash`
///     (a file/manifest/permissions/policy region tampered after signing), or
///     `policy` (publisher not trusted / expired) — is surfaced as
///     `ValidationError("package signature invalid: <layer>: <reason>")`, so the
///     caller REJECTS the install;
///   - the verified package is then BOUND to `sources` via
///     [`bind_signature_to_sources`] so the signature only blesses the code
///     actually being installed (review 080 #1);
///   - on success the verified publisher / key id (+ whether the policy layer was
///     enforced) is returned as [`InstallTrust::Signed`].
///
/// `publisher_trust` is optional: present → the marketplace-policy layer is
/// enforced (the publisher must be trusted and unexpired); absent → crypto +
/// integrity only, the M0a "verify when present, surface the result" default.
fn verify_install_signature(
    cmd: &CoreCommand,
    sources: &serde_json::Map<String, serde_json::Value>,
) -> Result<InstallTrust> {
    let signature = match cmd.payload.get("signature") {
        None | Some(serde_json::Value::Null) => return Ok(InstallTrust::Unsigned),
        Some(sig) => sig,
    };

    // The signed package (MP-4 `files`/`manifest`/`hashes`).
    let package: Package = signed_field(signature, "package")?;
    let signature_str = signed_str(signature, "signature")?;
    let public_key = signed_str(signature, "public_key")?;

    // Optional marketplace-policy input (the publisher trust set). Present →
    // enforce the policy layer; absent → crypto + integrity only.
    let publisher_trust: Option<PublisherTrust> = match signature.get("publisher_trust") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "applet.install `signature.publisher_trust` is malformed: {e}"
            ))
        })?),
    };
    let publisher_trust_enforced = publisher_trust.is_some();

    // Verify over the canonical preimage. A CRYPTO/integrity/policy failure
    // rejects the install; the typed reason names the failing layer.
    match verify_package(
        &package,
        &signature_str,
        &public_key,
        publisher_trust.as_ref(),
    ) {
        TrustOutcome::Trusted => {
            // BIND the verified package to the install payload (review 080 #1):
            // a valid signature only blesses the EXACT code it signed. The signed
            // package's files must be identical (path + content) to the `sources`
            // that will actually be compiled and stored — otherwise a caller could
            // attach any valid signed package to arbitrary top-level code and still
            // be reported as `Signed`.
            bind_signature_to_sources(&package, sources)?;

            // Record the verified publisher identity for later trust reporting.
            let publisher = manifest_string(&package.manifest, "publisher");
            let key_id = manifest_string(&package.manifest, "keyId");
            Ok(InstallTrust::Signed {
                publisher,
                key_id,
                publisher_trust_enforced,
            })
        }
        TrustOutcome::Rejected(err) => Err(CoreError::ValidationError(format!(
            "package signature invalid: {}: {}",
            err.layer.as_str(),
            err.reason
        ))),
    }
}

/// Confirm a verified signed `package` actually describes the code being
/// installed (review 080 #1). Without this, a valid signature over package A
/// could be attached to an install of arbitrary code B and still report
/// `Signed` — the signature would bless an app that is not the one installed.
///
/// The bind is exact: the signed package's files and the install `sources` must
/// be the SAME set of `path -> content` entries. The signature already attests
/// the files' integrity (forge-signing verified each `contentHash`/per-file
/// digest), so matching the install sources to those files transitively binds
/// the signature to exactly what is compiled and stored. A mismatch — an extra,
/// missing, or differing file — is a `package_hash`-class rejection (the package
/// does not match the payload), surfaced like any other signature failure so the
/// install is rejected and nothing is stored.
fn bind_signature_to_sources(
    package: &Package,
    sources: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let reject = |reason: String| {
        Err(CoreError::ValidationError(format!(
            "package signature invalid: package_hash: {reason}"
        )))
    };

    // Same number of files (so the signed set has no extra files and the install
    // has no unsigned ones).
    if package.files.len() != sources.len() {
        return reject(format!(
            "signed package declares {} file(s) but the install carries {} source(s)",
            package.files.len(),
            sources.len()
        ));
    }

    // Every signed file must be present in the install with identical content,
    // and every install source must therefore be covered (the equal-length check
    // above turns "every signed file matches a source" into a bijection).
    for file in &package.files {
        match sources.get(&file.path).and_then(|v| v.as_str()) {
            Some(content) if content == file.content => {}
            Some(_) => {
                return reject(format!(
                    "install source {:?} content does not match the signed package",
                    file.path
                ));
            }
            None => {
                return reject(format!(
                    "signed file {:?} is not among the install sources",
                    file.path
                ));
            }
        }
    }
    Ok(())
}

/// Read an optional `manifest.<key>` string out of a signed package's manifest
/// (a [`serde_json::Value`]), for recording the verified publisher / key id. A
/// missing/non-string field yields `None` rather than erroring — by the time
/// this runs the package has already verified, so this is provenance reporting,
/// not validation.
fn manifest_string(manifest: &serde_json::Value, key: &str) -> Option<String> {
    manifest
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Deserialize a required sub-field of the install `signature` object into `T`,
/// surfacing a `ValidationError` (never a panic) on a missing/malformed field.
fn signed_field<T: serde::de::DeserializeOwned>(
    signature: &serde_json::Value,
    field: &str,
) -> Result<T> {
    let value = signature.get(field).ok_or_else(|| {
        CoreError::ValidationError(format!(
            "applet.install `signature` requires a `{field}` field"
        ))
    })?;
    serde_json::from_value(value.clone()).map_err(|e| {
        CoreError::ValidationError(format!("applet.install `signature.{field}` is malformed: {e}"))
    })
}

/// Read a required string sub-field of the install `signature` object.
fn signed_str(signature: &serde_json::Value, field: &str) -> Result<String> {
    signature
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            CoreError::ValidationError(format!(
                "applet.install `signature.{field}` must be a string"
            ))
        })
}

/// Extract and require the command's `applet_id` (from the envelope, or the
/// payload as a fallback).
fn require_applet_id(cmd: &CoreCommand) -> Result<AppletId> {
    if let Some(id) = &cmd.applet_id {
        return Ok(id.clone());
    }
    cmd.payload
        .get("applet_id")
        .and_then(|v| v.as_str())
        .map(AppletId::new)
        .ok_or_else(|| CoreError::ValidationError(format!("{} requires an applet_id", cmd.name)))
}

/// Deserialize a required object field from the command payload.
fn take_field<T: serde::de::DeserializeOwned>(cmd: &CoreCommand, field: &str) -> Result<T> {
    let value = cmd.payload.get(field).ok_or_else(|| {
        CoreError::ValidationError(format!("{} requires a `{field}` field", cmd.name))
    })?;
    serde_json::from_value(value.clone()).map_err(|e| {
        CoreError::ValidationError(format!("{} `{field}` is malformed: {e}", cmd.name))
    })
}

/// `(ok, app_result_json)` for a run's outcome.
fn outcome_fields(run: &RunRecord) -> (bool, serde_json::Value) {
    use forge_domain::RunOutcome;
    match &run.outcome {
        RunOutcome::Completed { result } => {
            (result.ok, serde_json::to_value(result).unwrap_or(serde_json::Value::Null))
        }
        RunOutcome::Failed { error } => {
            (false, serde_json::json!({ "error": error }))
        }
    }
}

/// A compact summary of a run for the response payload + observability.
fn run_summary(run: &RunRecord) -> serde_json::Value {
    serde_json::json!({
        "run_id": run.run_id,
        "applet_id": run.applet_id,
        "code_hash": run.code_hash,
        "calls": run.calls.len(),
        "logs": run.logs.len(),
        "completed": run.is_completed(),
    })
}

#[cfg(test)]
mod seed_override_tests {
    use super::*;
    use forge_domain::{ActorContext, RequestId, WorkspaceId};

    fn run_cmd(payload: serde_json::Value) -> CoreCommand {
        CoreCommand {
            request_id: RequestId::new("r1"),
            actor: ActorContext::owner("dev"),
            workspace_id: WorkspaceId::new("ws1"),
            applet_id: None,
            name: "runtime.run".into(),
            payload,
        }
    }

    #[test]
    fn no_override_when_neither_seed_present() {
        assert_eq!(run_seed_override(&run_cmd(serde_json::json!({}))).unwrap(), None);
    }

    #[test]
    fn both_seeds_in_range_are_accepted() {
        let got = run_seed_override(&run_cmd(serde_json::json!({
            "random_seed": 7u64, "time_start": 1000u64
        })))
        .unwrap();
        assert_eq!(got, Some((7, 1000)));
    }

    #[test]
    fn half_specified_override_is_rejected() {
        assert_eq!(
            run_seed_override(&run_cmd(serde_json::json!({ "random_seed": 7u64 })))
                .unwrap_err()
                .code(),
            "ValidationError"
        );
    }

    #[test]
    fn time_start_above_i64_max_is_rejected() {
        // review 037 finding 2: the logical clock is i64, so a u64 time_start
        // beyond i64::MAX would wrap negative — reject it instead of recording an
        // unrepresentable seam. random_seed may still use the full u64 range.
        let over = (i64::MAX as u64) + 1;
        let err = run_seed_override(&run_cmd(serde_json::json!({
            "random_seed": u64::MAX, "time_start": over
        })))
        .unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        // boundary: exactly i64::MAX is allowed.
        let ok = run_seed_override(&run_cmd(serde_json::json!({
            "random_seed": 1u64, "time_start": i64::MAX as u64
        })))
        .unwrap();
        assert_eq!(ok, Some((1, i64::MAX as u64)));
    }
}
