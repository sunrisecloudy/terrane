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
use auth::*;
use crate::bridge::StorageHostBridge;
use crate::determinism::*;
use crate::event::EventSink;
use crate::sync_rbac::{
    authorize_remote_op, RemoteOp, RemoteOpEnvelope, ResourceType, SyncAuthDecision,
    TrustedMembership,
};
use forge_domain::{
    AppletId, CoreCommand, CoreError, CoreResponse, Manifest, Result, RunRecord,
};
use forge_runtime::{
    record_dispatch, record_run, replay, replay_dispatch, NullBridge, Program as RuntimeProgram,
};
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

/// KV key prefix (within [`META_NS`]) for an applet's **last-known UI tree** — the
/// most recent tree the applet rendered through this facade (`runtime.run`'s last
/// `ui.render`, then each accepted `ui.dispatch_event`). This is the DIFF BASE for
/// the next event: `ui.dispatch_event` re-enters the applet's handler in a fresh
/// one-shot realm, captures the handler's new tree, and diffs it against THIS
/// stored tree to produce the next UI patch (UI-4/CR-6). Keyed per applet so two
/// applets' interactive sessions never share a diff base; persisted so the loop
/// survives reopening the workspace. The full key is `ui_tree/<applet_id>`.
const UI_TREE_KEY_PREFIX: &str = "ui_tree/";

/// KV key prefix (within [`META_NS`]) for an applet's **dispatch lifecycle** — the
/// receiver-side flag that decides whether an applet may be re-entered by a UI
/// event. An applet is `active` by default; a workspace can SUSPEND it through the
/// trusted [`set_applet_lifecycle`](WorkspaceCore::set_applet_lifecycle) seam, after
/// which `ui.dispatch_event` rejects every event with a typed `ui.applet_not
/// _dispatchable` error BEFORE any handler runs and with no state change (the T034
/// `suspended_applet_rejected` vector). Set only through the trusted seam (never a
/// request payload), mirroring the `db.read` grant table; persisted so a suspended
/// applet stays suspended after reopen. The full key is `lifecycle/<applet_id>`.
const APPLET_LIFECYCLE_KEY_PREFIX: &str = "lifecycle/";

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
        Ok(WorkspaceCore {
            store,
            registry,
            indexes,
            events: EventSink::new(),
            workspace_id: workspace_id.into(),
            db_read_grants,
            sync_membership,
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
        let key = applet_lifecycle_key(applet_id.as_ref());
        let bytes = serde_json::to_vec(&lifecycle)
            .map_err(|e| CoreError::StorageError(format!("serialize applet lifecycle: {e}")))?;
        self.store.kv_set(META_NS, &key, &bytes, "application/json")
    }

    /// An applet's dispatch lifecycle, defaulting to [`AppletLifecycle::Active`] for
    /// an applet that was never explicitly suspended. Read-only access for tests /
    /// the `ui.dispatch_event` gate.
    pub fn applet_lifecycle(&self, applet_id: &str) -> Result<AppletLifecycle> {
        match self.store.kv_get(META_NS, &applet_lifecycle_key(applet_id))? {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                CoreError::StorageError(format!("deserialize applet lifecycle: {e}"))
            }),
            None => Ok(AppletLifecycle::Active),
        }
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
        let result = authorize(&cmd).and_then(|()| match cmd.name.as_str() {
            "workspace.create" => self.cmd_workspace_create(&cmd),
            "workspace.open" => self.cmd_workspace_open(&cmd),
            "applet.install" => self.cmd_applet_install(&cmd),
            "runtime.run" => self.cmd_runtime_run(&cmd),
            "runtime.replay" => self.cmd_runtime_replay(&cmd),
            "runtime.replay_session" => self.cmd_runtime_replay_session(&cmd),
            "ui.dispatch_event" => self.cmd_ui_dispatch_event(&cmd),
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
        // 080 #1). The signed package's MANIFEST/policy is also bound to the
        // top-level `manifest` that is stored and enforced (review 082 #1 / 083):
        // a signed install must enforce the SIGNED capability boundary — the same
        // app id, every resource limit, the full net rule, and the entrypoint —
        // not a broader one. `Unsigned` when the install carries no signature.
        let trust = verify_install_signature(cmd, &applet_id, &manifest, sources)?;

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
        // Build this run's `ctx.files` sandbox filesystem from the injected factory
        // (CR-3 / spec/files.md). It carries the TRUSTED handle → per-applet-root
        // resolution; the runtime resolves a granted handle to its sandbox root and
        // performs a capability-checked, confined read/write at the host edge.
        // Default = empty (no granted root → every ctx.files op fails closed).
        let file_system = (self.file_system_factory)();

        // Run in record mode against the live Store-backed bridge. The bridge
        // performs the SQLite writes / UI diffs; the runtime's HostContext gates
        // each ctx.* call against the manifest policy BEFORE the bridge runs —
        // including the SC-5 net egress check (so a denied ctx.net.fetch never
        // reaches the injected client) and the CR-3 files capability + sandbox
        // confinement (so a denied/escaping ctx.files op never reaches the
        // injected filesystem). The files grant the runtime gates against is read
        // from the TRUSTED manifest snapshot, not the request payload.
        let mut bridge = StorageHostBridge::with_http_client(
            &mut self.store,
            applet_id.as_str(),
            http_client,
        )
        .with_secret_store(secret_store)
        .with_file_system(file_system);
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

        // Persist the run's LAST rendered tree as the applet's last-known tree — the
        // diff base for the next UI event (UI-4/CR-6). `runtime.run`'s `main` is the
        // initial render of the interactive session; a subsequent `ui.dispatch_event`
        // diffs its handler's new tree against this one. Only on a completed run with
        // at least one render (a failed/no-render run leaves the prior base intact).
        if ok {
            if let Some(last) = ui_renders.last() {
                self.store_ui_tree(applet_id.as_str(), &last.tree)?;
            }
        }
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

        let (original, replayed) = self.replay_run_by_id(&run_id, &cmd.actor)?;

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

    /// Load the stored [`RunRecord`] for `run_id`, reconstruct the version-pinned
    /// program + manifest this execution used, replay it deterministically over a
    /// [`NullBridge`], and assert the replay is byte-identical to the original
    /// (CR-9). Returns `(original, replayed)` so a caller can fingerprint either.
    /// Shared by [`cmd_runtime_replay`](Self::cmd_runtime_replay) and the
    /// session-replay path so the two never drift in how a single run is replayed.
    ///
    /// Version-pinned replay (review 031 finding 3, review 036 finding 2):
    /// reconstruct the program + manifest from the artifact recorded for THIS
    /// execution, not the currently installed applet. Resolution order:
    ///   1. the PER-RUN pin (`program/run/<run_id>`) — unique to this run, so a
    ///      reinstall under a different manifest cannot overwrite or alter it
    ///      (the review 036 finding 2 case);
    ///   2. the content-addressed `program/<code_hash>` pin — covers runs
    ///      recorded before per-run pinning existed;
    ///   3. the currently installed applet — last-resort legacy fallback, and
    ///      only if its code_hash still matches the recorded one.
    ///
    /// A run recorded by `ui.dispatch_event` carries a `ui.dispatch_event` envelope
    /// and was driven by re-entering a named handler (UI-4/CR-6), not `main` — so it
    /// is replayed via the dispatch path, which recovers the recorded `(action_ref,
    /// payload)` and re-runs that handler. A normal `runtime.run` record has no such
    /// envelope and replays via `main`.
    fn replay_run_by_id(
        &self,
        run_id: &str,
        actor: &forge_domain::ActorContext,
    ) -> Result<(RunRecord, RunRecord)> {
        let original = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| CoreError::ValidationError(format!("run {run_id} not found")))?;

        let installed = match self.load_run_program(run_id)? {
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

        let program = RuntimeProgram::new(original.applet_id.clone(), installed.js_code.clone());
        let mut null = NullBridge::new();
        let is_dispatch = original.calls.iter().any(|c| c.method == "ui.dispatch_event");
        let replayed = if is_dispatch {
            replay_dispatch(&original, &program, &installed.manifest, actor, &mut null)?
        } else {
            replay(&original, &program, &installed.manifest, actor, &mut null)?
        };

        // The strict replay check: canonical provenance on both records AND
        // byte-identical traces, surfaced as a RuntimeError on divergence.
        original.assert_replay_of(&replayed)?;
        Ok((original, replayed))
    }

    /// `runtime.replay_session` — replay an ordered **event session** (an initial
    /// `runtime.run` record followed by N `ui.dispatch_event` records, in dispatch
    /// order) and prove the WHOLE sequence replays byte-identically (prd-merged/05
    /// UI-4, prd-merged/01 CR-6, CR-8). This is the session-level analogue of
    /// `runtime.replay`: where `runtime.replay` blesses ONE recorded run, this blesses
    /// a recorded interactive session as a unit, so a multi-event session round-trips.
    ///
    /// Payload: `{ run_ids: [ <initial run_id>, <event run_id>, ... ] }` — the
    /// session in dispatch order (the ids the initial `runtime.run` + each accepted
    /// `ui.dispatch_event` returned).
    ///
    /// For each id we replay the run via [`replay_run_by_id`](Self::replay_run_by_id)
    /// (which version-pins the program/manifest and asserts that single run is
    /// byte-identical), then we ALSO:
    ///   - re-derive each event's UI patch by diffing the replayed run's final tree
    ///     against the PRIOR run's final tree, exactly as the live `ui.dispatch_event`
    ///     loop did, and assert that re-derived patch equals the originally recorded
    ///     one (so every patch is byte-identical, not just the host-call trace);
    ///   - fold each record's per-run fingerprint into a composite
    ///     [`session_fingerprint`](RunRecord::session_fingerprint), and assert the
    ///     replayed session's composite equals the original's — which is sensitive to
    ///     BOTH per-run divergence AND event ORDER (two events applied in a different
    ///     order produce a different composite, so order is enforced).
    ///
    /// Divergence anywhere — a single run, a patch, or the composite — is a typed
    /// `RuntimeError`/`ValidationError`, never a panic. The recorded permission
    /// snapshot governs each replay (CR-9); the live bridge is never consulted.
    fn cmd_runtime_replay_session(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let run_ids: Vec<String> = match cmd.payload.get("run_ids") {
            Some(serde_json::Value::Array(arr)) => arr
                .iter()
                .map(|v| {
                    v.as_str().map(str::to_string).ok_or_else(|| {
                        CoreError::ValidationError(
                            "runtime.replay_session `run_ids` entries must be strings".into(),
                        )
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            _ => {
                return Err(CoreError::ValidationError(
                    "runtime.replay_session requires a non-empty `run_ids` array".into(),
                ))
            }
        };
        if run_ids.is_empty() {
            return Err(CoreError::ValidationError(
                "runtime.replay_session `run_ids` must not be empty".into(),
            ));
        }

        // Replay each run in order, accumulating (original, replayed) pairs. We keep
        // BOTH chains so the byte-identity claim is derived AND checked here, not just
        // returned for a test to assert.
        let mut originals: Vec<RunRecord> = Vec::with_capacity(run_ids.len());
        let mut replayeds: Vec<RunRecord> = Vec::with_capacity(run_ids.len());
        let mut applet_id: Option<AppletId> = None;

        for run_id in &run_ids {
            let (original, replayed) = self.replay_run_by_id(run_id, &cmd.actor)?;
            // Every run in one session must belong to the SAME applet — a session is
            // one applet's interactive loop. A mixed-applet `run_ids` list is a
            // caller error, not a silent cross-applet replay.
            match &applet_id {
                None => applet_id = Some(replayed.applet_id.clone()),
                Some(id) if id != &replayed.applet_id => {
                    return Err(CoreError::ValidationError(format!(
                        "runtime.replay_session run {run_id} belongs to applet {} but the session started with {}",
                        replayed.applet_id, id
                    )));
                }
                Some(_) => {}
            }
            originals.push(original);
            replayeds.push(replayed);
        }

        // STRUCTURAL session-shape guard. The patch-chain walk treats `run_ids[0]` as
        // the session HEAD (the initial `runtime.run` whose render is only the diff
        // base) and `run_ids[1..]` as the dispatched EVENTS (each diffed against the
        // prior render). That contract is only meaningful for a well-formed session:
        // the head must be a non-dispatch run and every tail entry must be a
        // `ui.dispatch_event` run, with no duplicate ids. Without this guard a caller
        // could pass an arbitrary same-applet `run_ids` list (a dispatch at the head,
        // a `runtime.run` mid-session, a duplicated id) and still get a misleading
        // `replays_identically: true` with a bogus "converged final tree" — because
        // each run self-replays and the recorded/replayed walks are trivially equal.
        // Rejecting a malformed shape up front makes the convergence claim load-bearing.
        let original_refs: Vec<&RunRecord> = originals.iter().collect();
        assert_well_formed_session(&run_ids, &original_refs)?;

        // Derive the ordered per-event patch chain + final tree from BOTH the recorded
        // (`originals`) and the replayed (`replayeds`) record sequences, walking each
        // run's final render against the PRIOR run's render — the same diff base the
        // live `ui.dispatch_event` loop used (UI-4). The two walks must be byte-equal:
        // that is the real session byte-identity claim (recorded patches == replayed
        // patches == recorded final tree == replayed final tree).
        let replayed_refs: Vec<&RunRecord> = replayeds.iter().collect();
        let (recorded_patches, recorded_final) = derive_session_patch_chain(&original_refs)?;
        let (event_patches, replayed_final) = derive_session_patch_chain(&replayed_refs)?;

        // The composite session identity: fold each record's per-run fingerprint in
        // order. Equal composites ⇒ each run replayed byte-identically to its recorded
        // counterpart, in order. Divergence is a RuntimeError.
        let session_fingerprint = RunRecord::session_fingerprint(&replayed_refs);
        let runs_replay_identically =
            RunRecord::session_replays_identically(&original_refs, &replayed_refs);
        if !runs_replay_identically {
            return Err(CoreError::RuntimeError(format!(
                "session replay diverged from the recorded session ({} run(s); composite fingerprints differ)",
                run_ids.len()
            )));
        }
        // Beyond the per-run trace fingerprint, the OBSERVABLE session output — the
        // ordered patch chain and the converged final tree — must reproduce exactly.
        // This is checked server-side so a caller's `replays_identically: true` is a
        // load-bearing claim, not something only the test asserts.
        if event_patches != recorded_patches {
            return Err(CoreError::RuntimeError(format!(
                "session replay diverged: re-derived event patch chain ({} event(s)) differs from the recorded one",
                event_patches.len()
            )));
        }
        if replayed_final != recorded_final {
            return Err(CoreError::RuntimeError(
                "session replay diverged: re-derived final tree differs from the recorded one"
                    .to_string(),
            ));
        }
        let replays_identically = runs_replay_identically;

        if let Some(applet_id) = &applet_id {
            self.events.emit(
                Some(applet_id.clone()),
                "session.replayed",
                serde_json::json!({
                    "applet_id": applet_id,
                    "run_ids": run_ids,
                    "events": run_ids.len().saturating_sub(1),
                    "ok": true,
                }),
            );
        }

        Ok(serde_json::json!({
            "ok": true,
            "run_ids": run_ids,
            // The per-event re-derived UI patches (one list per dispatched event, in
            // order). Already asserted byte-equal to the recorded chain above, so a
            // caller receives the verified patch sequence.
            "event_patches": event_patches,
            // The session's final rendered tree (the last replayed render, `null` if
            // nothing rendered). Already asserted equal to the recorded final tree.
            "final_tree": replayed_final,
            "session_fingerprint": session_fingerprint,
            "replays_identically": replays_identically,
        }))
    }

    /// `ui.dispatch_event` — re-enter an installed applet's handler on a UI event
    /// and produce the next UI patch (prd-merged/05 UI-4, prd-merged/01 CR-6). This
    /// is the keystone interactive loop through the facade: a rendered control
    /// carried an `onTap`/`onChange` `ActionRef`; the renderer sends that ref back
    /// with an event payload; this command dispatches the handler exported under
    /// that name over the **same** QuickJS containment / capability gate / record
    /// path as `runtime.run`, captures the handler's new UI tree, DIFFS it against
    /// the applet's last-known tree to a patch, emits a `ui.patch` event, persists
    /// the new tree as the next diff base, saves the recorded run (with the event in
    /// its trace), and returns `{ action_ref, tree, patches }`.
    ///
    /// Payload: `{ applet_id, action_ref, event_payload? }`.
    ///
    /// Contract (T034 `forge/fixtures/ui-events`), each a typed rejection with the
    /// applet's state + last-known tree UNCHANGED:
    ///   - a **null/absent `action_ref`** (an event on a control with no handler) is
    ///     a safe no-op: `{ ignored: true, patches: [] }`, no dispatch (the
    ///     `no_handler_event_ignored` vector);
    ///   - a **suspended applet** is rejected BEFORE any handler runs with
    ///     `ValidationError("ui.applet_not_dispatchable: ... suspended")` (the
    ///     `suspended_applet_rejected` vector; the lifecycle is the trusted flag set
    ///     via [`set_applet_lifecycle`](Self::set_applet_lifecycle));
    ///   - an **unknown `action_ref`** (no such exported handler) is the engine's
    ///     typed `ValidationError` (the `unknown_action_rejected` vector);
    ///   - an **invalid payload** / a **throwing handler** surfaces the handler's
    ///     own typed error (a `ValidationError` the handler raised, or a
    ///     `RuntimeError` for an uncaught throw) — the run record captures the
    ///     failure (`handler_throws_prior_tree_intact` / `invalid_payload_rejected`).
    ///
    /// The realm is one-shot per dispatch, so a handler persists state ONLY through
    /// `ctx.db`/`ctx.storage`; the next event reads it back. Command-level RBAC
    /// (run-capable roles) gates entry; the applet's manifest capabilities gate each
    /// `ctx.*` call inside the handler, exactly as a run.
    fn cmd_ui_dispatch_event(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        // The dispatch key (T034): the `ActionRef` the rendered control carried. A
        // null/absent ref means the targeted control has NO handler for this event —
        // a safe ignored no-op, not an error (the `no_handler_event_ignored` vector).
        let action_ref = match cmd.payload.get("action_ref") {
            None | Some(serde_json::Value::Null) => {
                return Ok(serde_json::json!({
                    "applet_id": applet_id,
                    "ignored": true,
                    "reason": "target node has no ActionRef for this event",
                    "patches": [],
                }));
            }
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(other) => {
                return Err(CoreError::ValidationError(format!(
                    "ui.dispatch_event `action_ref` must be a string, got {other}"
                )))
            }
        };
        let event_payload = cmd
            .payload
            .get("event_payload")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));

        // Lifecycle gate (UI-4): a suspended applet has no live UI session, so a UI
        // event is rejected BEFORE any handler runs and with NO state change (the
        // `suspended_applet_rejected` vector). The flag is the TRUSTED per-applet
        // lifecycle, read from workspace state — never from the request — so an
        // applet cannot un-suspend itself by sending an event. We emit a
        // `ui.dispatch_rejected` event carrying the renderer-facing code
        // (`ui.applet_not_dispatchable`) with `dispatch_attempted: false`, so the
        // pre-dispatch rejection is observable to a renderer/host exactly like the
        // post-dispatch failure below (T034 `dispatch_attempted` flag).
        if self.applet_lifecycle(applet_id.as_str())? == AppletLifecycle::Suspended {
            let error = CoreError::ValidationError(format!(
                "ui.applet_not_dispatchable: applet {applet_id} is suspended; UI events are rejected before dispatch"
            ));
            self.events.emit(
                Some(applet_id.clone()),
                "ui.dispatch_rejected",
                serde_json::json!({
                    "applet_id": applet_id,
                    "action_ref": action_ref,
                    "dispatch_attempted": false,
                    "code": dispatch_error_code(&error),
                    "message": error.to_string(),
                }),
            );
            return Err(error);
        }

        let installed = self.load_applet(applet_id.as_str())?.ok_or_else(|| {
            CoreError::ValidationError(format!("applet {applet_id} is not installed"))
        })?;

        // The DIFF BASE: the applet's last-known tree (the previous render this
        // facade saw). Absent (the applet has not rendered yet) ⇒ `None`, so the
        // first event's diff is a single root replace, exactly like `runtime.run`'s
        // first render (UI-1).
        let prev_tree = self.load_ui_tree(applet_id.as_str())?;

        self.events.emit(
            Some(applet_id.clone()),
            "ui.dispatch_started",
            serde_json::json!({
                "applet_id": applet_id,
                "action_ref": action_ref,
                "code_hash": installed.code_hash,
            }),
        );

        let program = RuntimeProgram::new(applet_id.clone(), installed.js_code.clone());
        if program.code_hash() != installed.code_hash {
            return Err(CoreError::RuntimeError(format!(
                "code_hash provenance broken: runtime {} != pipeline {}",
                program.code_hash(),
                installed.code_hash
            )));
        }

        // Deterministic seams derived from `(code_hash, action_ref+payload)` so a
        // re-dispatch of the SAME event reproduces the SAME seeded time/random
        // values — the event replays byte-identically (the `replay_determinism_
        // same_sequence` vector). Mint a unique per-execution run id like a run.
        let dispatch_input = serde_json::json!([action_ref, event_payload]);
        let (random_seed, time_start) = derive_seeds(&installed.code_hash, &dispatch_input);
        let invocation = self.next_run_counter()?;

        let http_client = (self.http_client_factory)();
        let secret_store = (self.secret_store_factory)();

        // Re-enter the handler over the SAME engine path as a run: record mode,
        // live Store-backed bridge, manifest-gated `ctx.*`. `record_dispatch` runs
        // the handler named `action_ref` and records the `ui.dispatch_event`
        // envelope so the event is part of the replayable trace (T034 "events ARE
        // recorded in the run record").
        let mut bridge = StorageHostBridge::with_http_client(
            &mut self.store,
            applet_id.as_str(),
            http_client,
        )
        .with_secret_store(secret_store);
        let mut run = record_dispatch(
            &program,
            &installed.manifest,
            &cmd.actor,
            &action_ref,
            &event_payload,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // The handler's final rendered tree (its last `ui.render`), if any. Drain
        // before dropping the bridge so the `&mut Store` borrow is released.
        let final_render = bridge.ui_renders.last().map(|r| r.tree.clone());
        drop(bridge);

        run.run_id = unique_run_id(&run.code_hash, invocation);

        // A failed dispatch (unknown handler → ValidationError, a handler throw →
        // RuntimeError, an invalid payload the handler rejected) is a typed
        // rejection: persist the failed record (so the denial/throw is auditable +
        // replayable), emit a failure event, and surface the handler's error. The
        // last-known tree is NOT advanced — the applet's prior view stays the diff
        // base (the error vectors' "tree_unchanged").
        if let forge_domain::RunOutcome::Failed { error } = &run.outcome {
            let error = error.clone();
            self.store_run_program(run.run_id.as_str(), &installed)?;
            self.store_program(&installed)?;
            self.store.save_run(&run)?;
            // The event carries BOTH the typed `CoreError` (for transport/audit)
            // AND the renderer-facing T034 code (`ui.action_not_found` for an
            // unknown handler, `runtime.handler_error` for a handler throw), so a
            // host/renderer can react to the stable code without parsing the
            // English error text. `dispatch_attempted: true` — the handler ran (or
            // we tried to resolve it), unlike the pre-dispatch suspended rejection.
            self.events.emit(
                Some(applet_id.clone()),
                "ui.dispatch_failed",
                serde_json::json!({
                    "applet_id": applet_id,
                    "action_ref": action_ref,
                    "run_id": run.run_id,
                    "dispatch_attempted": true,
                    "code": dispatch_error_code(&error),
                    "message": error.to_string(),
                    "error": error,
                }),
            );
            return Err(error);
        }

        // The handler completed. Its new tree is the last render; if the handler
        // rendered nothing, the view is unchanged (an empty patch over the prior
        // tree). Diff the new tree against the last-known tree to the next patch.
        let new_tree = match &final_render {
            Some(tree) => forge_ui::from_str(&tree.to_string())?,
            None => match &prev_tree {
                Some(prev) => prev.clone(),
                None => {
                    // No prior tree and no render: nothing to diff against. This is a
                    // degenerate dispatch (a handler that neither renders nor had a
                    // prior view); treat it as an empty-patch no-op over an empty base.
                    self.store_run_program(run.run_id.as_str(), &installed)?;
                    self.store_program(&installed)?;
                    self.store.save_run(&run)?;
                    return Ok(serde_json::json!({
                        "applet_id": applet_id,
                        "action_ref": action_ref,
                        "run_id": run.run_id,
                        "tree": serde_json::Value::Null,
                        "patches": [],
                    }));
                }
            },
        };
        let patches = forge_ui::diff(prev_tree.as_ref(), &new_tree);
        let patches_json = serde_json::to_value(&patches).map_err(|e| {
            CoreError::ValidationError(format!("ui.dispatch_event patch serialize failed: {e}"))
        })?;
        let tree_json = serde_json::to_value(&new_tree).map_err(|e| {
            CoreError::ValidationError(format!("ui.dispatch_event tree serialize failed: {e}"))
        })?;

        // Persist the new tree as the next diff base BEFORE returning, so the next
        // event in the session diffs against this one (the loop's state link).
        self.store_ui_tree(applet_id.as_str(), &tree_json)?;

        // Pin the per-run replay artifact + persist the recorded run (event in the
        // trace) so the dispatch replays byte-identically, exactly like a run.
        self.store_run_program(run.run_id.as_str(), &installed)?;
        self.store_program(&installed)?;
        self.store.save_run(&run)?;

        // Emit the UI patch event — the link the renderer consumes to advance the
        // live tree (UI-1/UI-4).
        self.events.emit(
            Some(applet_id.clone()),
            "ui.patch",
            serde_json::json!({
                "applet_id": applet_id,
                "action_ref": action_ref,
                "run_id": run.run_id,
                "tree": tree_json,
                "patches": patches_json,
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "action_ref": action_ref,
            "run_id": run.run_id,
            "tree": tree_json,
            "patches": patches_json,
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

    /// Persist `tree` as the applet's last-known UI tree (the diff base for the
    /// next UI event), keyed by applet id within [`META_NS`]. Written after every
    /// accepted render through this facade — a `runtime.run`'s last render and each
    /// accepted `ui.dispatch_event` — so the interactive loop's diff base survives
    /// reopening the workspace (UI-4/CR-6).
    fn store_ui_tree(&mut self, applet_id: &str, tree: &serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(tree)
            .map_err(|e| CoreError::StorageError(format!("ui tree serialize failed: {e}")))?;
        self.store
            .kv_set(META_NS, &ui_tree_key(applet_id), &bytes, "application/json")
    }

    /// Load the applet's last-known UI tree (the diff base) as a [`forge_ui::Node`],
    /// if one was recorded. `None` ⇒ the applet has not rendered through this facade
    /// yet, so the next render's diff is a single root replace (UI-1).
    fn load_ui_tree(&self, applet_id: &str) -> Result<Option<forge_ui::Node>> {
        match self.store.kv_get(META_NS, &ui_tree_key(applet_id))? {
            Some(bytes) => {
                let node = forge_ui::from_str(std::str::from_utf8(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("ui tree is not utf-8: {e}"))
                })?)?;
                Ok(Some(node))
            }
            None => Ok(None),
        }
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

/// KV key for an applet's last-known UI tree (the interactive diff base) within
/// [`META_NS`]. See [`UI_TREE_KEY_PREFIX`].
fn ui_tree_key(applet_id: &str) -> String {
    format!("{UI_TREE_KEY_PREFIX}{applet_id}")
}

/// KV key for an applet's dispatch lifecycle flag within [`META_NS`]. See
/// [`APPLET_LIFECYCLE_KEY_PREFIX`].
fn applet_lifecycle_key(applet_id: &str) -> String {
    format!("{APPLET_LIFECYCLE_KEY_PREFIX}{applet_id}")
}

/// Classify a `ui.dispatch_event` rejection into its **renderer-facing error
/// code** (the T034 `expect.results[i].error.code` space, `forge/fixtures/ui-
/// events`). The typed [`CoreError`] is the transport/RBAC error; this is the
/// stable, renderer-visible code a host surfaces to the UI so it can show the
/// right affordance without parsing English error text:
///
///   - `ui.applet_not_dispatchable` — the applet is suspended; the event was
///     rejected BEFORE any handler ran (the `suspended_applet_rejected` vector).
///     Marked by the `ui.applet_not_dispatchable:` prefix the lifecycle gate
///     writes.
///   - `ui.action_not_found` — no handler is exported under the dispatched
///     `ActionRef` (the `unknown_action_rejected` vector). The engine raises a
///     `ValidationError` whose message is `no UI handler registered for action
///     ref …` (engine.rs `Entry::resolve`); we key off that exact marker.
///   - `ui.invalid_event_payload` — the handler ran but rejected the event PAYLOAD
///     as malformed (the `invalid_payload_rejected` vector — a TextField `onChange`
///     whose `value` was not a string). A handler signals this by throwing an
///     `Error` whose message starts with the `invalid event payload` marker; the
///     engine surfaces every JS throw as a `RuntimeError`, so we key off that
///     marker to refine an otherwise-generic handler throw into the contract's
///     dedicated payload-validation code. This lets a renderer distinguish "your
///     input was bad" (re-prompt the field) from a general "the handler crashed".
///   - `runtime.handler_error` — the handler ran and threw for any OTHER reason
///     (the `handler_throws_prior_tree_intact` vector). Every uncaught JS throw is
///     a `RuntimeError` (engine.rs `classify_failure`); the handler's own message
///     (e.g. `boom`) rides along in `message`.
///
/// Anything else (a `PermissionDenied`/`ResourceLimitExceeded`/etc. — e.g. a
/// `ctx.*` call the manifest did not grant) keeps the typed error's own
/// [`code`](CoreError::code) so a capability/limit failure is never mislabeled as
/// a UI/handler error.
fn dispatch_error_code(error: &CoreError) -> &'static str {
    match error {
        CoreError::ValidationError(msg) if msg.contains("ui.applet_not_dispatchable") => {
            "ui.applet_not_dispatchable"
        }
        CoreError::ValidationError(msg) if msg.contains("no UI handler registered") => {
            "ui.action_not_found"
        }
        // A handler that threw with the `invalid event payload` marker is the
        // contract's payload-validation rejection, not a generic crash. Match
        // case-insensitively on the marker so the engine's `entrypoint threw: …`
        // wrapping (or a capitalized handler message) still classifies.
        CoreError::RuntimeError(msg)
            if msg.to_ascii_lowercase().contains("invalid event payload") =>
        {
            "ui.invalid_event_payload"
        }
        CoreError::RuntimeError(_) => "runtime.handler_error",
        other => other.code(),
    }
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
///   - the signed package's MANIFEST is BOUND to the top-level `manifest` that is
///     stored and enforced via [`bind_signature_to_manifest`] (review 082 #1), so
///     a valid signature over code cannot be installed under a BROADER runtime
///     policy (extra capabilities / different app id, entrypoint, or limits) than
///     the publisher signed — the runtime enforces exactly the signed boundary;
///   - on success the verified publisher / key id (+ whether the policy layer was
///     enforced) is returned as [`InstallTrust::Signed`].
///
/// `publisher_trust` is optional: present → the marketplace-policy layer is
/// enforced (the publisher must be trusted and unexpired); absent → crypto +
/// integrity only, the M0a "verify when present, surface the result" default.
fn verify_install_signature(
    cmd: &CoreCommand,
    applet_id: &AppletId,
    manifest: &Manifest,
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

            // BIND the signed package's manifest/policy to the top-level
            // `manifest` that is stored and enforced (review 082 #1 / 083): the
            // runtime must enforce EXACTLY the capability boundary + resource
            // limits the publisher signed, not a broader one. A signed install
            // whose top-level manifest grants more — a different app id, a wider
            // resource limit, a looser net rule, or a different entrypoint — than
            // the signed package manifest is rejected. The requested `applet_id`
            // is bound to the signed `appId` so a valid signature for one app
            // identity cannot bless a different local applet id (review 083 #1).
            bind_signature_to_manifest(&package, applet_id, manifest, sources)?;

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

/// Confirm the top-level install `manifest` enforces EXACTLY the capability
/// boundary + resource limits the publisher signed (review 082 #1).
///
/// [`bind_signature_to_sources`] binds the signed *code* to the install sources,
/// but `cmd_applet_install` stores and enforces a SEPARATE top-level
/// [`Manifest`] (its `capabilities`/`limits` are what the runtime's policy engine
/// checks every `ctx.*` call against). Without this bind, a valid signature over
/// code could be installed as `Signed` under a BROADER policy than the publisher
/// signed — e.g. `storage app/*` + `db tasks` where the publisher only signed
/// `storage notes/*` + `db notes` — so the runtime would enforce a capability
/// boundary the publisher never blessed.
///
/// This crate cannot *derive* a forge-domain [`Manifest`] from the signed package
/// manifest: the signed shape (prd-merged/08 MP-4 — `appId`, `permissions[]`,
/// `capabilities.{storage,db}.{read,write}`, `capabilities.ui`, `networkPolicy`,
/// `resourceBudget`) carries no `min_api`, so a clean conversion is impossible.
/// Instead we take option (b) — **reject on mismatch**: the policy-bearing fields
/// the publisher signed must match the install manifest EXACTLY. A mismatch is
/// surfaced like any other signature failure (a `ValidationError`), so the
/// install is rejected and nothing is stored.
///
/// The compared dimensions are exactly the runtime-enforced policy surface (review
/// 083 widened this from the prior partial surface):
///   - `appId` vs the requested `applet_id` — a valid signature for one app
///     identity must not bless a DIFFERENT local applet id (review 083 #1);
///   - `capabilities.storage.read` / `.write` (as a set);
///   - `capabilities.db.read` / `.write` (as a set);
///   - `capabilities.ui` (bool — signed `true`/absent is permissive in M0a);
///   - the WHOLE normalized net rule (method, url, `max_response_bytes`,
///     `max_body_bytes`, `timeout_ms`, request/response content types,
///     `allow_secret_headers`) — a signed install must not loosen a cap or add a
///     secret header (review 083 #3);
///   - EVERY enforced resource limit — `wall_ms`, `memory_bytes`, `fuel`,
///     `max_host_calls`, `storage_bytes`, `log_bytes`. A limit the signed
///     `resourceBudget` declares must equal the install's; a limit the signed
///     budget OMITS must equal the runtime default, so a signed install cannot
///     widen an unstated budget (review 083 #2);
///   - the runnable `entrypoint`. For a single-file signed package the entrypoint
///     must be that one file; a signed MULTI-FILE package is rejected because the
///     signed manifest does not (yet) carry an entrypoint to pin which file runs
///     (review 083 #4).
fn bind_signature_to_manifest(
    package: &Package,
    applet_id: &AppletId,
    install: &Manifest,
    sources: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let reject = |reason: String| -> Result<()> {
        Err(CoreError::ValidationError(format!(
            "install manifest does not match the signed package manifest: {reason}"
        )))
    };

    let signed = &package.manifest;
    let signed_caps = signed.get("capabilities");

    // --- app identity: the signed appId must equal the installed applet id ----
    // (review 083 #1). The signed preimage binds `appId`, so a valid signature
    // for one app identity cannot be attached to a different local applet id and
    // still report Signed — provenance and upgrade identity stay bound.
    let signed_app_id = signed.get("appId").and_then(serde_json::Value::as_str);
    match signed_app_id {
        Some(id) if id == applet_id.as_str() => {}
        Some(id) => {
            return reject(format!(
                "appId {id:?} differs from the installed applet id {:?}",
                applet_id.as_str()
            ));
        }
        None => {
            return reject("signed package manifest is missing `appId`".into());
        }
    }

    // --- fail closed on UNKNOWN signed policy fields (review 086 #1) -----------
    //     The signed manifest is hashed and signed WHOLE, but the bind below
    //     narrows it through today-only shapes: capability sub-objects, each
    //     `networkPolicy.allow[]` rule, and `resourceBudget` are interpreted
    //     key-by-key, and the runtime `NetRule` tolerates unknown fields for
    //     forward-compat (see `forge_domain::NetRule`). That tolerance is fine
    //     for the runtime, but on the SIGNED-INSTALL path it is a hole: a signed
    //     package could carry a FUTURE, tighter constraint this core does not
    //     understand (a new net field, a new `resourceBudget` limit such as
    //     `network_bytes`/`output_bytes`, a new capability namespace) and we
    //     would silently install it as Signed WITHOUT enforcing that constraint.
    //     prd-merged/08 §08:24 is fail-closed: clients REFUSE packages that
    //     declare features they do not support. So here — scoped to the signed
    //     bind, NOT the global runtime tolerance — reject the install whenever a
    //     signed policy sub-object contains a key this core cannot enforce.
    reject_unknown_signed_policy_fields(signed, signed_caps)?;

    // --- storage scopes (read/write), compared as order-independent sets ------
    for action in ["read", "write"] {
        let signed_scope = signed_string_set(signed_caps, "storage", action);
        let install_scope: std::collections::BTreeSet<&str> = match action {
            "read" => install.capabilities.storage.read.iter(),
            _ => install.capabilities.storage.write.iter(),
        }
        .map(String::as_str)
        .collect();
        if signed_scope != install_scope {
            return reject(format!(
                "storage.{action} grant {:?} differs from the signed {:?}",
                sorted_vec(&install_scope),
                sorted_vec(&signed_scope)
            ));
        }
    }

    // --- db scopes (read/write), compared as order-independent sets -----------
    for action in ["read", "write"] {
        let signed_scope = signed_string_set(signed_caps, "db", action);
        let install_scope: std::collections::BTreeSet<&str> = match action {
            "read" => install.capabilities.db.read.iter(),
            _ => install.capabilities.db.write.iter(),
        }
        .map(String::as_str)
        .collect();
        if signed_scope != install_scope {
            return reject(format!(
                "db.{action} grant {:?} differs from the signed {:?}",
                sorted_vec(&install_scope),
                sorted_vec(&signed_scope)
            ));
        }
    }

    // --- ui: a signed `ui: false` must not be installed as `ui: true` ---------
    // (absent signed `ui` is treated as granted, matching the M0a manifest
    // default where an absent `capabilities.ui` grants UI).
    let signed_ui = signed_caps
        .and_then(|c| c.get("ui"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if signed_ui != install.capabilities.ui {
        return reject(format!(
            "ui grant {} differs from the signed {signed_ui}",
            install.capabilities.ui
        ));
    }

    // --- network egress: the WHOLE normalized net rule set must match (review
    //     083 #3). The signed shape is `networkPolicy.allow[]`; the install shape
    //     is `capabilities.net[]`. Both are normalized to the SAME [`NetRule`]
    //     type and compared as order-independent sets, so a signed install that
    //     keeps the same (method, url) but loosens a cap (`max_response_bytes`,
    //     `max_body_bytes`, `timeout_ms`), changes a content-type constraint, or
    //     ADDS an `allow_secret_headers` entry no longer passes — every SC-5
    //     constraint the runtime enforces is bound, not just routing.
    let signed_net = signed_net_rules(signed)?;
    let install_net: std::collections::BTreeSet<NormalizedNetRule> = install
        .capabilities
        .net
        .rules()
        .iter()
        .map(NormalizedNetRule::from_rule)
        .collect();
    if signed_net != install_net {
        return reject(format!(
            "network egress grant {:?} differs from the signed {:?}",
            sorted_net_rules(&install_net),
            sorted_net_rules(&signed_net)
        ));
    }

    // --- filesystem grants: the WHOLE normalized files rule set must match
    //     (review 109 #1). Mirrors the net bind exactly: the signed shape is
    //     `capabilities.files.{read,write}[]`; the install shape is the same. Both
    //     sides normalize every `FileRule` (handle, path_glob, max_bytes,
    //     content_types) into [`NormalizedFileRule`] and compare order-independently
    //     per action, so a signed install that ADDS a grant, WIDENS a glob, raises
    //     `max_bytes`, or extends `content_types` no longer matches — every CR-3
    //     confinement the runtime enforces (`forge_runtime::host`) is bound, not
    //     just the handle. The runtime tolerates unknown `FileRule` fields for
    //     forward-compat, but `reject_unknown_signed_policy_fields` already rejects
    //     any unknown signed files key, so the signed/install normalization is total.
    for action in ["read", "write"] {
        let signed_files = signed_file_rules(signed_caps, action)?;
        let install_rules = match action {
            "read" => &install.capabilities.files.read,
            _ => &install.capabilities.files.write,
        };
        let install_files: std::collections::BTreeSet<NormalizedFileRule> = install_rules
            .iter()
            .map(NormalizedFileRule::from_rule)
            .collect();
        if signed_files != install_files {
            return reject(format!(
                "files.{action} grant {:?} differs from the signed {:?}",
                sorted_file_rules(&install_files),
                sorted_file_rules(&signed_files)
            ));
        }
    }

    // --- resource limits: EVERY enforced limit must match (review 083 #2). The
    //     runtime enforces wall_ms, memory_bytes, fuel, max_host_calls,
    //     storage_bytes, and log_bytes from the stored top-level manifest, so a
    //     signed install must not widen ANY of them. A limit the signed
    //     `resourceBudget` declares must equal the install's value; a limit the
    //     signed budget OMITS is bound to the runtime DEFAULT, so a signed install
    //     cannot silently widen an unstated budget. The runtime-enforced default
    //     is the single source of truth ([`forge_domain::Limits::default`]).
    let budget = signed.get("resourceBudget");
    let defaults = forge_domain::Limits::default();
    let limit_checks: [(&str, u64, u64); 6] = [
        ("wall_ms", install.limits.wall_ms, defaults.wall_ms),
        ("memory_bytes", install.limits.memory_bytes, defaults.memory_bytes),
        ("fuel", install.limits.fuel, defaults.fuel),
        ("max_host_calls", install.limits.max_host_calls, defaults.max_host_calls),
        ("storage_bytes", install.limits.storage_bytes, defaults.storage_bytes),
        ("log_bytes", install.limits.log_bytes, defaults.log_bytes),
    ];
    for (name, install_value, default_value) in limit_checks {
        // The signed expectation: the value the signed budget declared, or the
        // runtime default when the signed budget omits this limit.
        let signed_value = budget
            .and_then(|b| b.get(name))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(default_value);
        if install_value != signed_value {
            return reject(format!(
                "limits.{name} {install_value} differs from the signed {signed_value}"
            ));
        }
    }

    // --- entrypoint: the runnable entrypoint must be bound to the signed package
    //     (review 083 #4). The signed manifest does not (yet) carry an entrypoint
    //     field, so a signed MULTI-FILE package cannot pin which file runs — the
    //     install path would otherwise let a caller choose any signed file as the
    //     entrypoint. A single-file signed package is unambiguous: the one signed
    //     file IS the entrypoint, so the install's `entrypoint` must equal that
    //     file's path. Reject multi-file signed installs until the signed manifest
    //     can represent the entrypoint.
    if package.files.len() > 1 {
        return reject(format!(
            "signed multi-file packages are not installable yet: the signed manifest \
             carries no entrypoint to bind which of the {} files runs",
            package.files.len()
        ));
    }
    // bind_signature_to_sources (run before this) already proved `sources` equals
    // the signed files, so for a single-file package the lone source key is the
    // signed entrypoint path. Bind the install's chosen entrypoint to it.
    if let Some(signed_entry) = sources.keys().next() {
        if install.entrypoint != *signed_entry {
            return reject(format!(
                "entrypoint {:?} differs from the signed file {signed_entry:?}",
                install.entrypoint
            ));
        }
    }

    Ok(())
}

/// The set of strings under `manifest.capabilities.<ns>.<action>` (e.g.
/// `capabilities.storage.read`), or the empty set when the namespace/action is
/// absent. Used to compare a signed package's capability scopes against the
/// install manifest's order-independently.
fn signed_string_set<'a>(
    capabilities: Option<&'a serde_json::Value>,
    namespace: &str,
    action: &str,
) -> std::collections::BTreeSet<&'a str> {
    capabilities
        .and_then(|c| c.get(namespace))
        .and_then(|ns| ns.get(action))
        .and_then(serde_json::Value::as_array)
        .map(|arr| arr.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default()
}

/// Fail closed when a SIGNED package's policy carries a key this core cannot
/// enforce (review 086 #1).
///
/// The signed manifest is hashed/signed whole, but `bind_signature_to_manifest`
/// narrows it through today-only shapes — so an unknown key in a policy
/// sub-object would otherwise be dropped on the floor and the package would
/// install as Signed without that (possibly tighter) constraint being enforced.
/// This rejects, scoped to the signed-install bind path only, leaving the
/// runtime [`NetRule`](forge_domain::NetRule) forward-compat tolerance intact
/// for unsigned/already-installed manifests.
///
/// The known-key sets are the exact shapes the rest of this bind interprets:
///   - `capabilities`: `storage`, `db`, `ui`, `net`, `files` (and `storage`/`db`
///     each carry only `read`/`write`; `files` carries `read`/`write` arrays of
///     [`FileRule`](forge_domain::FileRule)-shaped entries);
///   - each `networkPolicy.allow[]` rule: the [`NetRule`](forge_domain::NetRule)
///     fields the policy engine enforces;
///   - `resourceBudget`: the six enforced limit keys.
///
/// A non-object where an object is expected, or any extra key, is a typed
/// rejection (never a panic).
fn reject_unknown_signed_policy_fields(
    signed: &serde_json::Value,
    signed_caps: Option<&serde_json::Value>,
) -> Result<()> {
    // The SC-5 constraints a `NetRule` carries — kept in lockstep with
    // `forge_domain::NetRule` so a NEW signed net field forces an update here
    // (and thus a deliberate enforcement decision) rather than silently passing.
    const NET_RULE_KEYS: &[&str] = &[
        "method",
        "url",
        "max_response_bytes",
        "max_body_bytes",
        "timeout_ms",
        "request_content_types",
        "response_content_types",
        "allow_secret_headers",
    ];
    // The CR-3 constraints a `FileRule` carries — kept in lockstep with
    // `forge_domain::FileRule` so a NEW signed files field forces an update here
    // (and thus a deliberate enforcement decision) rather than silently passing.
    const FILE_RULE_KEYS: &[&str] = &["handle", "path_glob", "max_bytes", "content_types"];
    // The resource limits this core actually enforces (mirrors the six-limit
    // bind below and `forge_domain::Limits`).
    const BUDGET_KEYS: &[&str] = &[
        "wall_ms",
        "fuel",
        "memory_bytes",
        "max_host_calls",
        "storage_bytes",
        "log_bytes",
    ];

    let unknown = |where_: &str, key: &str| -> Result<()> {
        Err(CoreError::ValidationError(format!(
            "install manifest does not match the signed package manifest: the signed \
             package declares an unsupported {where_} field {key:?} this core cannot \
             enforce; refusing to install it as Signed (review 086 #1)"
        )))
    };
    // Reject when `value` (when present) is an object carrying a key outside
    // `known`; a present-but-non-object policy field is also a rejection because
    // the bind cannot interpret it.
    let check_object = |where_: &str,
                        value: Option<&serde_json::Value>,
                        known: &[&str]|
     -> Result<()> {
        let value = match value {
            Some(v) => v,
            None => return Ok(()),
        };
        let obj = value.as_object().ok_or_else(|| {
            CoreError::ValidationError(format!(
                "install manifest does not match the signed package manifest: the signed \
                 package's {where_} is not an object (review 086 #1)"
            ))
        })?;
        for key in obj.keys() {
            if !known.contains(&key.as_str()) {
                return unknown(where_, key);
            }
        }
        Ok(())
    };

    // capabilities.* — only the namespaces this core maps are allowed.
    check_object(
        "capabilities",
        signed_caps,
        &["storage", "db", "ui", "net", "files"],
    )?;
    if let Some(caps) = signed_caps {
        for ns in ["storage", "db"] {
            check_object(
                &format!("capabilities.{ns}"),
                caps.get(ns),
                &["read", "write"],
            )?;
        }
        // capabilities.net[] is policy-bearing and covered by the signed policy
        // hash, so each entry must pass the SAME known-key check as
        // networkPolicy.allow[]. Otherwise a future/tighter net constraint hidden
        // under capabilities.net[] would install as Signed but go unenforced
        // (review 089 #1).
        if let Some(net) = caps.get("net").and_then(serde_json::Value::as_array) {
            for rule in net {
                check_object("capabilities.net[]", Some(rule), NET_RULE_KEYS)?;
            }
        }
        // capabilities.files.{read,write}[] is policy-bearing and covered by the
        // signed policy hash, so — exactly like capabilities.net[] — each entry
        // must carry only known `FileRule` fields. Otherwise a future/tighter
        // files constraint (a new per-action cap) hidden under the signed grant
        // would install as Signed but go unenforced (review 109 #1). The
        // `capabilities.files` object itself may only carry read/write.
        check_object("capabilities.files", caps.get("files"), &["read", "write"])?;
        if let Some(files) = caps.get("files") {
            for action in ["read", "write"] {
                if let Some(rules) = files.get(action).and_then(serde_json::Value::as_array) {
                    for rule in rules {
                        check_object(
                            &format!("capabilities.files.{action}[]"),
                            Some(rule),
                            FILE_RULE_KEYS,
                        )?;
                    }
                }
            }
        }
    }

    // networkPolicy.allow[] — each rule must carry only known NetRule fields.
    if let Some(allow) = signed
        .get("networkPolicy")
        .and_then(|n| n.get("allow"))
        .and_then(serde_json::Value::as_array)
    {
        for rule in allow {
            check_object("networkPolicy.allow[]", Some(rule), NET_RULE_KEYS)?;
        }
    }

    // resourceBudget — only the six enforced limits may appear.
    check_object("resourceBudget", signed.get("resourceBudget"), BUDGET_KEYS)?;

    Ok(())
}

/// The full, normalized form of one network egress rule — every SC-5 constraint
/// the runtime enforces (review 083 #3), not just routing. Both a signed
/// `networkPolicy.allow[]` entry and an install `capabilities.net[]`
/// [`NetRule`](forge_domain::NetRule) normalize to this so they compare
/// order-independently as set elements: method is upper-cased (the policy engine
/// matches case-insensitively), and the content-type / secret-header lists are
/// sorted so declaration order does not matter. A difference in ANY field — a
/// looser cap, an added/changed content type, or a newly allowed secret header —
/// makes two rules unequal, so the bind rejects it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedNetRule {
    method: String,
    url: String,
    max_response_bytes: Option<u64>,
    max_body_bytes: Option<u64>,
    timeout_ms: Option<u64>,
    request_content_types: Vec<String>,
    response_content_types: Vec<String>,
    allow_secret_headers: Vec<String>,
}

impl NormalizedNetRule {
    /// Normalize an install manifest [`NetRule`](forge_domain::NetRule).
    fn from_rule(rule: &forge_domain::NetRule) -> Self {
        let sorted = |v: &[String]| -> Vec<String> {
            let mut out: Vec<String> = v.to_vec();
            out.sort();
            out
        };
        NormalizedNetRule {
            method: rule.method.to_ascii_uppercase(),
            url: rule.url.clone(),
            max_response_bytes: rule.max_response_bytes,
            max_body_bytes: rule.max_body_bytes,
            timeout_ms: rule.timeout_ms,
            request_content_types: sorted(&rule.request_content_types),
            response_content_types: sorted(&rule.response_content_types),
            allow_secret_headers: sorted(&rule.allow_secret_headers),
        }
    }
}

/// The signed network egress allowlist (`networkPolicy.allow[]`) as a set of fully
/// normalized [`NormalizedNetRule`]s. Each signed entry is deserialized through
/// the SAME [`NetRule`](forge_domain::NetRule) type the install manifest uses, so
/// the signed and install sides normalize identically and a missing/extra
/// constraint is caught. A malformed allow entry is a typed rejection, never a
/// panic.
fn signed_net_rules(
    signed: &serde_json::Value,
) -> Result<std::collections::BTreeSet<NormalizedNetRule>> {
    let allow = match signed
        .get("networkPolicy")
        .and_then(|n| n.get("allow"))
        .and_then(serde_json::Value::as_array)
    {
        Some(a) => a,
        None => return Ok(std::collections::BTreeSet::new()),
    };
    let mut out = std::collections::BTreeSet::new();
    for entry in allow {
        let rule: forge_domain::NetRule = serde_json::from_value(entry.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "signed package manifest networkPolicy.allow entry is malformed: {e}"
            ))
        })?;
        out.insert(NormalizedNetRule::from_rule(&rule));
    }
    Ok(out)
}

/// The full, normalized form of one filesystem grant — every CR-3 constraint the
/// runtime enforces (review 109 #1), not just the handle. Both a signed
/// `capabilities.files.{read,write}[]` entry and an install
/// [`FileRule`](forge_domain::FileRule) normalize to this so they compare
/// order-independently as set elements: the `content_types` list is sorted so
/// declaration order does not matter. A difference in ANY field — a wider glob, a
/// bigger `max_bytes`, or an added content type — makes two rules unequal, so the
/// bind rejects it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedFileRule {
    handle: String,
    path_glob: String,
    max_bytes: Option<u64>,
    content_types: Vec<String>,
}

impl NormalizedFileRule {
    /// Normalize an install manifest [`FileRule`](forge_domain::FileRule).
    fn from_rule(rule: &forge_domain::FileRule) -> Self {
        let mut content_types = rule.content_types.clone();
        content_types.sort();
        NormalizedFileRule {
            handle: rule.handle.clone(),
            path_glob: rule.path_glob.clone(),
            max_bytes: rule.max_bytes,
            content_types,
        }
    }
}

/// The signed filesystem grant for `action` (`read`/`write`) as a set of fully
/// normalized [`NormalizedFileRule`]s. Each signed entry is deserialized through
/// the SAME [`FileRule`](forge_domain::FileRule) type the install manifest uses,
/// so the signed and install sides normalize identically and a missing/extra
/// constraint is caught. A malformed entry is a typed rejection, never a panic.
fn signed_file_rules(
    signed_caps: Option<&serde_json::Value>,
    action: &str,
) -> Result<std::collections::BTreeSet<NormalizedFileRule>> {
    let rules = match signed_caps
        .and_then(|c| c.get("files"))
        .and_then(|f| f.get(action))
        .and_then(serde_json::Value::as_array)
    {
        Some(r) => r,
        None => return Ok(std::collections::BTreeSet::new()),
    };
    let mut out = std::collections::BTreeSet::new();
    for entry in rules {
        let rule: forge_domain::FileRule = serde_json::from_value(entry.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "signed package manifest capabilities.files.{action} entry is malformed: {e}"
            ))
        })?;
        out.insert(NormalizedFileRule::from_rule(&rule));
    }
    Ok(out)
}

/// A sorted, readable `Vec` view of a normalized files-rule set, for a stable
/// rejection message that surfaces the full rule (glob + cap + content types).
fn sorted_file_rules(set: &std::collections::BTreeSet<NormalizedFileRule>) -> Vec<String> {
    set.iter()
        .map(|r| {
            format!(
                "{} {} max_bytes<={:?} content_types={:?}",
                r.handle, r.path_glob, r.max_bytes, r.content_types,
            )
        })
        .collect()
}

/// A sorted `Vec` view of a `&str` set, for a stable, readable rejection message.
fn sorted_vec(set: &std::collections::BTreeSet<&str>) -> Vec<String> {
    set.iter().map(|s| s.to_string()).collect()
}

/// A sorted, readable `Vec` view of a normalized net-rule set, for a stable
/// rejection message that surfaces the full rule (caps + secret headers), not
/// just routing.
fn sorted_net_rules(set: &std::collections::BTreeSet<NormalizedNetRule>) -> Vec<String> {
    set.iter()
        .map(|r| {
            format!(
                "{} {} resp<={:?} body<={:?} timeout<={:?} req_ct={:?} resp_ct={:?} secret_hdrs={:?}",
                r.method,
                r.url,
                r.max_response_bytes,
                r.max_body_bytes,
                r.timeout_ms,
                r.request_content_types,
                r.response_content_types,
                r.allow_secret_headers,
            )
        })
        .collect()
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

/// The final UI tree a (replayed) run rendered — the tree of its LAST recorded
/// `ui.render` call (`args = [tree]`), parsed as a [`forge_ui::Node`]. `None` when
/// the run rendered nothing (so the session's diff base does not advance, and an
/// event that renders nothing yields an empty patch). Used by the session-replay
/// path to walk the replayed trees and re-derive each event's UI patch (UI-4).
fn replayed_final_tree(run: &RunRecord) -> Result<Option<forge_ui::Node>> {
    let last_render = run
        .calls
        .iter()
        .rev()
        .find(|c| c.method == "ui.render")
        .and_then(|c| c.args.as_array().and_then(|a| a.first()).cloned());
    match last_render {
        Some(tree_json) => {
            let node = forge_ui::from_str(&tree_json.to_string())?;
            Ok(Some(node))
        }
        None => Ok(None),
    }
}

/// True iff `run` is a dispatched UI event (its recorded trace carries a
/// `ui.dispatch_event` envelope). The initial `runtime.run` that opens a session
/// has none; every `ui.dispatch_event` run has exactly one (recorder.rs). Used to
/// validate a replay session's SHAPE (head = a run, tail = events).
fn is_dispatch_run(run: &RunRecord) -> bool {
    run.calls.iter().any(|c| c.method == "ui.dispatch_event")
}

/// Reject a malformed `runtime.replay_session` `run_ids` list before the patch-chain
/// walk derives a (bogus) "converged" session. A well-formed session is exactly the
/// shape the live `ui.dispatch_event` loop produces and the walk assumes:
///   - `records[0]` is the session HEAD: the initial `runtime.run`, NOT a dispatch
///     (its render is only the diff base for event #1);
///   - every `records[1..]` entry is a dispatched event (a `ui.dispatch_event` run);
///   - no `run_id` appears twice (a session is a linear ordered trace, not a multiset
///     — a duplicate would double-apply one event's diff against itself).
///
/// Any violation is a typed `ValidationError` naming the offending id, so the
/// command's `replays_identically: true` / `final_tree` is a load-bearing claim about
/// a real recorded session, never an artifact of an arbitrary id list.
fn assert_well_formed_session(run_ids: &[String], records: &[&RunRecord]) -> Result<()> {
    // Linear trace: no duplicate ids. (`run_ids` and `records` are 1:1 by index.)
    for i in 0..run_ids.len() {
        for j in (i + 1)..run_ids.len() {
            if run_ids[i] == run_ids[j] {
                return Err(CoreError::ValidationError(format!(
                    "runtime.replay_session `run_ids` must be a linear session but run {} appears more than once",
                    run_ids[i]
                )));
            }
        }
    }
    // Head must be the opening run, not a dispatched event.
    if is_dispatch_run(records[0]) {
        return Err(CoreError::ValidationError(format!(
            "runtime.replay_session head run {} is a dispatched UI event, but a session must start with the initial runtime.run",
            run_ids[0]
        )));
    }
    // Every later entry must be a dispatched event (not another initial run spliced in).
    for (run_id, run) in run_ids[1..].iter().zip(&records[1..]) {
        if !is_dispatch_run(run) {
            return Err(CoreError::ValidationError(format!(
                "runtime.replay_session run {run_id} is not a dispatched UI event, but every run after the head must be a ui.dispatch_event"
            )));
        }
    }
    Ok(())
}

/// Walk an ordered **event session** (`records[0]` is the initial `runtime.run`,
/// `records[1..]` are the dispatched `ui.dispatch_event` runs in order) and derive
/// the OBSERVABLE session output the live `ui.dispatch_event` loop produced: the
/// ordered per-event UI patch chain and the converged final tree (UI-4).
///
/// Each event's patch is `forge_ui::diff(prior_render, this_render)` — diffing this
/// run's final render against the PRIOR run's render, the same diff base the live
/// loop used. A run that rendered nothing leaves the view unchanged: it contributes
/// an empty patch and does NOT advance the diff base (so the next event still diffs
/// against the last real render). The head run contributes no patch (its render is
/// only the base for event #1). Returns `(event_patches, final_tree_json)` where
/// `final_tree_json` is `null` if nothing rendered across the whole session.
///
/// Driving BOTH the recorded and the replayed record sequences through this single
/// walk and asserting the two outputs are byte-equal is the session byte-identity
/// check in [`cmd_runtime_replay_session`](WorkspaceCore::cmd_runtime_replay_session):
/// equal recorded/replayed walks ⇒ every patch and the final tree reproduced exactly.
fn derive_session_patch_chain(
    records: &[&RunRecord],
) -> Result<(Vec<serde_json::Value>, serde_json::Value)> {
    let mut prev_tree: Option<forge_ui::Node> = None;
    let mut event_patches: Vec<serde_json::Value> = Vec::new();
    for (step, run) in records.iter().enumerate() {
        let next_tree = replayed_final_tree(run)?;
        if step > 0 {
            // Every run after the head is a dispatched event. Diff its render against
            // the prior render to the event's patch; a non-rendering run is an empty
            // patch over the unchanged view.
            let patches = match &next_tree {
                Some(tree) => forge_ui::diff(prev_tree.as_ref(), tree),
                None => Vec::new(),
            };
            let patches_json = serde_json::to_value(&patches).map_err(|e| {
                CoreError::ValidationError(format!("replay_session patch serialize failed: {e}"))
            })?;
            event_patches.push(patches_json);
        }
        // Only advance the diff base when this run actually rendered, so a
        // non-rendering event does not blank out the prior tree.
        if next_tree.is_some() {
            prev_tree = next_tree;
        }
    }
    let final_tree = match prev_tree {
        Some(tree) => serde_json::to_value(&tree).map_err(|e| {
            CoreError::ValidationError(format!("replay_session final tree serialize failed: {e}"))
        })?,
        None => serde_json::Value::Null,
    };
    Ok((event_patches, final_tree))
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
mod session_patch_chain_tests {
    use super::*;
    use forge_domain::{AppResult, RecordedCall, RunOutcome};

    /// A minimal `RunRecord` whose only relevant trace is a single `ui.render` of
    /// `tree` (or no render at all when `tree` is `None`) — enough to exercise the
    /// session patch-chain walk without standing up the engine.
    fn rendered(tree: Option<serde_json::Value>) -> RunRecord {
        let calls = match tree {
            Some(t) => vec![RecordedCall {
                seq: 0,
                method: "ui.render".into(),
                args: serde_json::json!([t]),
                response: serde_json::json!(null),
            }],
            None => Vec::new(),
        };
        RunRecord {
            run_id: forge_domain::RunId::new("r"),
            applet_id: AppletId::new("app"),
            code_hash: forge_domain::hash::code_hash("body"),
            input: serde_json::json!(null),
            random_seed: 0,
            time_start: 0,
            calls,
            logs: Vec::new(),
            permissions: forge_domain::PermissionSnapshot::default(),
            outcome: RunOutcome::Completed {
                result: AppResult { ok: true, value: serde_json::json!(null) },
            },
        }
    }

    fn text(t: &str) -> serde_json::Value {
        serde_json::json!({ "type": "Text", "testId": "t", "text": t })
    }

    /// Like [`rendered`] but with a `ui.dispatch_event` envelope appended — the trace
    /// shape of an accepted `ui.dispatch_event` run (a session EVENT, not the head).
    /// `id` lets a test give each run a distinct id to exercise the duplicate guard.
    fn dispatched(id: &str, tree: Option<serde_json::Value>) -> RunRecord {
        let mut run = rendered(tree);
        run.run_id = forge_domain::RunId::new(id);
        run.calls.push(RecordedCall {
            seq: run.calls.len() as u64,
            method: "ui.dispatch_event".into(),
            args: serde_json::json!(["step", {}]),
            response: serde_json::json!(null),
        });
        run
    }

    /// A head run (an initial `runtime.run`: a plain render, no dispatch envelope)
    /// with a distinct id.
    fn head(id: &str, tree: Option<serde_json::Value>) -> RunRecord {
        let mut run = rendered(tree);
        run.run_id = forge_domain::RunId::new(id);
        run
    }

    /// A well-formed session (head run + dispatched events, distinct ids) is accepted.
    #[test]
    fn well_formed_session_is_accepted() {
        let ids = vec!["h".into(), "e1".into(), "e2".into()];
        let h = head("h", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let e2 = dispatched("e2", Some(text("c")));
        assert_well_formed_session(&ids, &[&h, &e1, &e2]).unwrap();
    }

    /// A single-run session (just the head) is well-formed.
    #[test]
    fn single_head_only_session_is_well_formed() {
        let ids = vec!["h".into()];
        let h = head("h", Some(text("a")));
        assert_well_formed_session(&ids, &[&h]).unwrap();
    }

    /// A dispatched event at the HEAD is rejected: a session must open with the
    /// initial `runtime.run`, not a `ui.dispatch_event`.
    #[test]
    fn dispatch_at_head_is_rejected() {
        let ids = vec!["e0".into(), "e1".into()];
        let e0 = dispatched("e0", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let err = assert_well_formed_session(&ids, &[&e0, &e1]).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("head run e0"), "{err}");
    }

    /// A non-dispatch run spliced into the TAIL is rejected: every run after the head
    /// must be a dispatched event, not a second initial run.
    #[test]
    fn non_dispatch_in_tail_is_rejected() {
        let ids = vec!["h".into(), "e1".into(), "h2".into()];
        let h = head("h", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let h2 = head("h2", Some(text("c"))); // a runtime.run spliced mid-session
        let err = assert_well_formed_session(&ids, &[&h, &e1, &h2]).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("run h2"), "{err}");
    }

    /// A duplicated run id is rejected: a session is a linear ordered trace, not a
    /// multiset (a duplicate would double-apply one event's diff against itself).
    #[test]
    fn duplicate_run_id_is_rejected() {
        let ids = vec!["h".into(), "e1".into(), "e1".into()];
        let h = head("h", Some(text("a")));
        let e1 = dispatched("e1", Some(text("b")));
        let e1b = dispatched("e1", Some(text("c")));
        let err = assert_well_formed_session(&ids, &[&h, &e1, &e1b]).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
        assert!(err.to_string().contains("appears more than once"), "{err}");
    }

    /// The head run contributes NO patch (its render is only the diff base); each
    /// subsequent run is an event whose patch diffs its render against the prior
    /// render. The final tree is the last render.
    #[test]
    fn head_is_base_and_events_diff_against_prior_render() {
        let records = [&rendered(Some(text("a"))), &rendered(Some(text("b")))];
        let (patches, final_tree) = derive_session_patch_chain(&records).unwrap();
        assert_eq!(patches.len(), 1, "one event after the head");
        let want = forge_ui::diff(
            Some(&forge_ui::from_str(&text("a").to_string()).unwrap()),
            &forge_ui::from_str(&text("b").to_string()).unwrap(),
        );
        assert_eq!(patches[0], serde_json::to_value(&want).unwrap());
        assert_eq!(final_tree, text("b"));
    }

    /// A non-rendering event contributes an EMPTY patch and does NOT advance the
    /// diff base, so the NEXT event still diffs against the last real render.
    #[test]
    fn non_rendering_event_is_empty_patch_and_does_not_advance_base() {
        let records = [
            &rendered(Some(text("a"))),
            &rendered(None),           // event #1 renders nothing
            &rendered(Some(text("c"))), // event #2 diffs c against a, not against "nothing"
        ];
        let (patches, final_tree) = derive_session_patch_chain(&records).unwrap();
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0], serde_json::json!([]), "non-rendering event = empty patch");
        let want = forge_ui::diff(
            Some(&forge_ui::from_str(&text("a").to_string()).unwrap()),
            &forge_ui::from_str(&text("c").to_string()).unwrap(),
        );
        assert_eq!(patches[1], serde_json::to_value(&want).unwrap(), "next event still diffs against \"a\"");
        assert_eq!(final_tree, text("c"));
    }

    /// An identical re-render is an EMPTY patch (no spurious diff).
    #[test]
    fn identical_rerender_is_empty_patch() {
        let records = [&rendered(Some(text("same"))), &rendered(Some(text("same")))];
        let (patches, _) = derive_session_patch_chain(&records).unwrap();
        assert_eq!(patches[0], serde_json::json!([]));
    }

    /// The walk is ORDER-sensitive: swapping two distinct events yields a different
    /// patch chain and a different final tree — the property the command relies on
    /// to enforce recorded event order.
    #[test]
    fn walk_is_order_sensitive() {
        let head = rendered(Some(text("a")));
        let e_b = rendered(Some(text("b")));
        let e_c = rendered(Some(text("c")));
        let (ordered, ordered_final) =
            derive_session_patch_chain(&[&head, &e_b, &e_c]).unwrap();
        let (swapped, swapped_final) =
            derive_session_patch_chain(&[&head, &e_c, &e_b]).unwrap();
        assert_ne!(ordered, swapped, "swapped order = different patch chain");
        assert_ne!(ordered_final, swapped_final, "swapped order = different final tree");
    }

    /// Two byte-identical record sequences produce byte-identical chains — the
    /// equality the command asserts between the recorded and replayed walks. This
    /// is the building block of the server-side `replays_identically` claim.
    #[test]
    fn identical_record_sequences_produce_identical_chains() {
        let a = [&rendered(Some(text("a"))), &rendered(Some(text("b")))];
        let b = [&rendered(Some(text("a"))), &rendered(Some(text("b")))];
        assert_eq!(
            derive_session_patch_chain(&a).unwrap(),
            derive_session_patch_chain(&b).unwrap()
        );
    }

    /// A single-run session (just the head) has no events: an empty patch chain and
    /// the head's render as the final tree.
    #[test]
    fn single_run_session_has_no_event_patches() {
        let (patches, final_tree) =
            derive_session_patch_chain(&[&rendered(Some(text("only")))]).unwrap();
        assert!(patches.is_empty());
        assert_eq!(final_tree, text("only"));
    }
}

#[cfg(test)]
mod dispatch_error_code_tests {
    use super::*;

    // Pin the T034 renderer-facing classification (`forge/fixtures/ui-events`)
    // independent of the JS engine path: each rejection family maps to the stable
    // code a renderer keys on, and an unrelated typed error keeps its own code.

    #[test]
    fn suspended_gate_maps_to_applet_not_dispatchable() {
        let e = CoreError::ValidationError(
            "ui.applet_not_dispatchable: applet x is suspended; UI events are rejected before dispatch".into(),
        );
        assert_eq!(dispatch_error_code(&e), "ui.applet_not_dispatchable");
    }

    #[test]
    fn unknown_handler_maps_to_action_not_found() {
        // The engine raises exactly this message for a missing handler
        // (engine.rs `Entry::resolve`); the classifier keys off the marker.
        let e = CoreError::ValidationError(
            "no UI handler registered for action ref \"counter.delete_everything\"".into(),
        );
        assert_eq!(dispatch_error_code(&e), "ui.action_not_found");
    }

    #[test]
    fn handler_throw_maps_to_runtime_handler_error() {
        // A generic uncaught JS throw (no marker) is a `runtime.handler_error`.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError("boom".into())),
            "runtime.handler_error"
        );
        // Even when wrapped by the engine's `entrypoint threw: …` prefix.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError("entrypoint threw: boom".into())),
            "runtime.handler_error"
        );
    }

    #[test]
    fn invalid_payload_throw_maps_to_invalid_event_payload() {
        // A handler that threw with the `invalid event payload` marker is the
        // contract's dedicated payload-validation code, NOT a generic crash —
        // so a renderer can re-prompt the field instead of showing a fatal error.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError(
                "invalid event payload: value must be a string".into()
            )),
            "ui.invalid_event_payload"
        );
        // The marker still classifies through the engine's `entrypoint threw: …`
        // wrapping and is matched case-insensitively.
        assert_eq!(
            dispatch_error_code(&CoreError::RuntimeError(
                "entrypoint threw: Error: Invalid Event Payload: value must be a string".into()
            )),
            "ui.invalid_event_payload"
        );
    }

    #[test]
    fn capability_or_limit_failure_keeps_its_own_code() {
        // A `ctx.*` call the manifest did not grant must NOT be relabeled as a
        // UI/handler error — it keeps its typed code so an authz/limit failure
        // stays distinguishable from a missing handler or a handler throw.
        assert_eq!(
            dispatch_error_code(&CoreError::PermissionDenied("storage.set".into())),
            "PermissionDenied"
        );
        assert_eq!(
            dispatch_error_code(&CoreError::ResourceLimitExceeded("fuel".into())),
            "ResourceLimitExceeded"
        );
        // A non-marked ValidationError (not a UI dispatch marker) keeps its kind.
        assert_eq!(
            dispatch_error_code(&CoreError::ValidationError("applet x is not installed".into())),
            "ValidationError"
        );
    }
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
