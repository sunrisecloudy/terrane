//! [`StorageHostBridge`]: the [`HostBridge`] that backs `ctx.*` effects with the
//! real workspace [`Store`] — this is where the spine's "Rust capability ctx →
//! SQLite write → UI tree patch" links live.
//!
//! prd-merged/01 CR-1/CR-3 (effects injected through `ctx`, never imported) +
//! prd-merged/02 DL-4/DL-18 (records projection, KV namespaces) +
//! prd-merged/05 UI-1 (tree diff → patches).
//!
//! The bridge is a thin effect surface: policy/capability gating is enforced one
//! layer up by the runtime's [`HostContext`](forge_runtime::HostContext) (built
//! from a [`PolicyEngine`](forge_policy::PolicyEngine)) *before* any method here
//! runs, exactly as the [`HostBridge`] contract promises. So a denied call never
//! reaches the Store.
//!
//! Two effects are special:
//!   * `db.insert` routes the record write through the storage **CRDT-backed
//!     mutation path** ([`Store::apply_mutation_crdt`](forge_storage::Store::apply_mutation_crdt),
//!     DL-4): the insert becomes a Loro op on the collection's `RecordsDoc`, the
//!     incremental update is appended to `crdt_chunks` (+ an oplog row), AND the
//!     `records` projection row is materialized — all in one SQLite transaction.
//!     The CRDT docs are the source of truth and the projection is derived /
//!     rebuildable (DL-6). This is the literal **SQLite write** link of the spine;
//!     it returns the new record id, observably unchanged from the prior
//!     projection-only write so the recorded trace (and replay) is byte-identical.
//!   * `ui.render` parses the rendered tree into a [`forge_ui::Node`], diffs it
//!     against the previously-rendered tree, and captures the resulting
//!     [`forge_ui::Patch`] list so [`WorkspaceCore`](crate::WorkspaceCore) can
//!     emit a `ui.patch` `CoreEvent` per render — the **UI tree patch** link.

use forge_domain::{CoreError, LogicalTimestamp, Result};
use forge_runtime::{
    FileSystem, HostBridge, HttpClient, InMemoryFileSystem, InMemorySecretStore, NetRequest,
    NetResponse, SecretStore,
};
use forge_storage::{AggregateResult, IndexManager, Mutation, Query, QueryResult, Store};
use serde::Serialize;
use std::collections::BTreeMap;

/// The applet-facing JSON shape of an aggregate result bundle returned by
/// `ctx.db.query` over an aggregate plan. Mirrors [`AggregateResult`] with stable,
/// serializable field names so an applet can read `count`/`sum`/`avg`/`min`/`max`.
#[derive(Debug, Clone, Serialize)]
struct AggregateJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sum: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    avg: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<serde_json::Value>,
}

impl From<AggregateResult> for AggregateJson {
    fn from(a: AggregateResult) -> Self {
        AggregateJson {
            count: a.count,
            sum: a.sum,
            avg: a.avg,
            min: a.min,
            max: a.max,
        }
    }
}

/// The applet-facing JSON shape of one group bucket (`{key, aggregate}`) returned
/// by `ctx.db.query` over a group-by plan.
#[derive(Debug, Clone, Serialize)]
struct GroupJson {
    key: serde_json::Value,
    aggregate: AggregateJson,
}

/// The default [`HttpClient`] for a [`StorageHostBridge`]: it performs **no**
/// network and refuses every request with `PlatformUnavailable` (prd-merged/01
/// CR-3 `PlatformUnavailable`).
///
/// This is the fail-closed default so CI, the demo, and any caller that does not
/// explicitly opt into networking never makes — and never *can* make — a live
/// request: a real client is wired only by a host/shell via
/// [`StorageHostBridge::with_http_client`] (tests inject a mock). The bridge's
/// [`net_fetch`](StorageHostBridge::net_fetch) is itself reached only in record
/// mode and only **after** the runtime's [`HostContext`] has run the SC-5
/// [`NetPolicy`](forge_policy::NetPolicy) egress check — so a denied fetch never
/// reaches this client at all; this stub governs the *allowed-but-no-client* case
/// (prd-merged/01 CR-8: live network is forbidden unless a recorded response is
/// being served).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoNetworkClient;

impl HttpClient for NoNetworkClient {
    fn send(&self, _request: NetRequest) -> Result<NetResponse> {
        Err(CoreError::PlatformUnavailable(
            "no network client configured".to_string(),
        ))
    }
}

/// A single UI render captured during a run: the full tree the applet rendered,
/// plus the minimal patch list that turns the *previous* rendered tree into it
/// (prd-merged/05 UI-1). The first render diffs against `None` → a single
/// root `replace`.
#[derive(Debug, Clone)]
pub struct UiRender {
    /// The full rendered node tree (canonical JSON).
    pub tree: serde_json::Value,
    /// The patch list from the previous tree to this one (canonical JSON).
    pub patches: serde_json::Value,
}

/// One live-query subscription change an applet requested during a run, in call
/// order (DL-16). `ctx.db.watch` produces a [`WatchIntent::Watch`]; `ctx.db.unwatch`
/// a [`WatchIntent::Unwatch`]. The bridge records the *intent* (the registry it must
/// mutate is workspace state the facade owns, not the per-run `&mut Store` the bridge
/// holds); the facade DRAINS these after the run and applies them to the workspace
/// [`WatchRegistry`](forge_storage::WatchRegistry), exactly as it drains `ui_renders`
/// (review: keep the registry mutation off the bridge's borrow of the store).
#[derive(Debug, Clone)]
pub enum WatchIntent {
    /// Register/replace a watch over `query` (the query's `from` is the watched
    /// collection; already capability-checked as `db.read` by the runtime host).
    Watch {
        watch_id: String,
        query: serde_json::Value,
    },
    /// Cancel the watch under `watch_id` (idempotent).
    Unwatch { watch_id: String },
}

/// A [`HostBridge`] backed by a real [`Store`], scoped to one applet.
///
/// `ctx.storage` keys are namespaced per applet (`applet/<id>` namespace) so two
/// applets in the same workspace can't read or clobber each other's KV (DL-18).
/// `ctx.db` writes land in the shared `records` projection keyed by the
/// collection the applet names (capability gating upstream limits *which*
/// collections it may touch).
pub struct StorageHostBridge<'a> {
    store: &'a mut Store,
    /// Applet id, used to scope the KV namespace.
    applet_ns: String,
    /// Logical clock for record `created_at`/`updated_at`; advances per write so
    /// the run's effects are ordered without consulting wall-clock.
    logical: LogicalTimestamp,
    /// Per-collection monotone counter for deterministic record ids
    /// (`<collection>/<n>`), mirroring the in-memory bridge so a real run's ids
    /// are reproducible.
    db_counter: BTreeMap<String, u64>,
    /// Dynamic-index manager threaded into the CRDT write path so an inserted
    /// record keeps any active FTS5 shadow rows in sync inside the same write
    /// transaction (DL-5). M0a constructs no indexes through this bridge, so it is
    /// an empty manager and the FTS sync is a cheap no-op; it exists so the CRDT
    /// mutation surface is never bypassed (see [`db_insert`](Self::db_insert)).
    indexes: IndexManager,
    /// The previous rendered tree, used as the diff base for the next render.
    prev_ui: Option<forge_ui::Node>,
    /// Every UI render captured this run (tree + patch list), in order.
    pub ui_renders: Vec<UiRender>,
    /// Every live-query subscription change requested this run, in call order
    /// (DL-16). The facade drains this after the run and applies it to the
    /// workspace [`WatchRegistry`](forge_storage::WatchRegistry).
    pub watch_intents: Vec<WatchIntent>,
    /// Every log line captured this run.
    pub logs: Vec<String>,
    /// The injectable HTTP client backing `ctx.net.fetch` (prd-merged/07 SC-5,
    /// prd-merged/01 CR-3 `net`). The bridge performs no networking itself; it
    /// delegates to this seam *after* the runtime's [`HostContext`] has run the
    /// [`NetPolicy`](forge_policy::NetPolicy) egress check. The default is
    /// [`NoNetworkClient`] (refuses every request with `PlatformUnavailable`) so
    /// CI/the demo never reach the network; a host/shell injects a real client via
    /// [`with_http_client`](Self::with_http_client) and tests inject a mock.
    http: Box<dyn HttpClient>,
    /// The secret store the host resolves `secret_ref` headers against at the
    /// HTTP edge (prd-merged/07 SC-13). The runtime's [`HostContext`] consults
    /// this ONLY inside the `net_fetch` recording closure to inject a resolved
    /// value into the outgoing request handed to [`net_fetch`](Self::net_fetch);
    /// the value never reaches the recorded trace, the applet, or any log.
    ///
    /// The default is an EMPTY [`InMemorySecretStore`] (every name unknown ⇒ a
    /// secret_ref fails closed). The concrete OS-keychain-backed store is wired
    /// host-side / shell-side (out of this crate's scope) via
    /// [`with_secret_store`](Self::with_secret_store).
    secret_store: Box<dyn SecretStore>,
    /// The sandboxed filesystem `ctx.files` resolves handles/paths against
    /// (prd-merged/01 CR-3, prd-merged/07 SC-8/SC-10/SC-12, `forge/spec/files.md`).
    /// The runtime's [`HostContext`] consults this ONLY **after** it has
    /// capability-checked the op against the running applet's manifest
    /// `files.<read|write>` grant and confined the path to the handle's sandbox
    /// root — to resolve the per-applet handle root, ask whether a symlink target
    /// escapes the root, and (in record mode) perform the confined read/write.
    /// On **replay** the recorder serves the recorded bytes and this is never
    /// consulted (CR-8: no live filesystem unless a recorded response is served).
    ///
    /// The trusted handle → per-applet-root resolution is the **host policy** and
    /// lives in this filesystem (a handle the host has not granted a root for
    /// resolves to `None` ⇒ `PermissionDenied`); the manifest never names a native
    /// root. The default is an EMPTY [`InMemoryFileSystem`] (no granted root ⇒
    /// every `ctx.files` op fails closed). A host/shell wires a real per-applet
    /// sandbox filesystem via [`with_file_system`](Self::with_file_system); tests
    /// inject an in-memory one.
    file_system: Box<dyn FileSystem>,
}

impl<'a> StorageHostBridge<'a> {
    /// Build a bridge over `store`, scoped to `applet_id`, with the fail-closed
    /// [`NoNetworkClient`] as the `ctx.net.fetch` seam (no live network unless a
    /// real client is injected via [`with_http_client`](Self::with_http_client)).
    pub fn new(store: &'a mut Store, applet_id: &str) -> Self {
        Self::with_http_client(store, applet_id, Box::new(NoNetworkClient))
    }

    /// Build a bridge with an explicit injected [`HttpClient`] for `ctx.net.fetch`
    /// (a host/shell wires a real client here; tests inject a mock). Everything
    /// else matches [`new`](Self::new). Keeping the client injectable is what keeps
    /// CI/the demo network-free: nothing in this crate constructs a live client.
    pub fn with_http_client(
        store: &'a mut Store,
        applet_id: &str,
        http: Box<dyn HttpClient>,
    ) -> Self {
        StorageHostBridge {
            store,
            applet_ns: format!("applet/{applet_id}"),
            logical: LogicalTimestamp::default(),
            db_counter: BTreeMap::new(),
            indexes: IndexManager::new(),
            prev_ui: None,
            ui_renders: Vec::new(),
            watch_intents: Vec::new(),
            logs: Vec::new(),
            http,
            // Fail-closed default: empty store ⇒ any secret_ref header is denied
            // until a host/shell injects a real secret store.
            secret_store: Box::new(InMemorySecretStore::new()),
            // Fail-closed default: empty filesystem ⇒ no handle has a granted root,
            // so every ctx.files op is PermissionDenied until a host/shell injects a
            // real per-applet sandbox filesystem.
            file_system: Box::new(InMemoryFileSystem::new()),
        }
    }

    /// Inject the [`SecretStore`] the host resolves `secret_ref` headers against
    /// at the HTTP edge (prd-merged/07 SC-13). A host/shell wires its real
    /// OS-keychain-backed store here; tests inject an in-memory store. Builder
    /// style; everything else matches [`with_http_client`](Self::with_http_client).
    /// Without this the store is empty and every secret_ref fails closed.
    pub fn with_secret_store(mut self, secret_store: Box<dyn SecretStore>) -> Self {
        self.secret_store = secret_store;
        self
    }

    /// Inject the sandboxed [`FileSystem`] `ctx.files` resolves handles/paths
    /// against (prd-merged/01 CR-3, prd-merged/07 SC-8/SC-10/SC-12,
    /// `forge/spec/files.md`). The filesystem carries the **trusted** handle →
    /// per-applet-sandbox-root resolution (a handle with no granted root is
    /// `PermissionDenied`), so the manifest never names a native root. A host/shell
    /// wires its real per-applet sandbox filesystem here; tests inject an
    /// [`InMemoryFileSystem`]. Builder style. Without this the filesystem is empty
    /// (no granted root) and every `ctx.files` op fails closed.
    ///
    /// The capability gate is still the runtime's: the injected filesystem is
    /// consulted only for an *allowed*, *record-mode* op whose path the runtime
    /// already confined to the handle's root (replay serves the recording; a denied
    /// or confinement-rejected op never reaches the filesystem).
    pub fn with_file_system(mut self, file_system: Box<dyn FileSystem>) -> Self {
        self.file_system = file_system;
        self
    }

    /// Advance and return the next logical timestamp for a write.
    fn tick(&mut self) -> LogicalTimestamp {
        self.logical = self.logical.next();
        self.logical
    }

    /// Validate the JSON `record` an applet passed to `ctx.db.insert` and return
    /// its display-named `fields` map (the `Mutation::Insert` field shape). A
    /// non-object record is rejected as a `ValidationError` (the `DbRecord`
    /// contract is an object).
    fn record_fields(
        record: serde_json::Value,
    ) -> Result<serde_json::Map<String, serde_json::Value>> {
        match record {
            serde_json::Value::Object(map) => Ok(map),
            other => Err(CoreError::ValidationError(format!(
                "ctx.db.insert record must be an object, got {other}"
            ))),
        }
    }
}

impl HostBridge for StorageHostBridge<'_> {
    fn storage_get(&mut self, key: &str) -> Result<serde_json::Value> {
        match self.store.kv_get(&self.applet_ns, key)? {
            // Stored as canonical JSON bytes; parse back to a JSON value so the
            // applet sees structured data, not a string blob.
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                CoreError::StorageError(format!("ctx.storage.get decode failed: {e}"))
            }),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(&value)
            .map_err(|e| CoreError::StorageError(format!("ctx.storage.set encode failed: {e}")))?;
        self.store
            .kv_set(&self.applet_ns, key, &bytes, "application/json")
    }

    fn storage_delete(&mut self, key: &str) -> Result<()> {
        self.store.kv_delete(&self.applet_ns, key)
    }

    fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>> {
        self.store.kv_list(&self.applet_ns, prefix)
    }

    fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String> {
        let fields = Self::record_fields(record)?;
        // Deterministic, readable record id: `<collection>/<n>`. The per-run
        // counter is seeded on first use from the count of records already in the
        // collection, so ids never collide with a prior run's writes (each run
        // would otherwise restart at 1 and clobber `<collection>/1`). The id is
        // captured into the recorded trace, so replay (which serves the recorded
        // response) reproduces it without re-running this generator.
        let next = match self.db_counter.get(collection) {
            Some(n) => n + 1,
            None => {
                let existing = self.store.list_records(collection)?.len() as u64;
                existing + 1
            }
        };
        self.db_counter.insert(collection.to_string(), next);
        let id = format!("{collection}/{next}");
        let at = self.tick();
        // THE SQLite write link of the spine — now the CRDT-backed write path
        // (DL-4): the insert becomes a Loro op on the collection's RecordsDoc, the
        // incremental update is appended to `crdt_chunks` (+ an oplog row), AND the
        // `records` projection row is materialized — all in ONE SQLite transaction.
        // The CRDT docs are the source of truth; the projection is derived and
        // rebuildable (`Store::rebuild_projection`, DL-6). Observable behavior is
        // unchanged: the inserted record is still queryable/returned by the same
        // id, so the recorded trace (the returned id) — and therefore replay — is
        // byte-identical to the prior projection-only write.
        let mutation = Mutation::Insert {
            collection: collection.to_string(),
            id: Some(id.clone()),
            fields,
            logical_at: Some(at.0 as i64),
        };
        self.store.apply_mutation_crdt(&mutation, &self.indexes)?;
        Ok(id)
    }

    fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        match self.store.get_record(collection, id)? {
            Some(env) => serde_json::to_value(env.fields).map_err(|e| {
                CoreError::StorageError(format!("ctx.db.get encode failed: {e}"))
            }),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        let records = self.store.list_records(collection)?;
        records
            .into_iter()
            .map(|env| {
                serde_json::to_value(env.fields).map_err(|e| {
                    CoreError::StorageError(format!("ctx.db.list encode failed: {e}"))
                })
            })
            .collect()
    }

    fn db_query(
        &mut self,
        collection: &str,
        query: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // The applet passes the structured query plan; the trusted collection
        // (already capability-checked upstream as `db.read`) is authoritative for
        // `from`, so an applet cannot widen the query to a collection it lacks
        // read on by naming a different `from` in the payload (core 048#1).
        let mut q = Query::from_fixture_value(&query)?;
        q.from = collection.to_string();
        let result = self.store.query(&q)?;
        // Mirror `ctx.db.list`: a row result surfaces each record's display
        // `fields` map. Aggregate/group shapes serialize their result bundle.
        let rows = match result {
            QueryResult::Rows(rows) => rows
                .into_iter()
                .map(|row| {
                    serde_json::to_value(row.envelope.fields).map_err(|e| {
                        CoreError::StorageError(format!("ctx.db.query encode failed: {e}"))
                    })
                })
                .collect::<Result<Vec<_>>>()?,
            QueryResult::Aggregate(agg) => vec![serde_json::to_value(AggregateJson::from(agg))
                .map_err(|e| {
                    CoreError::StorageError(format!("ctx.db.query encode failed: {e}"))
                })?],
            QueryResult::Groups(groups) => groups
                .into_iter()
                .map(|g| {
                    serde_json::to_value(GroupJson {
                        key: g.key,
                        aggregate: AggregateJson::from(g.aggregate),
                    })
                    .map_err(|e| {
                        CoreError::StorageError(format!("ctx.db.query encode failed: {e}"))
                    })
                })
                .collect::<Result<Vec<_>>>()?,
        };
        Ok(serde_json::Value::Array(rows))
    }

    fn db_watch(&mut self, watch_id: &str, query: serde_json::Value) -> Result<String> {
        // Validate the watch query is a parseable, watchable ROW query BEFORE
        // capturing the intent, so a malformed/aggregate/group watch is rejected at
        // registration (DL-16, review 129 #2) rather than silently captured and
        // failing later in the facade. `WatchRegistry::register_from_value` runs the
        // same canonical parse + non-row rejection the facade will, but it needs a
        // registry; here we mirror its guard with the shared parser + a cheap
        // aggregate/group check so the bridge fails fast and identically.
        let q = Query::from_fixture_value(&query)?;
        if q.aggregate.is_some() {
            return Err(CoreError::QueryError(
                "ctx.db.watch does not support aggregate queries; watch the underlying rows \
                 and reduce in the callback"
                    .into(),
            ));
        }
        if q.group_by.is_some() {
            return Err(CoreError::QueryError(
                "ctx.db.watch does not support groupBy queries; watch the underlying rows \
                 and group in the callback"
                    .into(),
            ));
        }
        self.watch_intents.push(WatchIntent::Watch {
            watch_id: watch_id.to_string(),
            query,
        });
        Ok(watch_id.to_string())
    }

    fn db_unwatch(&mut self, watch_id: &str) -> Result<()> {
        self.watch_intents.push(WatchIntent::Unwatch {
            watch_id: watch_id.to_string(),
        });
        Ok(())
    }

    fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        // Parse the rendered tree into a typed Node (unknown component types are
        // tolerated as Node::Unknown, UI-6 — never an error here).
        let node = forge_ui::from_str(&tree.to_string())?;
        // Diff against the previous tree → minimal index-path patches (UI-1).
        let patches = forge_ui::diff(self.prev_ui.as_ref(), &node);
        let patches_json = serde_json::to_value(&patches).map_err(|e| {
            CoreError::ValidationError(format!("ui patch serialize failed: {e}"))
        })?;
        // Re-serialize the parsed node canonically so the emitted tree is the
        // catalog-normalized shape (and round-trips for the renderer).
        let canonical = forge_ui::to_canonical_string(&node)?;
        let tree_json = serde_json::from_str(&canonical).map_err(|e| {
            CoreError::ValidationError(format!("ui tree re-parse failed: {e}"))
        })?;
        self.ui_renders.push(UiRender {
            tree: tree_json,
            patches: patches_json,
        });
        self.prev_ui = Some(node);
        Ok(())
    }

    fn log(&mut self, line: &str) -> Result<()> {
        self.logs.push(line.to_string());
        Ok(())
    }

    /// `ctx.net.fetch(request)` — perform the request through the injected
    /// [`HttpClient`] (prd-merged/07 SC-5, prd-merged/01 CR-3 `net`).
    ///
    /// This method is reached **only in record mode** and **only after** the
    /// runtime's [`HostContext`](forge_runtime::HostContext) has run the SC-5
    /// [`NetPolicy`](forge_policy::NetPolicy) egress check against the running
    /// applet's manifest `net` allowlist and the host-call budget — a denied fetch
    /// (empty allowlist → `CapabilityRequired`; host/scheme/path/method/size/
    /// timeout/content-type/secret-header/private-IP violation → `PermissionDenied`)
    /// never reaches here. On **replay** the recorder serves the recorded response
    /// and this method is never called (CR-8: no live network unless a recorded
    /// response is being served).
    ///
    /// The bridge performs no networking itself: it delegates to the injected
    /// client. The default [`NoNetworkClient`] refuses with `PlatformUnavailable`
    /// ("no network client configured") so an allowed fetch with no client wired
    /// fails closed rather than reaching the network — which is exactly the
    /// CI/demo path (they install no `net` capability and inject no client).
    fn net_fetch(&mut self, request: NetRequest) -> Result<NetResponse> {
        // By the time the runtime calls this, any `secret_ref` header has ALREADY
        // been resolved to its literal value at the HTTP edge (the HostContext
        // injects via `secret_store()` inside its recording closure). So `request`
        // here is literal-only; the recorded trace upstream still holds only the
        // secret_ref. The bridge performs no resolution and no networking itself.
        self.http.send(request)
    }

    fn secret_store(&self) -> &dyn SecretStore {
        self.secret_store.as_ref()
    }

    /// The sandboxed filesystem `ctx.files` resolves handles/paths against
    /// (prd-merged/01 CR-3, `forge/spec/files.md`). The runtime's
    /// [`HostContext`](forge_runtime::HostContext) consults this ONLY **after** it
    /// has capability-checked the op against the running applet's manifest
    /// `files.<read|write>` grant and confined the path to the handle's sandbox
    /// root (resolving the handle root and the symlink-escape question through
    /// this seam). On **replay** the recorder serves the recorded bytes and this is
    /// never consulted (CR-8). The bridge performs no policy itself: it returns the
    /// injected, host-trusted filesystem.
    fn file_system(&self) -> &dyn FileSystem {
        self.file_system.as_ref()
    }

    /// `ctx.files.write(handle, rel_path, bytes, content_type)` — write a confined
    /// file through the injected sandbox filesystem (record mode only).
    ///
    /// Reached **only after** the runtime's [`HostContext`](forge_runtime::HostContext)
    /// has capability-checked the write against the manifest `files.write` grant,
    /// confined the path, enforced the byte/content-type caps, and checked
    /// parent-/final-target symlink escape — so a denied or escaping write never
    /// reaches here. On **replay** the recorder serves the recorded write response
    /// and this is never called (CR-8: replay never writes a live file). The bridge
    /// performs no policy itself: it delegates to the injected filesystem.
    fn files_write(
        &mut self,
        handle: &str,
        rel_path: &str,
        bytes: &[u8],
        content_type: Option<&str>,
    ) -> Result<u64> {
        self.file_system.write(handle, rel_path, bytes, content_type)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn db_insert_writes_a_record_into_the_projection() {
        let mut s = store();
        let id = {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.db_insert("tasks", serde_json::json!({ "title": "Ship", "done": false }))
                .unwrap()
        };
        assert_eq!(id, "tasks/1");
        let env = s.get_record("tasks", "tasks/1").unwrap().unwrap();
        assert_eq!(env.fields["title"], serde_json::json!("Ship"));
        assert_eq!(env.fields["done"], serde_json::json!(false));
    }

    #[test]
    fn db_insert_seeds_id_from_existing_records_across_bridges() {
        // Two separate bridges (≈ two runs) over the same store must not collide.
        let mut s = store();
        let id1 = {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.db_insert("tasks", serde_json::json!({ "t": 1 })).unwrap()
        };
        let id2 = {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.db_insert("tasks", serde_json::json!({ "t": 2 })).unwrap()
        };
        assert_eq!(id1, "tasks/1");
        assert_eq!(id2, "tasks/2", "second run must not clobber the first record");
        assert_eq!(s.list_records("tasks").unwrap().len(), 2);
    }

    #[test]
    fn db_insert_rejects_non_object_record() {
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        let err = b.db_insert("tasks", serde_json::json!("not an object")).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn storage_roundtrips_through_kv_namespaced_per_applet() {
        let mut s = store();
        {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.storage_set("app/k", serde_json::json!({ "v": 1 })).unwrap();
            assert_eq!(b.storage_get("app/k").unwrap(), serde_json::json!({ "v": 1 }));
            assert_eq!(b.storage_list("app/").unwrap(), vec!["app/k".to_string()]);
            assert_eq!(b.storage_get("missing").unwrap(), serde_json::Value::Null);
        }
        // A different applet sees an isolated namespace.
        let mut b2 = StorageHostBridge::new(&mut s, "app2");
        assert_eq!(b2.storage_get("app/k").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn ui_render_first_render_is_root_replace_then_diffs() {
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        // First render → diff against None → single root replace.
        b.ui_render(serde_json::json!({
            "type": "Stack", "direction": "v",
            "children": [ { "type": "Text", "text": "A" } ]
        }))
        .unwrap();
        assert_eq!(b.ui_renders.len(), 1);
        let patches = b.ui_renders[0].patches.as_array().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0]["op"], serde_json::json!("replace"));
        assert!(b.ui_renders[0].tree.to_string().contains("\"A\""));

        // Second render changes only the Text → a minimal update_text patch.
        b.ui_render(serde_json::json!({
            "type": "Stack", "direction": "v",
            "children": [ { "type": "Text", "text": "B" } ]
        }))
        .unwrap();
        assert_eq!(b.ui_renders.len(), 2);
        let patches = b.ui_renders[1].patches.as_array().unwrap();
        assert_eq!(patches.len(), 1, "only the text changed → one patch");
        assert_eq!(patches[0]["op"], serde_json::json!("update_text"));
        assert_eq!(patches[0]["value"], serde_json::json!("B"));
    }

    #[test]
    fn ui_render_tolerates_unknown_node_types() {
        // UI-6: an unknown component type is not an error; it round-trips.
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        b.ui_render(serde_json::json!({ "type": "FutureWidget", "x": 1 })).unwrap();
        assert_eq!(b.ui_renders.len(), 1);
        assert!(b.ui_renders[0].tree.to_string().contains("FutureWidget"));
    }

    // --- ctx.net.fetch: injectable HttpClient (SC-5 / CR-3 / CR-8) -----------

    use forge_runtime::{HttpClient, MockHttpClient, NetRequest, NetResponse};

    fn get_req(url: &str) -> NetRequest {
        NetRequest { method: "GET".into(), url: url.into(), ..Default::default() }
    }

    #[test]
    fn net_fetch_default_client_fails_closed_platform_unavailable() {
        // The default bridge wires NoNetworkClient: an (already-policy-approved)
        // fetch with no client configured fails closed, never reaching a socket.
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        let err = b
            .net_fetch(get_req("https://api.example.com/x"))
            .unwrap_err();
        assert_eq!(err.code(), "PlatformUnavailable");
        assert!(err.to_string().contains("no network client configured"), "{err}");
    }

    #[test]
    fn no_network_client_refuses_directly() {
        // The stub itself is the fail-closed default (CR-8: no live network).
        let err = NoNetworkClient.send(get_req("https://api.example.com/x")).unwrap_err();
        assert_eq!(err.code(), "PlatformUnavailable");
    }

    #[test]
    fn net_fetch_delegates_to_an_injected_client() {
        // An injected client (here a canned mock) is consulted by net_fetch and its
        // response is returned verbatim — the seam that lets a host/shell wire real
        // HTTP and a test wire a mock, with no networking in the bridge itself.
        let mut s = store();
        let canned = NetResponse {
            status: 200,
            body: Some(r#"{"ok":true}"#.into()),
            content_type: Some("application/json".into()),
            ..Default::default()
        };
        let mut b = StorageHostBridge::with_http_client(
            &mut s,
            "app1",
            Box::new(MockHttpClient::new(canned)),
        );
        let resp = b.net_fetch(get_req("https://api.example.com/weather")).unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.body.as_deref(), Some(r#"{"ok":true}"#));
    }

    // --- ctx.secrets: header secret_ref injection through the real bridge (SC-13) -

    use forge_domain::{ActorContext, Capabilities, Limits, Manifest, NetGrant, NetRule};
    use forge_runtime::{HostContext, InMemorySecretStore, NetHeaderValue, RunRecorder};
    use std::sync::{Arc, Mutex};

    /// A client that CAPTURES the request it received (so a test can prove the
    /// resolved secret value arrived) and returns a canned 200.
    #[derive(Clone, Default)]
    struct CapturingClient {
        seen: Arc<Mutex<Vec<NetRequest>>>,
    }

    impl HttpClient for CapturingClient {
        fn send(&self, request: NetRequest) -> Result<NetResponse> {
            self.seen.lock().unwrap().push(request);
            Ok(NetResponse {
                status: 200,
                body: Some(r#"{"ok":true}"#.into()),
                content_type: Some("application/json".into()),
                ..Default::default()
            })
        }
    }

    fn secret_manifest() -> Manifest {
        Manifest {
            entrypoint: "main.ts".into(),
            min_api: "forge-api@0.1".into(),
            deterministic: true,
            capabilities: Capabilities {
                net: NetGrant(vec![NetRule {
                    method: "GET".into(),
                    url: "https://api.weather.example/*".into(),
                    allow_secret_headers: vec!["Authorization".into()],
                    ..Default::default()
                }]),
                ..Capabilities::default()
            },
            limits: Limits { max_host_calls: 100, ..Limits::default() },
        }
    }

    fn secret_req() -> NetRequest {
        let mut r =
            NetRequest { method: "GET".into(), url: "https://api.weather.example/now".into(), ..Default::default() };
        r.headers.insert(
            "Authorization".into(),
            NetHeaderValue::Secret { secret_ref: "secret_weather".into() },
        );
        r
    }

    /// End-to-end through the REAL StorageHostBridge: an allowlisted secret header
    /// is resolved against the bridge's injected store and injected into the
    /// outgoing client request, while the recorded trace keeps only the secret_ref
    /// and the resolved value never appears in the trace or the applet's response.
    #[test]
    fn storage_bridge_injects_secret_into_client_but_not_trace() {
        let mut s = store();
        let client = CapturingClient::default();
        let seen = client.seen.clone();
        let secrets =
            InMemorySecretStore::new().with_secret("secret_weather", "Bearer abc123");
        let mut bridge = StorageHostBridge::with_http_client(&mut s, "app1", Box::new(client))
            .with_secret_store(Box::new(secrets));

        let manifest = secret_manifest();
        let actor = ActorContext::owner("dev");
        let mut host = HostContext::new(
            &manifest,
            &actor,
            RunRecorder::recording(1, 0),
            &mut bridge,
        )
        .unwrap();
        let resp = host.net_fetch(secret_req()).unwrap();
        assert_eq!(resp.status, 200);
        // The applet's response carries no secret value.
        assert!(!serde_json::to_string(&resp).unwrap().contains("abc123"));

        let (recorder, _logs) = host.finish();
        let trace = serde_json::to_string(&recorder.into_calls()).unwrap();
        // The trace keeps only the secret_ref — never the resolved value.
        assert!(trace.contains("secret_ref"), "trace must keep the ref: {trace}");
        assert!(trace.contains("secret_weather"), "trace must name the ref: {trace}");
        assert!(!trace.contains("abc123"), "trace leaked the secret value: {trace}");

        // The CLIENT received the RESOLVED literal header value.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "exactly one request reached the client");
        assert_eq!(
            seen[0].headers.get("Authorization").and_then(|h| h.as_literal()),
            Some("Bearer abc123"),
            "client must receive the resolved secret value"
        );
    }

    /// The bridge's default secret store is EMPTY, so a `secret_ref` header fails
    /// closed (RuntimeError) and no request reaches the client.
    #[test]
    fn storage_bridge_default_secret_store_is_empty_fail_closed() {
        let mut s = store();
        let b = StorageHostBridge::new(&mut s, "app1");
        // The default store resolves nothing.
        assert_eq!(b.secret_store().resolve("secret_weather"), None);
    }
}
