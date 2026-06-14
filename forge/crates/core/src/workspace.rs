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
const SCHEMA_REGISTRY_KEY: &str = "schema_registry";

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
        let sync_membership = load_sync_membership(&store)?;
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
            sync_membership,
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

        let WorkspaceCore {
            store: self_store,
            indexes: self_indexes,
            events: self_events,
            sync_membership: self_membership,
            ..
        } = self;
        let WorkspaceCore {
            store: other_store,
            indexes: other_indexes,
            events: other_events,
            sync_membership: other_membership,
            ..
        } = other;

        forge_sync::sync_stores_authorized(
            self_store,
            self_indexes,
            other_store,
            other_indexes,
            |source, envelope| {
                // `source` is the RELAY peer the chunk arrived from; it selects the
                // RECEIVER (the other side) for the direction. But the ACTOR whose
                // trusted membership decides authorization is the chunk's ORIGINAL
                // author: a chunk `source` merely forwarded (its `origin_source` is
                // set from the remote-import provenance) must be gated against that
                // original author, not the relay (`review 092 #1` / SS-7 actor
                // identity). A locally-authored chunk has no `origin_source`, so the
                // relay IS the author and `actor == source`.
                let actor = envelope.origin_source.as_deref().unwrap_or(source);
                if source == self_source {
                    // Direction: self → received by `other`. Authorize against
                    // `other`'s table for the original author.
                    authorize_incoming_op(other_membership, other_events, actor, envelope)
                } else {
                    // Direction: other → received by `self`.
                    debug_assert_eq!(source, other_source);
                    authorize_incoming_op(self_membership, self_events, actor, envelope)
                }
            },
        )
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
        let result = authorize(&cmd).and_then(|()| command_registry().dispatch(self, &cmd));
        match result {
            Ok(payload) => CoreResponse::ok(request_id, payload),
            Err(error) => CoreResponse::err(request_id, error),
        }
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
fn authorize_incoming_op(
    membership: &std::collections::BTreeMap<String, TrustedMembership>,
    events: &mut EventSink,
    source: &str,
    envelope: &forge_sync::SyncOpEnvelope,
) -> bool {
    // A chunk whose doc id is not a valid `collection/<name>` records doc (or whose
    // staged envelope is otherwise unfit) is denied fail-closed BEFORE membership
    // resolution: the apply path must reject a malformed chunk rather than guess a
    // collection (`review 092 #2` / `forge/spec/sync-rbac.md` line 52). Surface a
    // permission_denied audit naming the staging defect so the skip is observable.
    if let Some(reason) = &envelope.malformed {
        events.emit(
            None,
            "sync.permission_denied",
            serde_json::json!({
                "decision": "deny",
                "source": source,
                "collection": envelope.collection,
                "reason": reason,
            }),
        );
        return false;
    }
    let env = remote_op_envelope_from_sync(envelope);
    match membership.get(source) {
        Some(trusted) => {
            // The trusted row is authoritative. In-process M0b carries no separate
            // session claim, so `claim = None` (a claim could only narrow, never
            // widen, the decision — `forge/spec/sync-rbac.md`).
            let decision = authorize_remote_op(trusted, None, &env);
            emit_sync_audit(events, source, &decision);
            decision.is_allow()
        }
        None => {
            // Unknown peer: fail closed. Surface a permission_denied audit naming
            // the missing trust so the skip is observable.
            events.emit(
                None,
                "sync.permission_denied",
                serde_json::json!({
                    "decision": "deny",
                    "source": source,
                    "collection": envelope.collection,
                    "reason": "no trusted membership for sync peer",
                }),
            );
            false
        }
    }
}

/// Translate the [`forge_sync`] op envelope (recovered at the apply boundary from
/// the origin's oplog + the chunk's `doc_id`) into the pure-decision
/// [`RemoteOpEnvelope`] the authorizer consumes. M0b chunk sync carries only
/// record ops; the FULL list of touched record ids is threaded through so the
/// envelope-metadata gate (`forge/spec/sync-rbac.md` line 90) sees a concrete
/// record identity and the collection grant gates the chunk as a whole.
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
        schema_version: None,
    }
}

/// Record one SS-7 authorization decision (allow or deny) on the receiver's event
/// sink as an audit row (`forge/spec/sync-rbac.md`: actor id, op, resource,
/// collection, trusted role + grants, reason). Denials are emitted as
/// `sync.permission_denied`; allows as `sync.authorized` so the audit trail is
/// complete (SC-12). `source` is the authenticated origin peer id.
fn emit_sync_audit(events: &mut EventSink, source: &str, decision: &SyncAuthDecision) {
    let audit = decision.audit();
    let kind = if decision.is_allow() {
        "sync.authorized"
    } else {
        "sync.permission_denied"
    };
    events.emit(
        None,
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
    );
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
        // A trusted owner applies it (the gate sees a concrete record id).
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
