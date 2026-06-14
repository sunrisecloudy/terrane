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

#[path = "auth.rs"]
mod auth;
use auth::authorize;
#[path = "persistence.rs"]
mod persistence;
use persistence::*;
#[path = "watch.rs"]
mod watch;
use watch::{load_watch_sessions, WatchSessions};
#[path = "signing.rs"]
mod signing;
#[path = "commands/mod.rs"]
mod commands;
// The command handlers' shared record types live in `commands::applet` (the
// install handler that mints them); re-export so this facade + the sibling
// `signing` module (`super::InstallTrust`) keep their existing names.
pub(in crate::workspace) use commands::applet::{InstallTrust, InstalledApplet};
// The registry → storage-index helper the open-time index reconstruction
// (`rebuild_indexes_from_registry`) shares with the `schema.*` command module.
use commands::schema::indexed_fields;
// The command [`Registry`] that turns the former `handle` dispatch match into a
// built-once name → handler table (/simplify #11b). `handle` consults it AFTER the
// CR-A3 `authorize` gate, preserving identical routing + the CR-A5 unknown-command
// reject.
use commands::Registry;
// The DL-16 live-query delivery surface (`commands::watch`): the per-transaction
// delivered batch + the notification-stream replay helper, re-exported so the crate
// root (and the conformance harness) can drive the reactive loop end to end.
pub use commands::watch::{replay_notification_stream, DeliveredBatch};
use crate::event::EventSink;
use crate::run_policy::RunPolicy;
use crate::sync_rbac::{
    authorize_remote_op, RemoteOp, RemoteOpEnvelope, ResourceType, SyncAuthDecision,
    TrustedMembership,
};
use forge_domain::{CoreCommand, CoreError, CoreResponse, Result};
use forge_schema::SchemaRegistry;
use forge_storage::{IndexManager, Store};

/// The KV key (within [`META_NS`]) holding the persisted trusted `db.read` grant
/// table (actor id → readable collections). Persisted so a scoped grant survives
/// reopening the workspace file instead of fail-opening to read-all (review 050).
const DB_READ_GRANTS_KEY: &str = "db_read_grants";

/// The KV key (within [`META_NS`]) holding the persisted trusted `db.write` grant
/// table (actor id → writable collections). The write counterpart of
/// [`DB_READ_GRANTS_KEY`] — the authorization source for `db.restore` (DL-20: a
/// non-destructive restore is a record WRITE), set only through the trusted
/// [`grant_db_write`](WorkspaceCore::grant_db_write) seam and persisted so a scoped
/// grant survives reopening the workspace file (mirrors the `db.read` table, review
/// 050). An actor with no entry falls back to its role-derived write scope, so the
/// existing owner-permits-all spine is unaffected.
const DB_WRITE_GRANTS_KEY: &str = "db_write_grants";

/// The KV key (within [`META_NS`]) holding the persisted SS-7 sync membership
/// table: the receiver's TRUSTED role + collection grants for each remote sync
/// peer, keyed by the peer's sync source id (`peer:<loro_id>`). This is the
/// authoritative authorization source for applying a remote op (`forge/spec/
/// sync-rbac.md` "Trust boundary"), mirroring the persisted `db.read` grant table
/// (review 050): a seeded membership survives reopening the workspace file instead
/// of fail-opening to "no membership = allow".
const SYNC_MEMBERSHIP_KEY: &str = "sync_membership";

/// The KV key (within [`META_NS`]) holding the persisted [`SchemaRegistry`]
/// (serialized JSON). The dynamic schema is workspace state (DL-7/DL-8): a
/// collection/field defined via `schema.apply_change` must survive reopening the
/// workspace file, so the registry is loaded on [`WorkspaceCore::open`] /
/// [`in_memory`](WorkspaceCore::in_memory) the same way the `db.read` grant table
/// is (it mirrors [`load_db_read_grants`]).
///
/// Re-exported from `forge_storage` (one source of truth) because the DL-13 sync
/// receiver evolves the SAME persisted registry inside the migration import
/// transaction (review 143): if the two crates spelled this key differently, a synced
/// registry change would land under a key the open-time loader never reads.
const SCHEMA_REGISTRY_KEY: &str = forge_storage::SCHEMA_REGISTRY_KEY;

/// The KV key (within [`META_NS`]) holding the persisted trusted SC-10 run policy
/// — the workspace/run/platform inputs the live workspace-policy / run-profile /
/// platform-permission gates read (`forge/spec/policy-gates.md`, gates 2/4/5;
/// T037). Persisted like [`DB_READ_GRANTS_KEY`] / [`SYNC_MEMBERSHIP_KEY`] so a
/// configured policy survives reopening the workspace file, and read ONLY from
/// here at the run boundary, never from a request payload (review 048/050). Absent
/// ⇒ un-provisioned ⇒ the permissive `AllowAll` baseline (the M0a spine default).
const RUN_POLICY_KEY: &str = "run_policy";

/// An applet's dispatch lifecycle for the interactive UI loop (UI-4/CR-6).
///
/// `Active` is the default: a UI event re-enters the applet's handler. `Suspended`
/// is a receiver-side admin state in which `ui.dispatch_event` rejects every event
/// BEFORE any handler runs (the T034 `suspended_applet_rejected` vector) — a
/// suspended applet has no live UI session, so dispatching into it is a typed
/// `ui.applet_not_dispatchable` rejection with no state change. Set only through
/// the trusted [`set_applet_lifecycle`](WorkspaceCore::set_applet_lifecycle) seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppletLifecycle {
    /// The applet is live and re-entrant: a UI event dispatches its handler.
    #[default]
    Active,
    /// The applet is suspended: a UI event is rejected before dispatch.
    Suspended,
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
    /// Trusted `db.write` grant table: actor id → the collections that actor may
    /// write (`"*"` = write-all). The write counterpart of `db_read_grants`: the
    /// SOURCE OF TRUTH for the collection-scoped `db.write` capability that
    /// `db.restore` (DL-20) requires — a non-destructive restore appends a new record
    /// version, i.e. it is a record WRITE. Set only through the trusted
    /// [`grant_db_write`](WorkspaceCore::grant_db_write) seam (workspace configuration
    /// / membership), never from a request payload (review 048/050). An actor with NO
    /// entry falls back to its role-derived write scope, so the existing
    /// owner-permits-all spine is unaffected.
    db_write_grants: std::collections::BTreeMap<String, Vec<String>>,
    /// SS-7 sync membership table: the receiver's TRUSTED role + collection grants
    /// for each remote sync peer, keyed by the peer's sync **source id**
    /// (`peer:<loro_id>` — the authenticated session identity that reaches the apply
    /// boundary in-process). This is the SOURCE OF TRUTH for authorizing an
    /// incoming remote op (`forge/spec/sync-rbac.md`): the gate in
    /// [`sync_with`](WorkspaceCore::sync_with) resolves the row for the chunk's
    /// origin peer and calls [`authorize_remote_op`], never trusting the message.
    /// Mirrors `db_read_grants` (review 048/050): set only through the trusted
    /// [`set_peer_membership`](WorkspaceCore::set_peer_membership) seam and persisted
    /// to the workspace file so a seeded membership survives reopen. A peer with NO
    /// entry is UNKNOWN and every op it sends is denied (fail-closed).
    sync_membership: std::collections::BTreeMap<String, TrustedMembership>,
    /// Trusted SC-10 run policy: the workspace/run/platform inputs the LIVE
    /// workspace-policy / run-profile / platform-permission gates read on every
    /// `ctx.*` host call (`forge/spec/policy-gates.md`, gates 2/4/5; T037). This is
    /// the SOURCE OF TRUTH for those three gates and is set ONLY through the trusted
    /// [`set_run_policy`](WorkspaceCore::set_run_policy) seam (workspace
    /// configuration), never a request payload (review 048/050). Persisted to the
    /// workspace file so a configured policy survives reopen (mirrors
    /// `db_read_grants` / `sync_membership`).
    ///
    /// `None` ⇒ un-provisioned: the run installs the permissive `AllowAll` context
    /// (the M0a spine baseline — the demo and existing applets are unaffected). When
    /// `Some`, [`decision_context_for_run`](WorkspaceCore::decision_context_for_run)
    /// builds a real `ComposedDecisionContext` so a configured deny actually blocks
    /// the live command.
    run_policy: Option<RunPolicy>,
    /// Live-query (`db.watch`) session state (DL-16, `forge/spec/live-queries.md`):
    /// the registered watches (owning applet + callback handler + query) plus the
    /// workspace's monotone notification `version`. Loaded from `__forge/meta`
    /// (`watch_sessions`) on open so a registered watch — and the version sequence —
    /// survives reopen (mirrors `db_read_grants` / the schema registry), and
    /// re-persisted after every `db.watch`/`db.unwatch` and every delivered batch.
    /// The storage [`WatchRegistry`](forge_storage::WatchRegistry) substrate is
    /// rebuilt from this on demand to compute notification bytes; this facade owns
    /// DELIVERY (re-entering the right applet callback) + persistence.
    watch_sessions: WatchSessions,
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
    /// Factory for the `ctx.files` sandbox [`FileSystem`](forge_runtime::FileSystem)
    /// (prd-merged/01 CR-3, prd-merged/07 SC-8/SC-10/SC-12, `forge/spec/files.md`).
    /// Each `runtime.run` builds a fresh filesystem from this factory and hands it
    /// to the run's [`StorageHostBridge`], so the runtime can resolve a granted
    /// **handle** to its per-applet sandbox root and perform a *capability-checked,
    /// confined* read/write at the HOST edge. The trusted handle → root resolution
    /// lives in the filesystem (the manifest never names a native root), exactly as
    /// the `files` grant the runtime gates against rides on the TRUSTED manifest
    /// snapshot — not the request payload.
    ///
    /// The default factory yields an EMPTY in-memory filesystem — so no handle has
    /// a granted root and any `ctx.files` op fails closed (`PermissionDenied`) until
    /// a host/shell injects a real per-applet sandbox filesystem via
    /// [`set_file_system_factory`](WorkspaceCore::set_file_system_factory). A
    /// factory (not one shared filesystem) is used because the run's bridge owns its
    /// filesystem and the trait object is not `Clone`.
    file_system_factory: FileSystemFactory,
}

/// A factory that produces a fresh `ctx.net.fetch`
/// [`HttpClient`](forge_runtime::HttpClient) per run. See
/// [`WorkspaceCore::set_http_client_factory`].
type HttpClientFactory = Box<dyn Fn() -> Box<dyn forge_runtime::HttpClient>>;

/// A factory that produces a fresh `ctx.net.fetch`
/// [`SecretStore`](forge_runtime::SecretStore) per run. See
/// [`WorkspaceCore::set_secret_store_factory`].
type SecretStoreFactory = Box<dyn Fn() -> Box<dyn forge_runtime::SecretStore>>;

/// A factory that produces a fresh `ctx.files` sandbox
/// [`FileSystem`](forge_runtime::FileSystem) per run. See
/// [`WorkspaceCore::set_file_system_factory`].
type FileSystemFactory = Box<dyn Fn() -> Box<dyn forge_runtime::FileSystem>>;

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
        let db_write_grants = load_db_write_grants(&store)?;
        let sync_membership = load_sync_membership(&store)?;
        let run_policy = load_run_policy(&store)?;
        let registry = load_schema_registry(&store)?;
        let indexes = rebuild_indexes_from_registry(&store, &registry)?;
        let watch_sessions = load_watch_sessions(&store, META_NS)?;
        Ok(WorkspaceCore {
            store,
            registry,
            indexes,
            events: EventSink::new(),
            workspace_id: workspace_id.into(),
            db_read_grants,
            db_write_grants,
            sync_membership,
            run_policy,
            watch_sessions,
            // Fail-closed default: no live network. A host/shell opts in by
            // calling `set_http_client_factory` (review: keep the network seam
            // injectable so CI/the demo never reach the network).
            http_client_factory: Box::new(|| Box::new(crate::bridge::NoNetworkClient)),
            // Fail-closed default: an EMPTY secret store (every secret_ref denied)
            // until a host/shell injects a real one via `set_secret_store_factory`.
            secret_store_factory: Box::new(|| {
                Box::new(forge_runtime::InMemorySecretStore::new())
            }),
            // Fail-closed default: an EMPTY sandbox filesystem (no granted handle
            // root, so every ctx.files op is PermissionDenied) until a host/shell
            // injects a real per-applet sandbox filesystem via
            // `set_file_system_factory`.
            file_system_factory: Box::new(|| {
                Box::new(forge_runtime::InMemoryFileSystem::new())
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

    /// Inject the factory that builds the `ctx.files` sandbox
    /// [`FileSystem`](forge_runtime::FileSystem) for each `runtime.run`
    /// (prd-merged/01 CR-3, prd-merged/07 SC-8/SC-10/SC-12, `forge/spec/files.md`).
    /// A host/shell wires its real per-applet sandbox filesystem here; tests inject
    /// an [`InMemoryFileSystem`](forge_runtime::InMemoryFileSystem). Until this is
    /// called the workspace uses an EMPTY in-memory filesystem — no handle has a
    /// granted root — so any `ctx.files` op fails closed (`PermissionDenied`).
    ///
    /// The filesystem carries the **trusted** handle → per-applet-sandbox-root
    /// resolution (a handle with no granted root is denied), so the manifest never
    /// names a native root — mirroring how the `files` capability grant the runtime
    /// gates against rides on the TRUSTED manifest snapshot, never the request
    /// payload. The factory is invoked once per run so each run's bridge owns its
    /// own filesystem (the trait object is not `Clone`). The capability decision is
    /// still the runtime's: the injected filesystem is consulted only for an
    /// *allowed*, *record-mode* op whose path the runtime already confined to the
    /// handle's root (replay serves the recording; a denied op never reaches it).
    pub fn set_file_system_factory(
        &mut self,
        factory: impl Fn() -> Box<dyn forge_runtime::FileSystem> + 'static,
    ) {
        self.file_system_factory = Box::new(factory);
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

    /// Configure the TRUSTED `db.write` grant scope for `actor` (workspace
    /// membership / capability provisioning). The write counterpart of
    /// [`grant_db_read`](Self::grant_db_read): `scope` is the list of collections the
    /// actor may write; `"*"` grants write-all. This is the only way a caller's
    /// `db.write` scope is set — `db.restore` (DL-20) reads it from here, never from
    /// the request payload, so a shell cannot widen its own write scope by editing the
    /// command body (review 048/050). Passing an empty `scope` provisions an actor
    /// that holds the write role but is granted NO collection.
    ///
    /// The grant table is **persisted** to the workspace file: a scoped actor stays
    /// scoped after `open(...)`, instead of silently reverting to role-derived
    /// write-all (a fail-open regression).
    pub fn grant_db_write(
        &mut self,
        actor: impl Into<String>,
        scope: impl IntoIterator<Item = impl Into<String>>,
    ) -> Result<()> {
        self.db_write_grants
            .insert(actor.into(), scope.into_iter().map(Into::into).collect());
        let bytes = serde_json::to_vec(&self.db_write_grants)
            .map_err(|e| CoreError::StorageError(format!("serialize db.write grants: {e}")))?;
        self.store
            .kv_set(META_NS, DB_WRITE_GRANTS_KEY, &bytes, "application/json")?;
        Ok(())
    }

    /// Record an OWNER-approved capability GRANT to `target_actor` and persist a
    /// durable `permission.grant` audit row (SC-12, the `audit-log-e2e`
    /// `permission_grant_revoke_ordered_rows` vector). This is the real
    /// permission-management admin seam (a workspace-membership provisioning
    /// operation, alongside [`grant_db_read`](Self::grant_db_read) /
    /// [`set_peer_membership`](Self::set_peer_membership)): an owner approves a
    /// capability for an actor and the decision lands an append-only, queryable audit
    /// row through this live path, not merely a transient event.
    ///
    /// `(namespace, action, resource)` names the capability, e.g.
    /// `("db", "write", "collection:tasks")`; the audit row's `resource_id` is the
    /// canonical `"<namespace>.<action>:<resource>"` (`db.write:collection:tasks`) and
    /// its `collection` is parsed from a `collection:<name>` resource. Returns the
    /// persisted (seq-assigned, redacted) row.
    pub fn grant_capability(
        &mut self,
        owner_actor: impl Into<String>,
        target_actor: impl Into<String>,
        namespace: &str,
        action: &str,
        resource: &str,
    ) -> Result<forge_storage::AuditRecord> {
        self.persist_capability_decision(
            owner_actor.into(),
            target_actor.into(),
            namespace,
            action,
            resource,
            true,
        )
    }

    /// Record an OWNER-approved capability REVOKE from `target_actor` and persist a
    /// durable `permission.revoke` audit row (the mirror of
    /// [`grant_capability`](Self::grant_capability); SC-12). Re-running grant→revoke
    /// appends new rows with fresh seq/audit_id and never mutates prior history.
    pub fn revoke_capability(
        &mut self,
        owner_actor: impl Into<String>,
        target_actor: impl Into<String>,
        namespace: &str,
        action: &str,
        resource: &str,
    ) -> Result<forge_storage::AuditRecord> {
        self.persist_capability_decision(
            owner_actor.into(),
            target_actor.into(),
            namespace,
            action,
            resource,
            false,
        )
    }

    /// Shared body for [`grant_capability`] / [`revoke_capability`]: build and persist
    /// the `permission.grant` / `permission.revoke` audit row. `grant = true` selects
    /// the grant action/reason, `false` the revoke. The metadata carries the target
    /// actor + the capability namespace/action (no secret material), so redaction is a
    /// no-op.
    fn persist_capability_decision(
        &mut self,
        owner_actor: String,
        target_actor: String,
        namespace: &str,
        action: &str,
        resource: &str,
        grant: bool,
    ) -> Result<forge_storage::AuditRecord> {
        let resource_id = format!("{namespace}.{action}:{resource}");
        // A `collection:<name>` resource carries the collection name in the audit row.
        let collection = resource
            .strip_prefix("collection:")
            .map(|name| name.to_string());
        let (audit_action, event_kind, reason) = if grant {
            ("permission.grant", "permission.granted", "owner approved capability grant")
        } else {
            ("permission.revoke", "permission.revoked", "owner revoked capability grant")
        };
        self.persist_producer_audit(
            event_kind,
            serde_json::json!({
                "decision": "allow",
                "owner_actor": owner_actor,
                "target_actor_id": target_actor,
                "namespace": namespace,
                "capability_action": action,
                "resource": resource,
            }),
            "permission-manager",
            audit_action,
            "allow",
            owner_actor,
            "capability",
            Some(resource_id),
            collection,
            reason,
            serde_json::json!({
                "target_actor_id": target_actor,
                "namespace": namespace,
                "capability_action": action,
            }),
        )
    }

    /// Seed/replace the TRUSTED SS-7 membership row for a remote sync `peer`
    /// (workspace membership provisioning). `peer` is the peer's sync **source id**
    /// — `peer:<loro_id>`, the form the apply boundary sees (see
    /// [`source_id_for`]). `membership` is the role + collection grants the
    /// receiver trusts for that peer.
    ///
    /// This is the ONLY way the sync authorization table is set: the apply-time
    /// gate in [`sync_with`](WorkspaceCore::sync_with) reads it from here, never
    /// from the incoming message, so a peer cannot widen its own grants by claiming
    /// a role (`forge/spec/sync-rbac.md` "Trust boundary"; mirrors `grant_db_read`,
    /// review 048/050). The table is **persisted** to the workspace file, so a
    /// seeded membership survives `open(...)`. A peer with NO row is unknown and
    /// every op it sends is denied.
    ///
    /// Convenience: pass the remote peer's
    /// [`crdt_peer_id`](forge_storage::Store::crdt_peer_id) via [`source_id_for`]
    /// to build the key, e.g. `set_peer_membership(source_id_for(other_loro_id), m)`.
    pub fn set_peer_membership(
        &mut self,
        peer: impl Into<String>,
        membership: TrustedMembership,
    ) -> Result<()> {
        self.sync_membership.insert(peer.into(), membership);
        let bytes = serde_json::to_vec(&self.sync_membership)
            .map_err(|e| CoreError::StorageError(format!("serialize sync membership: {e}")))?;
        self.store
            .kv_set(META_NS, SYNC_MEMBERSHIP_KEY, &bytes, "application/json")?;
        Ok(())
    }

    /// The trusted SS-7 membership row seeded for sync `peer` (`peer:<loro_id>`),
    /// if any. Read-only access for tests / diagnostics.
    pub fn peer_membership(&self, peer: &str) -> Option<&TrustedMembership> {
        self.sync_membership.get(peer)
    }

    /// Configure the TRUSTED SC-10 [`RunPolicy`] for this workspace — the
    /// workspace/run/platform inputs the LIVE workspace-policy / run-profile /
    /// platform-permission gates read on every `ctx.*` host call
    /// (`forge/spec/policy-gates.md`, gates 2/4/5; T037).
    ///
    /// This is the ONLY way those three gates are configured: `runtime.run` /
    /// `ui.dispatch_event` / live-query delivery read the policy from here, never
    /// from a request payload, so an applet (or a shell) cannot widen its own
    /// grants by editing the command body (review 048/050). The policy is
    /// **persisted** to the workspace file (mirrors
    /// [`grant_db_read`](Self::grant_db_read) /
    /// [`set_peer_membership`](Self::set_peer_membership)), so a configured policy
    /// survives `open(...)`.
    ///
    /// Until this is called the workspace is un-provisioned and the run installs
    /// the permissive `AllowAll` context — the M0a spine baseline (the demo and
    /// existing applets are unaffected). Once set, a configured deny actually blocks
    /// the live command (the gate is consulted on the real decision path, not a
    /// tested-but-disconnected library).
    pub fn set_run_policy(&mut self, policy: RunPolicy) -> Result<()> {
        let bytes = serde_json::to_vec(&policy)
            .map_err(|e| CoreError::StorageError(format!("serialize run policy: {e}")))?;
        self.store
            .kv_set(META_NS, RUN_POLICY_KEY, &bytes, "application/json")?;
        self.run_policy = Some(policy);
        Ok(())
    }

    /// The trusted SC-10 [`RunPolicy`] configured for this workspace, if any.
    /// Read-only access for tests / diagnostics. `None` ⇒ un-provisioned (the
    /// permissive `AllowAll` baseline).
    pub fn run_policy(&self) -> Option<&RunPolicy> {
        self.run_policy.as_ref()
    }

    /// Build the live [`DecisionContext`](forge_runtime::DecisionContext) to install
    /// on this run's record entry point — the SC-10 workspace-policy / run-profile /
    /// platform-permission gates (T037).
    ///
    /// When a [`RunPolicy`] is provisioned this returns a real
    /// [`ComposedDecisionContext`](forge_runtime::ComposedDecisionContext) reading
    /// the trusted workspace/run/platform state, so a configured deny blocks the
    /// live command. When un-provisioned it returns the permissive `AllowAll`
    /// default (the M0a spine baseline). The context reads ONLY trusted state, never
    /// the request payload (review 048/050). The same context is used by `runtime.run`,
    /// `ui.dispatch_event`, and live-query notification delivery so every live `ctx.*`
    /// path is gated identically.
    fn decision_context_for_run(&self) -> Box<dyn forge_runtime::DecisionContext> {
        match &self.run_policy {
            Some(policy) => policy.to_decision_context(),
            None => Box::new(forge_runtime::AllowAll),
        }
    }

    /// Set an applet's TRUSTED dispatch lifecycle (UI-4/CR-6): `Active` (the
    /// default, re-entrant) or `Suspended` (a UI event is rejected before any
    /// handler runs). This is a workspace-membership/admin operation, never a
    /// request payload — `ui.dispatch_event` reads the flag from here, so an applet
    /// cannot un-suspend itself by sending an event (mirrors `grant_db_read` /
    /// `set_peer_membership`, review 048/050). The flag is **persisted** to the
    /// workspace file, so a suspended applet stays suspended after `open(...)`.
    pub fn set_applet_lifecycle(
        &mut self,
        applet_id: impl AsRef<str>,
        lifecycle: AppletLifecycle,
    ) -> Result<()> {
        set_applet_lifecycle(&mut self.store, applet_id, lifecycle)
    }

    /// An applet's dispatch lifecycle, defaulting to [`AppletLifecycle::Active`] for
    /// an applet that was never explicitly suspended. Read-only access for tests /
    /// the `ui.dispatch_event` gate.
    pub fn applet_lifecycle(&self, applet_id: &str) -> Result<AppletLifecycle> {
        get_applet_lifecycle(&self.store, applet_id)
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

    /// Rebuild the derived `records` projection purely from the CRDT source of
    /// truth (DL-6): drop the projection and rematerialize it from `crdt_chunks`
    /// against this workspace's own index manager. Used by in-process tooling and
    /// tests to prove a durable write (e.g. a DL-13 record migration) survives a
    /// rematerialization — the migration mutates the chunk stream, so a rebuild
    /// reproduces the migrated values rather than the pre-migration state. The
    /// disjoint borrow of `store` + `indexes` is internal to the facade (the two
    /// are private fields, so a caller cannot borrow both at once through the
    /// public accessors).
    pub fn rebuild_projection(&mut self) -> Result<()> {
        self.store.rebuild_projection(&self.indexes)
    }

    /// Compact this workspace's CRDT history (DL-19), optionally enforcing the DL-20
    /// retention window carried on `opts` ([`forge_storage::CompactionOptions::with_retention`]):
    /// fold safely-folded chunks into a compact snapshot while PROTECTING the
    /// most-recent `window` logical versions of the change-feed/oplog from pruning
    /// (`forge/spec/time-travel.md` §4). The projection is unchanged (the DL-19
    /// invariant); only the standalone change-feed entries beyond the window are
    /// pruned. The disjoint borrow of `store` + `indexes` is internal to the facade
    /// (mirrors [`rebuild_projection`](Self::rebuild_projection)). Returns the
    /// compaction report (chunks folded, oplog rows removed).
    pub fn compact_history(
        &mut self,
        opts: &forge_storage::CompactionOptions,
    ) -> Result<forge_storage::CompactionReport> {
        self.store.compact_history(opts, &self.indexes)
    }

    /// In-process CRDT sync (SS-1/SS-2, M0b): converge this workspace with
    /// `other` by exchanging the chunk sets their two [`Store`]s hold, then
    /// rebuilding both projections — the local CI seam before WebSocket transport
    /// / relay / server RBAC exist (those are deferred; both peers are assumed
    /// already authorized here).
    ///
    /// Delegates to [`forge_sync::sync_stores`] over the two stores: for the union
    /// of `collection/<name>` docs it diffs the per-doc content-addressed chunk
    /// frontiers, sends each peer the chunks the other lacks (append-only,
    /// idempotent), and rebuilds the records projection on both. Afterwards the
    /// two workspaces hold the same chunk set per doc and their projections
    /// converge (DL-9): independent collections and different records merge,
    /// concurrent edits to different fields of one record both survive, and a
    /// concurrent same-scalar write resolves to one Loro LWW winner both peers
    /// agree on. Idempotent — a second `sync_with` over an already-converged pair
    /// moves zero chunks.
    ///
    /// For two peers' concurrent same-scalar edits to converge to one agreed
    /// winner the two stores must mint CRDT ops under DISTINCT Loro peer ids
    /// ([`Store::set_crdt_peer_id`](forge_storage::Store::set_crdt_peer_id));
    /// callers that build peers for sync set this (the demo/single-writer default
    /// is the shared local id, which is fine until two workspaces are synced).
    pub fn sync_with(&mut self, other: &mut WorkspaceCore) -> Result<forge_sync::SyncReport> {
        // Each store rebuilds its projection against its OWN index manager: this
        // workspace's `self.indexes` for `self.store`, `other.indexes` for
        // `other.store` (review 084 #1). Index metadata is per-workspace and NOT
        // part of the synced (chunk) payload, so the two peers may hold asymmetric
        // active indexes (e.g. one has an FTS index the other lacks). Passing a
        // single manager for both rebuilds would be order-dependent and wrong —
        // issuing index DML against tables the other store lacks, or skipping the
        // indexes it does have and leaving them stale. Per-store managers keep both
        // projections materialized from canonical chunks with each store's own
        // indexes intact (DL-6).
        //
        // SS-7: every incoming op is authorized against the RECEIVER's trusted
        // membership table BEFORE its chunk is imported (`forge/spec/sync-rbac.md`).
        // The exchange is symmetric, so each direction has a distinct receiver:
        //   - `self.store` RECEIVES `other`'s chunks (source `peer:<other_loro>`),
        //     authorized against `self.sync_membership`;
        //   - `other.store` RECEIVES `self`'s chunks (source `peer:<self_loro>`),
        //     authorized against `other.sync_membership`.
        // A denied op's chunk is SKIPPED before [`forge_sync`] hands the batch to
        // the atomic per-store import, so the receiver's CRDT history + projection
        // are unchanged; an audit denial is recorded and a `permission_denied` is
        // surfaced via the receiver's event sink + the report's `chunks_denied`.
        //
        // Disjoint-borrow the fields so the authorize closure can hold the two
        // membership tables + event sinks while `forge_sync` holds the two stores.
        let self_source = source_id_for(self.store.crdt_peer_id());
        let other_source = source_id_for(other.store.crdt_peer_id());

        let report = {
            let WorkspaceCore {
                store: self_store,
                indexes: self_indexes,
                events: self_events,
                sync_membership: self_membership,
                run_policy: self_run_policy,
                ..
            } = self;
            let WorkspaceCore {
                store: other_store,
                indexes: other_indexes,
                events: other_events,
                sync_membership: other_membership,
                run_policy: other_run_policy,
                ..
            } = other;

            // SC-12 review 149: the authorize gate pushes each receiver's audit row
            // into the per-direction `audit` sink `forge_sync` hands it, and
            // `forge_sync` appends that sink IN THE SAME TRANSACTION as the receiver's
            // import. So a committed authorization decision and its durable `audit_log`
            // row commit (or roll back) together — no crash window between them. The
            // EventSink emission is UNCHANGED (the transient event still fires); only
            // the durable persistence moved inside the import txn.
            forge_sync::sync_stores_authorized(
                self_store,
                self_indexes,
                other_store,
                other_indexes,
                |source, envelope, audit| {
                    // `source` is the RELAY peer the chunk arrived from; it selects the
                    // RECEIVER (the other side) for the direction. But the ACTOR whose
                    // trusted membership decides authorization is the chunk's ORIGINAL
                    // author: a chunk `source` merely forwarded (its `origin_source` is
                    // set from the remote-import provenance) must be gated against that
                    // original author, not the relay (`review 092 #1` / SS-7 actor
                    // identity). A locally-authored chunk has no `origin_source`, so the
                    // relay IS the author and `actor == source`.
                    //
                    // The `audit` sink is the RECEIVING store's audit batch: `forge_sync`
                    // routes the sink for the `other`-receives direction into `other`'s
                    // import txn and the `self`-receives direction into `self`'s, so the
                    // decision lands in the receiver's own durable log.
                    let actor = envelope.origin_source.as_deref().unwrap_or(source);
                    if source == self_source {
                        // Direction: self → received by `other`. Authorize against
                        // `other`'s table for the original author; the durable row
                        // lands in `other`'s log. The receiver's OWN trusted
                        // `RunPolicy` supplies the SC-10 workspace-policy gate, so a
                        // category `other` forbids blocks the import even when its
                        // membership would allow it (SS-7).
                        authorize_incoming_op(
                            other_membership,
                            other_run_policy.as_ref(),
                            other_events,
                            audit,
                            actor,
                            envelope,
                        )
                    } else {
                        // Direction: other → received by `self`.
                        debug_assert_eq!(source, other_source);
                        authorize_incoming_op(
                            self_membership,
                            self_run_policy.as_ref(),
                            self_events,
                            audit,
                            actor,
                            envelope,
                        )
                    }
                },
            )?
        };

        // DL-13 review 143: an authorized migration chunk evolved the RECEIVER's
        // PERSISTED `SchemaRegistry` (and `schema_version`) inside the import txn — but
        // each side's IN-MEMORY `registry` handle (loaded at open) is now stale. The
        // exchange is symmetric, so EITHER peer may have received a migration; reload
        // both in-memory registries from their stores so `registry()` reflects the
        // synced schema, and rebuild each index manager from its refreshed registry so a
        // newly-indexed field reconstructed on the receiver becomes a live planner
        // candidate (mirrors `from_store`'s open-time reconstruction). This is the only
        // mutation outside the import txn, and it cannot diverge from the durable state:
        // it re-derives the in-memory handles purely FROM the just-committed store.
        self.refresh_schema_from_store()?;
        other.refresh_schema_from_store()?;
        Ok(report)
    }

    /// Reload this workspace's in-memory [`SchemaRegistry`] and rebuild its
    /// [`IndexManager`] purely from the persisted store (DL-13 review 143). Called
    /// after [`sync_with`](Self::sync_with) so an authorized migration chunk that
    /// evolved the persisted registry + `schema_version` inside the import transaction
    /// is reflected in `registry()` and in the active index set, instead of leaving the
    /// receiver running validation / index reconstruction / later schema changes
    /// against a stale registry. Re-derives the handles from the just-committed durable
    /// state, so it can never drift from it (mirrors [`from_store`]'s open-time load).
    fn refresh_schema_from_store(&mut self) -> Result<()> {
        self.registry = load_schema_registry(&self.store)?;
        self.indexes = rebuild_indexes_from_registry(&self.store, &self.registry)?;
        Ok(())
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
        // CR-A3 ordering, UNCHANGED: command-level RBAC runs FIRST; only on `Ok(())`
        // does dispatch proceed. The former hand-written match is now the command
        // `Registry` (the catalog as data, built once): `dispatch` routes a
        // registered name to the SAME `cmd_*` handler and rejects an unregistered
        // name with the IDENTICAL CR-A5 `ValidationError`. Authorization, the
        // lifecycle suspension gate (inside `cmd_ui_dispatch_event`), and every
        // handler body are untouched — only the match-vs-table shape changed
        // (/simplify #11b).
        // CR-A3 command-level RBAC. A denial here is the command-RBAC producer for
        // the durable audit log (SC-12): before returning the `PermissionDenied`
        // response, persist an append-only `audit_log` row through the live path so
        // a real role-denied command is queryable, not merely a transient error.
        if let Err(error) = authorize(&cmd) {
            if matches!(error, CoreError::PermissionDenied(_)) {
                // Best-effort: a denial is the security signal; if the durable append
                // itself errors we still surface the denial (we never fail OPEN on an
                // audit-persistence error, and we never turn a deny into an allow).
                let _ = self.persist_command_rbac_denial(&cmd, &error);
            }
            return CoreResponse::err(request_id, error);
        }
        match command_registry().dispatch(self, &cmd) {
            Ok(payload) => CoreResponse::ok(request_id, payload),
            Err(error) => CoreResponse::err(request_id, error),
        }
    }

    /// Append a command-RBAC denial to the durable audit log (SC-12 live wiring).
    /// The row mirrors the `audit-log-e2e` `command_rbac_denial_query_actor` shape:
    /// `producer = command-rbac`, `action = command.<name>`, `decision = deny`, the
    /// authenticated `actor_id`, `resource_type = command`, `resource_id = <name>`,
    /// the denial reason, and `metadata = {role, command, applet_id?}` (no secret
    /// value / body, so redaction is a no-op). The `logical_time` is the EventSink
    /// clock so the durable row replays deterministically; the append rides its own
    /// [`Store::transact`] so a real denied command lands one queryable row.
    fn persist_command_rbac_denial(
        &mut self,
        cmd: &CoreCommand,
        error: &CoreError,
    ) -> Result<()> {
        let action = format!("command.{}", cmd.name);
        let reason = match error {
            CoreError::PermissionDenied(msg) => msg.clone(),
            other => other.to_string(),
        };
        let mut metadata = serde_json::Map::new();
        metadata.insert(
            "role".to_string(),
            serde_json::json!(format!("{:?}", cmd.actor.role)),
        );
        metadata.insert("command".to_string(), serde_json::json!(cmd.name));
        if let Some(applet) = &cmd.applet_id {
            metadata.insert("applet_id".to_string(), serde_json::json!(applet.as_str()));
        }
        let logical_time = emit_event_logical_time(
            &mut self.events,
            "command.permission_denied",
            serde_json::json!({
                "decision": "deny",
                "command": cmd.name,
                "actor_id": cmd.actor.actor.as_str(),
                "role": format!("{:?}", cmd.actor.role),
                "reason": reason,
            }),
        );
        let record = forge_storage::AuditRecord::new(
            logical_time,
            "command-rbac",
            action,
            "deny",
            cmd.actor.actor.as_str(),
            "command",
            Some(cmd.name.clone()),
            None,
            reason,
            serde_json::Value::Object(metadata),
        );
        self.store.append_audit(&record).map(|_| ())
    }

    /// Append ONE producer audit row to the durable SC-12 log through the live path
    /// (`forge/spec/audit-log.md`), stamping the `logical_time` the EventSink minted
    /// for `event_kind`+`event_payload` so the transient event and the persisted row
    /// share one deterministic clock — no wall clock on the replayable path. This is
    /// the shared seam the NON-RBAC producers (secrets, network, lifecycle purge,
    /// signing refusal, permission grant/revoke) emit through: each builds its row
    /// (resource type, decision, redactable metadata) and hands it here, so the
    /// append + redaction chokepoint is identical across producers. Redaction runs at
    /// persistence ([`Store::append_audit`] → `redact_metadata`), so a secret value or
    /// a request/response body the caller leaves in `metadata` is still dropped.
    ///
    /// Append-only: this never updates or deletes a prior row; re-running a producer
    /// mints a fresh `seq`/`audit_id`. Returns the persisted (redacted, seq-assigned)
    /// row so the caller can echo or assert it.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn persist_producer_audit(
        &mut self,
        event_kind: &str,
        event_payload: serde_json::Value,
        producer: &str,
        action: impl Into<String>,
        decision: &'static str,
        actor_id: impl Into<String>,
        resource_type: &str,
        resource_id: Option<String>,
        collection: Option<String>,
        reason: impl Into<String>,
        metadata: serde_json::Value,
    ) -> Result<forge_storage::AuditRecord> {
        let record = self.build_producer_audit_record(
            event_kind,
            event_payload,
            producer,
            action,
            decision,
            actor_id,
            resource_type,
            resource_id,
            collection,
            reason,
            metadata,
        );
        self.store.append_audit(&record)
    }

    /// Build (but do NOT append) a producer audit row, emitting the transient event
    /// so the durable row carries the SAME EventSink `logical_time` as
    /// [`persist_producer_audit`] would. This is the seam a producer uses when the
    /// audit append must commit in the SAME `Store::transact` as the durable decision
    /// it records (spec/audit-log.md §2 — "a committed decision always lands its
    /// row"): the caller mints the row(s) here, then folds them into its own
    /// transaction via [`Store::append_audit_tx`](forge_storage::Store::append_audit_tx),
    /// which is also where `metadata` is redacted. Used by the lifecycle-purge
    /// uninstall and the `runtime.run` egress producers (whose `allow` rows must be
    /// atomic with the tombstone writes / the `save_run`, respectively).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn build_producer_audit_record(
        &mut self,
        event_kind: &str,
        event_payload: serde_json::Value,
        producer: &str,
        action: impl Into<String>,
        decision: &'static str,
        actor_id: impl Into<String>,
        resource_type: &str,
        resource_id: Option<String>,
        collection: Option<String>,
        reason: impl Into<String>,
        metadata: serde_json::Value,
    ) -> forge_storage::AuditRecord {
        let logical_time = emit_event_logical_time(&mut self.events, event_kind, event_payload);
        forge_storage::AuditRecord::new(
            logical_time,
            producer,
            action,
            decision,
            actor_id,
            resource_type,
            resource_id,
            collection,
            reason,
            metadata,
        )
    }

    /// Build a producer audit row stamped with an EXPLICIT `logical_time` and WITHOUT
    /// emitting the transient event. The DEFERRED-EMIT seam (vs.
    /// [`build_producer_audit_record`], which emits during build): a producer whose
    /// decision can ROLL BACK as a normal outcome (the lifecycle-purge uninstall,
    /// whose tombstone txn may fail) peeks the next `logical_time`
    /// ([`EventSink::peek_next_logical_time`]), builds the row here, appends it inside
    /// the SAME `Store::transact` as the mutation, and emits the matching
    /// observability event ONLY after that transaction COMMITS. So a rolled-back
    /// decision persists no row AND emits no spurious event, while a committed one
    /// keeps the transient event and the durable row under one clock (SC-12 §2). The
    /// peeked timestamp matches the post-commit `emit` exactly because nothing else
    /// emits between the peek and that emit.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::workspace) fn build_producer_audit_record_at(
        &self,
        logical_time: u64,
        producer: &str,
        action: impl Into<String>,
        decision: &'static str,
        actor_id: impl Into<String>,
        resource_type: &str,
        resource_id: Option<String>,
        collection: Option<String>,
        reason: impl Into<String>,
        metadata: serde_json::Value,
    ) -> forge_storage::AuditRecord {
        forge_storage::AuditRecord::new(
            logical_time,
            producer,
            action,
            decision,
            actor_id,
            resource_type,
            resource_id,
            collection,
            reason,
            metadata,
        )
    }

    // ---------------------------------------------------------------- commands

    /// `workspace.create` — in M0a the store is created on open, so this reports
    /// the workspace identity + the base logical version (CR-A2; M0b adds
    /// templates/owner wiring).
    pub(in crate::workspace) fn cmd_workspace_create(
        &mut self,
        _cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "root_version": 0,
        }))
    }

    /// `workspace.open` — report workspace metadata + the current logical clock
    /// (CR-A2). The file is already open (this core wraps one workspace file).
    pub(in crate::workspace) fn cmd_workspace_open(
        &mut self,
        _cmd: &CoreCommand,
    ) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "logical_clock": self.events.len(),
        }))
    }

    // -------------------------------------------------- last-known UI tree base

    /// Persist `tree` as the applet's last-known UI tree (the diff base for the
    /// next UI event), keyed by applet id within [`META_NS`]. Written after every
    /// accepted render through this facade — a `runtime.run`'s last render and each
    /// accepted `ui.dispatch_event` — so the interactive loop's diff base survives
    /// reopening the workspace (UI-4/CR-6).
    fn store_ui_tree(&mut self, applet_id: &str, tree: &serde_json::Value) -> Result<()> {
        store_ui_tree(&mut self.store, applet_id, tree)
    }

    /// Load the applet's last-known UI tree (the diff base) as a [`forge_ui::Node`],
    /// if one was recorded. `None` ⇒ the applet has not rendered through this facade
    /// yet, so the next render's diff is a single root replace (UI-1).
    fn load_ui_tree(&self, applet_id: &str) -> Result<Option<forge_ui::Node>> {
        load_ui_tree(&self.store, applet_id)
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
        next_run_counter(&mut self.store)
    }
}

/// The process-wide command [`Registry`], built ONCE on first dispatch and reused
/// for every [`WorkspaceCore::handle`]. The registry holds only the static command
/// catalog (a `&'static` table) and no per-workspace state, so one shared instance
/// routes every workspace's commands identically. Lazily initialized via a
/// [`OnceLock`](std::sync::OnceLock) so construction happens exactly once.
fn command_registry() -> &'static Registry {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(Registry::new)
}

/// The sync **source id** for a Loro peer id — the form
/// [`forge_sync`](forge_sync::sync_stores_authorized) tags an incoming chunk with
/// and the key into the [`set_peer_membership`](WorkspaceCore::set_peer_membership)
/// table: `peer:<loro_id>`. Kept in lockstep with `forge_sync`'s internal
/// `remote_source_id` so a seeded membership matches the source the gate sees.
pub fn source_id_for(loro_peer_id: u64) -> String {
    format!("peer:{loro_peer_id}")
}

/// The SS-7 apply-time authorization gate for ONE incoming remote op, run by
/// [`WorkspaceCore::sync_with`] for each staged chunk BEFORE it is imported
/// (`forge/spec/sync-rbac.md` "Apply-time decision order"). Resolves the
/// receiver's TRUSTED membership row for the chunk's origin `source`, calls
/// [`authorize_remote_op`], records an audit row on the receiver's `events` sink
/// (allow AND deny), and returns `true` to import the chunk / `false` to skip it.
///
/// A `source` with NO membership row is UNKNOWN: the op is denied fail-closed (the
/// receiver never seeded trust for that peer), an audit denial is written, and the
/// chunk is skipped. This mirrors the command boundary's trusted-grant model
/// (review 048/050): authorization comes only from the receiver-side table.
///
/// SC-12 live wiring (review 149): in addition to the transient `EventSink` audit
/// event, every decision (allow AND deny) is pushed onto `audit` — the per-direction
/// [`forge_storage::AuditRecord`] sink `forge_sync` threads into the RECEIVER's
/// import transaction. So the decision and its durable `audit_log` row commit (or
/// roll back) TOGETHER with the import it authorizes — no crash window where a
/// committed decision lacks its row. The `logical_time` is the SAME value the
/// EventSink minted for this decision, so the transient event and the durable row
/// share one deterministic clock.
///
/// SC-10 workspace-policy gate at the sync boundary (T037 / reviews 164/165): the
/// remote-sync boundary evaluates the SYNC-APPLICABLE SC-10 gates — the SS-7 RBAC
/// role/membership decision PLUS the receiver's trusted workspace-policy capability
/// gate per category. The receiver's `run_policy` gates an incoming op for the
/// CHUNK'S category (review 165, [`sync_op_policy_category`]): a remote record write
/// is a `Db`-category op, so a receiver whose `RunPolicy` forbids that category SKIPS
/// the chunk even when its `sync_membership` would allow it — and a policy denying a
/// DIFFERENT category does NOT skip it. The gate runs BEFORE membership resolution so
/// a workspace-policy deny is the first failing gate (SC-10 order: workspace-policy
/// precedes the role/grant checks membership RBAC plays).
///
/// The run-profile, platform-permission, manifest, resource-allowlist, and
/// rate/resource-limit gates are RUN-scoped: a passive chunk import is NOT a run — it
/// has no run profile, requests no manifest capability, and uses no OS platform
/// permission — so those gates do not apply here (normative carve-out in
/// `prd-merged/07-security-prd.md` SC-10 / `prd-merged/DECISIONS.md`; see
/// [`RunPolicy::workspace_policy_gate`](crate::RunPolicy::workspace_policy_gate)). A
/// DL-13 migration chunk's schema authority is gated by the membership `schema_write`
/// RBAC (review 143), the sync-applicable gate for the schema dimension.
fn authorize_incoming_op(
    membership: &std::collections::BTreeMap<String, TrustedMembership>,
    run_policy: Option<&RunPolicy>,
    events: &mut EventSink,
    audit: &mut Vec<forge_storage::AuditRecord>,
    source: &str,
    envelope: &forge_sync::SyncOpEnvelope,
) -> bool {
    // A chunk whose doc id is not a valid `collection/<name>` records doc (or whose
    // staged envelope is otherwise unfit) is denied fail-closed BEFORE membership
    // resolution: the apply path must reject a malformed chunk rather than guess a
    // collection (`review 092 #2` / `forge/spec/sync-rbac.md` line 52). Surface a
    // permission_denied audit naming the staging defect so the skip is observable.
    if let Some(reason) = &envelope.malformed {
        let logical_time = emit_event_logical_time(
            events,
            "sync.permission_denied",
            serde_json::json!({
                "decision": "deny",
                "source": source,
                "collection": envelope.collection,
                "reason": reason,
            }),
        );
        // An unknown-actor (no resolved trusted row) denial: persist what the
        // envelope names so the durable row still identifies the rejected resource.
        let collection = collection_opt(&envelope.collection);
        audit.push(forge_storage::AuditRecord::new(
            logical_time,
            SYNC_RBAC_PRODUCER,
            envelope_action(envelope),
            "deny",
            source,
            "record",
            collection.clone(),
            collection,
            reason.clone(),
            serde_json::json!({ "source": source, "reason": reason }),
        ));
        return false;
    }
    // SC-10 workspace-policy gate (gate 2), evaluated FIRST so a workspace-policy deny
    // wins over a membership-RBAC denial (first-failing-gate order). The gate runs for
    // the CHUNK'S capability category (review 165) — a record write is a `Db`-category
    // op — so a receiver whose policy forbids a category skips an incoming chunk of THAT
    // category, not only `Db`. An un-provisioned receiver (`None`) imposes no SC-10 deny.
    if let Some(policy) = run_policy {
        let category = sync_op_policy_category(envelope);
        if let Err(reason) = policy.workspace_policy_gate(category) {
            let logical_time = emit_event_logical_time(
                events,
                "sync.permission_denied",
                serde_json::json!({
                    "decision": "deny",
                    "source": source,
                    "collection": envelope.collection,
                    "reason": reason,
                }),
            );
            let collection = collection_opt(&envelope.collection);
            audit.push(forge_storage::AuditRecord::new(
                logical_time,
                SYNC_RBAC_PRODUCER,
                envelope_action(envelope),
                "deny",
                source,
                "record",
                collection.clone(),
                collection,
                reason.clone(),
                serde_json::json!({ "source": source, "reason": reason, "gate": "workspace-policy" }),
            ));
            return false;
        }
    }
    let env = remote_op_envelope_from_sync(envelope);
    match membership.get(source) {
        Some(trusted) => {
            // The trusted row is authoritative. In-process M0b carries no separate
            // session claim, so `claim = None` (a claim could only narrow, never
            // widen, the decision — `forge/spec/sync-rbac.md`).
            let decision = authorize_remote_op(trusted, None, &env);
            let logical_time = emit_sync_audit(events, source, &decision);
            audit.push(sync_audit_record(logical_time, source, &decision, &env));
            decision.is_allow()
        }
        None => {
            // Unknown peer: fail closed. Surface a permission_denied audit naming
            // the missing trust so the skip is observable.
            let reason = "no trusted membership for sync peer";
            let logical_time = emit_event_logical_time(
                events,
                "sync.permission_denied",
                serde_json::json!({
                    "decision": "deny",
                    "source": source,
                    "collection": envelope.collection,
                    "reason": reason,
                }),
            );
            let collection = collection_opt(&envelope.collection);
            audit.push(forge_storage::AuditRecord::new(
                logical_time,
                SYNC_RBAC_PRODUCER,
                envelope_action(envelope),
                "deny",
                source,
                "record",
                collection.clone(),
                collection,
                reason,
                serde_json::json!({ "source": source, "reason": reason }),
            ));
            false
        }
    }
}

/// The audit `producer` string for the sync-RBAC apply-boundary decision
/// (`forge/spec/audit-log.md`). Matches the `audit-log-e2e` fixtures' `producer`.
const SYNC_RBAC_PRODUCER: &str = "sync-rbac";

/// `Some(collection)` when the staged envelope names a non-empty collection, else
/// `None` (a malformed chunk has an empty `collection`, so the durable audit row's
/// nullable `resource_id`/`collection` columns are `NULL` rather than `""`).
fn collection_opt(collection: &str) -> Option<String> {
    (!collection.is_empty()).then(|| collection.to_string())
}

/// The audit `action` for an incoming sync op, derived from the staged envelope's
/// resource/op so a malformed or unknown-peer denial (which never builds a
/// [`RemoteOpEnvelope`]) still records what was attempted. Record ops are the only
/// resource the M0b chunk boundary carries; the op selects the suffix.
fn envelope_action(envelope: &forge_sync::SyncOpEnvelope) -> &'static str {
    match envelope.op {
        forge_sync::SyncRecordOp::Insert | forge_sync::SyncRecordOp::Write => {
            "sync.record.insert"
        }
        forge_sync::SyncRecordOp::Patch => "sync.record.patch",
        forge_sync::SyncRecordOp::Delete => "sync.record.delete",
    }
}

/// The SC-10 workspace-policy capability **category** for one incoming chunk
/// (review 165): the category the receiver's `RunPolicy` workspace-policy gate
/// (gate 2) decides over for THIS op — not a hardcoded `Db`. The gate then skips an
/// incoming chunk whose category the receiver's policy forbids, for EVERY category
/// the chunk boundary carries, not only `Db`.
///
/// Mapping (`prd-merged/07-security-prd.md` SC-10 sync carve-out):
/// - a record write (insert/patch/delete, or a generic transact write) targets the
///   workspace **database**, so it is a [`Db`](crate::Capability::Db)-category op;
/// - a DL-13 **migration** chunk is a SCHEMA-affecting op. The workspace-policy gate
///   has no separate `Schema` category, and schema-change authority is already gated
///   by the membership-RBAC `schema_write` check (review 143) — the SYNC-APPLICABLE
///   gate for the schema dimension. Its underlying record rewrite is still gated as
///   the `Db` category here, so a receiver that forbids `db` skips a migration chunk
///   too, and the `schema_write` RBAC remains the schema-authority gate on top.
///
/// M0b chunk sync carries only [`Record`](forge_sync::SyncResource::Record) ops, so
/// every chunk maps to `Db` today; the `match` is written over the resource so a
/// future non-record chunk category routes to its own gate rather than silently
/// reusing `Db`.
fn sync_op_policy_category(envelope: &forge_sync::SyncOpEnvelope) -> crate::Capability {
    match envelope.resource_type {
        // Record writes (and migration chunks, whose schema authority is the
        // membership `schema_write` RBAC, not a workspace-policy category) gate as
        // the `Db` capability — the workspace database the op touches.
        forge_sync::SyncResource::Record => crate::Capability::Db,
    }
}

/// Build the durable [`forge_storage::AuditRecord`] for one resolved sync-RBAC
/// decision (SC-12). The `metadata` mirrors the `audit-log-e2e`
/// `sync_remote_denial_persisted_query_decision` shape: the originating `source`
/// peer, the TRUSTED role + grants that decided the op (never the untrusted
/// incoming claim), and the concrete touched `record_ids` — no secret value or
/// request/response body is present, so redaction is a no-op here.
fn sync_audit_record(
    logical_time: u64,
    source: &str,
    decision: &SyncAuthDecision,
    env: &RemoteOpEnvelope,
) -> forge_storage::AuditRecord {
    let audit = decision.audit();
    let metadata = serde_json::json!({
        "source": source,
        "trusted_role": format!("{:?}", audit.trusted_role),
        "trusted_db_read": audit.trusted_db_read,
        "trusted_db_write": audit.trusted_db_write,
        "trusted_schema_write": audit.trusted_schema_write,
        "record_ids": env.record_ids,
    });
    forge_storage::AuditRecord::new(
        logical_time,
        SYNC_RBAC_PRODUCER,
        audit.action.clone(),
        audit.decision,
        audit.actor_id.clone(),
        "record",
        audit.resource_id.clone(),
        audit.collection.clone(),
        audit.reason.clone(),
        metadata,
    )
}

/// Translate the [`forge_sync`] op envelope (recovered at the apply boundary from
/// the origin's oplog + the chunk's `doc_id`) into the pure-decision
/// [`RemoteOpEnvelope`] the authorizer consumes. M0b chunk sync carries record ops
/// AND DL-13 migration chunks; the FULL list of touched record ids is threaded
/// through so the envelope-metadata gate (`forge/spec/sync-rbac.md` line 90) sees a
/// concrete record identity and the collection grant gates the chunk as a whole. A
/// migration chunk additionally carries its target `schema_version`, which is
/// threaded through (NOT dropped — review 143) and flips `is_migration` so the
/// authorizer requires schema-change authority on top of the record-write grant.
///
/// `record_ids` is the WHOLE recovered list, trimmed and with any blank entry
/// dropped (`review 093`): a single-record op carries exactly one; a multi-record
/// transact group legitimately carries several, and ALL of them are surfaced — the
/// collection grant gates the op as a whole, not a single representative record.
/// The list is empty ONLY when the chunk names NO record at all (an unknown-op /
/// record-less chunk), which the authorizer's `envelope_defect` denies fail-closed
/// before any grant check (`review 092 #2`): a record write must carry a concrete
/// record identity, never be coerced to an `Insert` with no id. An earlier
/// translation dropped a multi-record group to a single id (or `None`), which made
/// the envelope-metadata gate reason about only one of several touched records;
/// threading the full list closes that gap while keeping the record-less chunk
/// fail-closed.
fn remote_op_envelope_from_sync(env: &forge_sync::SyncOpEnvelope) -> RemoteOpEnvelope {
    let op = match env.op {
        forge_sync::SyncRecordOp::Insert => RemoteOp::Insert,
        forge_sync::SyncRecordOp::Patch => RemoteOp::Patch,
        forge_sync::SyncRecordOp::Delete => RemoteOp::Delete,
        // A transact group / foreign re-import is still a record write; gate it as
        // an insert (the most permissive write op shares the same role + grant
        // checks, so the collection-level decision is identical).
        forge_sync::SyncRecordOp::Write => RemoteOp::Insert,
    };
    let resource_type = match env.resource_type {
        forge_sync::SyncResource::Record => ResourceType::Record,
    };
    RemoteOpEnvelope {
        resource_type,
        op,
        collection: Some(env.collection.clone()),
        // Thread the WHOLE touched-record list (trimmed, blank-free) so the gate
        // gates the op as a whole. A single-record op carries one id; a
        // multi-record transact group carries several. An empty list (a chunk that
        // names NO record) leaves the gate to deny it fail-closed instead of
        // importing a record write with no record identity (`review 092 #2`,
        // `review 093`).
        record_ids: env
            .record_ids
            .iter()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect(),
        schema_id: None,
        // Carry the migration's target `schema_version` THROUGH the envelope instead
        // of dropping it (review 143): a chunk that carries `schema_version: Some(_)`
        // is a DL-13 migration, and the authorizer gates it as a SCHEMA CHANGE — it
        // needs the collection `db.write` grant AND schema-change authority
        // (Owner/Maintainer + `schema_write`), not a plain record write. An ordinary
        // record-write chunk carries `None` and is gated by `db.write` alone.
        schema_version: env.schema_version,
        is_migration: env.schema_version.is_some(),
    }
}

/// Record one SS-7 authorization decision (allow or deny) on the receiver's event
/// sink as an audit row (`forge/spec/sync-rbac.md`: actor id, op, resource,
/// collection, trusted role + grants, reason). Denials are emitted as
/// `sync.permission_denied`; allows as `sync.authorized` so the audit trail is
/// complete (SC-12). `source` is the authenticated origin peer id.
///
/// Returns the `logical_time` the EventSink minted for this event, so the caller
/// can stamp the SAME deterministic clock on the durable `audit_log` row — the
/// transient event and the persisted row are one decision under one clock.
fn emit_sync_audit(events: &mut EventSink, source: &str, decision: &SyncAuthDecision) -> u64 {
    let audit = decision.audit();
    let kind = if decision.is_allow() {
        "sync.authorized"
    } else {
        "sync.permission_denied"
    };
    emit_event_logical_time(
        events,
        kind,
        serde_json::json!({
            "action": audit.action,
            "decision": audit.decision,
            "source": source,
            "actor_id": audit.actor_id,
            "collection": audit.collection,
            "trusted_role": format!("{:?}", audit.trusted_role),
            "trusted_db_write": audit.trusted_db_write,
            "reason": audit.reason,
        }),
    )
}

/// Emit a workspace event and return the `logical_time` (the EventSink's minted
/// [`LogicalTimestamp`] value) it carries. The audit-log producers stamp this same
/// value on their durable row so the transient event and the persisted audit row
/// share one deterministic clock — no wall clock on the replayable path (SC-12).
fn emit_event_logical_time(
    events: &mut EventSink,
    kind: &str,
    payload: serde_json::Value,
) -> u64 {
    let id = events.emit(None, kind, payload);
    events
        .events()
        .iter()
        .rev()
        .find(|e| e.event_id == id)
        .map(|e| e.created_at_logical.0)
        .unwrap_or(0)
}

/// Load the persisted SS-7 sync membership table from the workspace file (mirrors
/// [`load_db_read_grants`]). Absent / empty → an empty table; with no row a peer is
/// unknown and the apply gate denies it (fail-closed).
fn load_sync_membership(
    store: &Store,
) -> Result<std::collections::BTreeMap<String, TrustedMembership>> {
    match store.kv_get(META_NS, SYNC_MEMBERSHIP_KEY)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::StorageError(format!("deserialize sync membership: {e}"))
        }),
        None => Ok(std::collections::BTreeMap::new()),
    }
}

/// Load the persisted trusted SC-10 run policy from the workspace file (T037).
/// Absent → `None` (un-provisioned): the run installs the permissive `AllowAll`
/// context, preserving the M0a spine baseline. A present policy is materialized
/// into a real `ComposedDecisionContext` at run time. Mirrors
/// [`load_sync_membership`] / [`load_db_read_grants`].
fn load_run_policy(store: &Store) -> Result<Option<RunPolicy>> {
    match store.kv_get(META_NS, RUN_POLICY_KEY)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| CoreError::StorageError(format!("deserialize run policy: {e}"))),
        None => Ok(None),
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

/// Load the persisted trusted `db.write` grant table from the workspace file (the
/// write counterpart of [`load_db_read_grants`]). Absent / empty → an empty table
/// (no configured scopes), which preserves the owner-permits-all M0a default for
/// actors with no grant entry.
fn load_db_write_grants(
    store: &Store,
) -> Result<std::collections::BTreeMap<String, Vec<String>>> {
    match store.kv_get(META_NS, DB_WRITE_GRANTS_KEY)? {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
            CoreError::StorageError(format!("deserialize db.write grants: {e}"))
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

#[cfg(test)]
mod remote_envelope_tests {
    //! The wired-path translation `remote_op_envelope_from_sync` must produce a
    //! pure-decision [`RemoteOpEnvelope`] that the authorizer's envelope-metadata
    //! gate (`forge/spec/sync-rbac.md` line 90, `review 092 #2`/`review 093`) can
    //! accept for a well-formed record op AND deny fail-closed for a record-less
    //! one. The translation threads the WHOLE touched-record list: before the fix it
    //! dropped a MULTI-record transact group to a single id (or `None`), so the gate
    //! reasoned about only one of several touched records. These tests pin the
    //! translation directly (it is private), and cross-check the end-to-end decision
    //! through `authorize_remote_op`.

    use super::*;
    use forge_domain::Role;
    use forge_sync::{SyncOpEnvelope, SyncRecordOp, SyncResource};

    fn sync_env(op: SyncRecordOp, collection: &str, record_ids: &[&str]) -> SyncOpEnvelope {
        SyncOpEnvelope {
            resource_type: SyncResource::Record,
            op,
            collection: collection.to_string(),
            record_ids: record_ids.iter().map(|s| s.to_string()).collect(),
            origin_source: None,
            malformed: None,
            schema_version: None,
            registry_collection: None,
        }
    }

    fn owner() -> TrustedMembership {
        TrustedMembership {
            actor_id: "actor-owner".into(),
            role: Role::Owner,
            db_read: vec!["*".into()],
            db_write: vec!["*".into()],
            schema_write: true,
        }
    }

    #[test]
    fn single_record_op_threads_the_named_record_id() {
        let env = remote_op_envelope_from_sync(&sync_env(SyncRecordOp::Insert, "tasks", &["t1"]));
        assert_eq!(env.collection.as_deref(), Some("tasks"));
        assert_eq!(env.record_ids, vec!["t1".to_string()]);
        assert_eq!(env.op, RemoteOp::Insert);
        assert!(!env.is_migration, "an ordinary record write is not a migration");
        assert_eq!(env.schema_version, None);
        // A trusted owner applies it (the gate sees a concrete record id).
        assert!(authorize_remote_op(&owner(), None, &env).is_allow());
    }

    #[test]
    fn migration_chunk_translates_to_a_schema_affecting_envelope() {
        // review 143: a sync chunk that carries a migration `schema_version` is a
        // SCHEMA-AFFECTING op. The translation threads the version through (NOT dropped)
        // and flips `is_migration`, so the authorizer gates it as a schema change — an
        // editor with only db.write is DENIED, an owner with schema_write applies it.
        let mut sync = sync_env(SyncRecordOp::Write, "expenses", &["e1"]);
        sync.schema_version = Some(2);
        let env = remote_op_envelope_from_sync(&sync);
        assert!(env.is_migration, "a chunk carrying a schema_version is a migration");
        assert_eq!(env.schema_version, Some(2), "the migration target version is threaded, not dropped");

        // An editor with only db.write on the collection is denied (FIX-1 polarity).
        let editor = TrustedMembership {
            actor_id: "actor-editor".into(),
            role: Role::Editor,
            db_read: vec!["*".into()],
            db_write: vec!["expenses".into()],
            schema_write: false,
        };
        assert!(
            !authorize_remote_op(&editor, None, &env).is_allow(),
            "an editor with only db.write must not apply a migration chunk"
        );
        // An owner with wildcard db.write + schema_write applies it.
        assert!(authorize_remote_op(&owner(), None, &env).is_allow());
    }

    #[test]
    fn multi_record_transact_group_threads_full_list_and_is_authorized() {
        // review 093 regression: a transact group names SEVERAL records. The whole
        // list must be threaded (not collapsed to one id / `None`), so the gate
        // gates the op as a whole and a legitimate group is allowed for a trusted
        // owner instead of being denied as "missing record id".
        let env =
            remote_op_envelope_from_sync(&sync_env(SyncRecordOp::Write, "tasks", &["t1", "t2"]));
        assert_eq!(
            env.record_ids,
            vec!["t1".to_string(), "t2".to_string()],
            "the full touched-record list is threaded, not just the first id"
        );
        let decision = authorize_remote_op(&owner(), None, &env);
        assert!(
            decision.is_allow(),
            "a multi-record transact group must NOT be denied as missing a record id: {decision:?}"
        );
    }

    #[test]
    fn record_less_chunk_has_empty_record_ids_and_is_denied_fail_closed() {
        // A chunk that names NO record (empty list — an unknown-op chunk) yields an
        // empty `record_ids` so the envelope-metadata gate denies it fail-closed
        // before any grant check, even for a trusted owner (`review 092 #2`).
        let env = remote_op_envelope_from_sync(&sync_env(SyncRecordOp::Write, "tasks", &[]));
        assert!(env.record_ids.is_empty(), "a record-less chunk names no record");
        let decision = authorize_remote_op(&owner(), None, &env);
        assert!(!decision.is_allow(), "a record write with no record id is denied");
        assert!(
            decision.reason().contains("missing record id"),
            "the denial names the missing record id: {}",
            decision.reason()
        );
    }

    #[test]
    fn blank_record_ids_are_filtered_from_the_list() {
        // A stray empty/whitespace id in the list must be DROPPED — the gate then
        // sees only the concrete ids and still allows a group that names a real
        // record (`review 093`).
        let env =
            remote_op_envelope_from_sync(&sync_env(SyncRecordOp::Write, "tasks", &["", "  ", "t9"]));
        assert_eq!(env.record_ids, vec!["t9".to_string()], "blank entries are filtered out");
        assert!(authorize_remote_op(&owner(), None, &env).is_allow());
    }

    #[test]
    fn all_blank_record_ids_collapse_to_empty_and_deny() {
        // A list of only blank ids names no concrete record — they all filter out to
        // an empty list, so the gate fails closed.
        let env =
            remote_op_envelope_from_sync(&sync_env(SyncRecordOp::Patch, "tasks", &["", "   "]));
        assert!(env.record_ids.is_empty());
        assert!(!authorize_remote_op(&owner(), None, &env).is_allow());
    }
}
