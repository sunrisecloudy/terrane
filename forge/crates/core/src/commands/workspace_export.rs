//! `workspace.export` / `workspace.import` — the portable single-file workspace
//! bundle (DL-24). Moved verbatim from `workspace.rs` (/simplify #11a): the two
//! command handlers, their typed in-process API counterparts
//! ([`export_to_file`](WorkspaceCore::export_to_file) /
//! [`import_from_file`](WorkspaceCore::import_from_file)), and the bundle helpers.

use forge_domain::{CoreError, Result};
use forge_storage::{ExportOptions, IndexManager, RunLogPolicy, Store, EXPORT_FORMAT_VERSION};

use super::super::persistence::{META_NS, RUN_COUNTER_KEY};
use super::super::{
    load_db_read_grants, load_schema_registry, rebuild_indexes_from_registry, WorkspaceCore,
    SCHEMA_REGISTRY_KEY,
};
use super::bool_field;

impl WorkspaceCore {
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
    pub(in crate::workspace) fn cmd_workspace_export(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
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
    pub(in crate::workspace) fn cmd_workspace_import(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
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
