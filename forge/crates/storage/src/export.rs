//! Workspace export / import — the DL-24 portable single-file workspace.
//!
//! Normative spec: `prd-merged/02-data-layer-prd.md` DL-24 and
//! `forge/spec/workspace-export-format.md`. A workspace exports as a single
//! **portable SQLite file** containing the *syncable* state — the CRDT
//! chunks/snapshots (the source of truth), the oplog, schema/index definitions
//! that are persisted, the records projection, portable `kv` (applet
//! manifests/programs/schema, NOT secrets or device-local keys), and — per a
//! policy flag — the run records + run logs. Re-importing into a **fresh**
//! `Store` and rebuilding the projection from the imported CRDT chunks (DL-6
//! [`Store::rebuild_projection`](crate::Store::rebuild_projection)) reproduces a
//! **byte-identical records projection**.
//!
//! ## Container & format
//!
//! The export artifact is itself a self-describing SQLite workspace file (the
//! spec's "fresh Store written table-by-table"). [`Store::export_workspace`]
//! writes a brand-new `Store` and copies the syncable tables into it **in a
//! fully deterministic order** (table order fixed below; rows ordered by their
//! primary key / the spec's ordering columns), so two exports of the same
//! workspace are byte-identical. A format-version header lives in the bundle's
//! `meta` table:
//!
//! - `export_format_version` — the open-format version ([`EXPORT_FORMAT_VERSION`]).
//! - `forge_storage_schema_version` — the physical schema version
//!   ([`STORAGE_SCHEMA_VERSION`]).
//!
//! [`Store::import_workspace`] opens the bundle read-only, validates the format
//! version (an unknown version is a clean [`CoreError::StorageError`], never a
//! silent reinterpretation), copies every syncable table into a fresh target
//! `Store`, then calls `rebuild_projection` so the `records` table is
//! reconstructed purely from the imported `crdt_chunks`.
//!
//! ## What is exported (included)
//!
//! | Table          | Included | Ordering (deterministic)            |
//! |----------------|----------|-------------------------------------|
//! | meta           | yes      | key                                  |
//! | kv             | portable | namespace, key (secrets/device excluded) |
//! | oplog          | yes      | lamport, op_id                       |
//! | crdt_chunks    | yes      | doc_id, created_at, chunk_id         |
//! | crdt_snapshots | yes      | doc_id, created_at, snapshot_id      |
//! | records        | yes      | collection, id (re-derived on import)|
//! | runs           | policy   | created_at, run_id                   |
//! | run_logs       | policy   | run_id, seq                          |
//!
//! ## What is NEVER exported (excluded)
//!
//! Local-only / secret state is **never** written to the bundle (DL-24): the
//! `kv` namespaces matching [`is_local_only_namespace`] — secrets, provider
//! credentials, and device-local settings/window state — are filtered out at
//! export, and an exclusion guard test pins this. The reserved `__forge/meta`
//! namespace IS portable: it carries applet manifests/programs/schema pointers
//! and the workspace run counter, which are workspace state rather than secrets.
//! (When secret *references* need to travel they would ride as redacted refs;
//! M0a persists no secret rows, so there is nothing to redact yet — the guard is
//! the forward-compatible enforcement point.)
//!
//! ## Re-import invariant
//!
//! After import + rebuild, the target's `records` projection compares **equal**
//! to the source's under the deterministic `(collection, id)` ordering, live
//! `kv` values + tombstones match, indexes rebuild from canonical records, and
//! (when policy includes them) `runs`/`run_logs` round-trip with their order
//! preserved. This leverages DL-6: records are *derived*, so the CRDT chunks are
//! the portable source and the projection is reconstructed, not trusted.

use crate::index::IndexManager;
use crate::{map_sql, Store};
use forge_domain::{CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

/// The open-format version written to / required from a bundle's `meta` table
/// (`export_format_version`). Bumped only on an incompatible bundle-layout
/// change; an importer refuses a version it does not understand rather than
/// silently reinterpreting unknown data (spec §Versioning).
pub const EXPORT_FORMAT_VERSION: i64 = 1;

/// The physical storage schema version recorded alongside the open-format
/// version (`forge_storage_schema_version`). Lets a future importer migrate an
/// older physical layout explicitly.
pub const STORAGE_SCHEMA_VERSION: i64 = 1;

/// Policy for whether run records + logs travel in the bundle (spec: run logs
/// are policy-dependent and default-excluded for privacy; include them only for
/// an explicit debug/backup bundle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunLogPolicy {
    /// Default: omit `runs` and `run_logs` from the bundle (privacy).
    Exclude,
    /// Debug/backup bundle: include `runs` and `run_logs` (ordered).
    Include,
}

/// Options controlling what an export contains.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// The workspace identifier stamped into the bundle's `meta` (so an import
    /// can carry the source identity). Empty is allowed (anonymous bundle).
    pub workspace_id: String,
    /// Whether run records + logs are included (default: excluded).
    pub run_logs: RunLogPolicy,
}

impl Default for ExportOptions {
    fn default() -> Self {
        ExportOptions {
            workspace_id: String::new(),
            run_logs: RunLogPolicy::Exclude,
        }
    }
}

impl ExportOptions {
    /// A bundle stamped with `workspace_id`, run logs excluded (the default).
    pub fn new(workspace_id: impl Into<String>) -> Self {
        ExportOptions {
            workspace_id: workspace_id.into(),
            run_logs: RunLogPolicy::Exclude,
        }
    }

    /// Include run records + logs (debug/backup bundle).
    pub fn with_run_logs(mut self) -> Self {
        self.run_logs = RunLogPolicy::Include;
        self
    }
}

/// True iff a `kv` namespace holds **local-only / secret** data that must NEVER
/// be exported (DL-24): secrets, provider credentials, and device-local settings
/// / window state. The reserved `__forge/meta` namespace is explicitly portable
/// (applet manifests/programs + run counter), so it is excluded from this guard.
///
/// The match is by reserved namespace prefix so an applet cannot accidentally
/// (or maliciously) smuggle a secret out by choosing a clever key — the whole
/// namespace is dropped. Applet `ctx.storage` namespaces are `applet/<id>` and
/// are portable workspace data, so they are not matched here.
pub fn is_local_only_namespace(namespace: &str) -> bool {
    // Reserved secret / device-local buckets. Each entry is the bucket root; a
    // namespace is dropped when it IS the bucket exactly (a key stored directly
    // under the root, e.g. `secret`) OR is a child of it (`secret/<...>`). Matching
    // both forms closes the gap where an exact root namespace like `secret`,
    // `provider`, or `__device` (no trailing key) would otherwise slip past a
    // prefix-only check and be exported (review 061 P2).
    const LOCAL_ONLY_BUCKETS: &[&str] = &[
        "secret",      // secret values
        "secrets",
        "provider",    // provider credentials / tokens
        "credentials",
        "device",      // device-local settings / window state
        "__device",
        "local",       // local window state / transient UI
        "__local",
    ];
    LOCAL_ONLY_BUCKETS.iter().any(|bucket| {
        namespace == *bucket
            || namespace
                .strip_prefix(bucket)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

impl Store {
    /// Export this workspace's syncable state into a brand-new portable SQLite
    /// **bundle file** at `bundle_path` (DL-24). The bundle is a self-describing
    /// `Store` written table-by-table in a fully deterministic order, so two
    /// exports of the same workspace produce byte-identical files.
    ///
    /// Included: `meta` (with the format-version header), portable `kv`
    /// (secrets / device-local namespaces excluded — see
    /// [`is_local_only_namespace`]), `oplog`, `crdt_chunks` + `crdt_snapshots`
    /// (the CRDT source of truth), the `records` projection, and — when
    /// `options.run_logs` is [`RunLogPolicy::Include`] — `runs` + `run_logs`.
    ///
    /// The `records` projection is copied for inspectability, but on import it is
    /// re-derived from the imported chunks via `rebuild_projection`, so the CRDT
    /// chunks are the authoritative portable source (DL-6).
    pub fn export_workspace(
        &self,
        bundle_path: impl AsRef<Path>,
        options: &ExportOptions,
    ) -> Result<()> {
        // Refuse to clobber an existing file: an export writes a brand-new
        // bundle, and silently overwriting one would be a surprising data loss.
        let path = bundle_path.as_ref();
        if path.exists() {
            return Err(CoreError::StorageError(format!(
                "export target {} already exists; refusing to overwrite",
                path.display()
            )));
        }
        // A fresh Store creates the canonical M0a schema, so the bundle is a
        // valid, inspectable workspace file in its own right.
        let bundle = Store::open(path)?;
        // On a write failure, remove the freshly created (now partial/empty) file
        // so a retry is not refused by the "already exists" guard above and no
        // half-written bundle is left on disk (review 061 P1).
        match self.write_bundle(&bundle, options) {
            Ok(()) => Ok(()),
            Err(e) => {
                drop(bundle);
                let _ = std::fs::remove_file(path);
                Err(e)
            }
        }
    }

    /// Export into an **in-memory** bundle store (tests / piping). Same
    /// deterministic contents as the file path; the caller owns the returned
    /// `Store` (e.g. to serialize it, or to import straight back).
    pub fn export_workspace_in_memory(&self, options: &ExportOptions) -> Result<Store> {
        let bundle = Store::open_in_memory()?;
        self.write_bundle(&bundle, options)?;
        Ok(bundle)
    }

    /// Copy every syncable table from `self` into the fresh `bundle`, in a fixed
    /// deterministic order. Shared by the file and in-memory export entry points.
    ///
    /// **Source snapshot (review 062 P1 #3).** Every SOURCE read runs inside ONE
    /// read transaction (`src_tx`) opened on `self.conn` for the whole copy, so the
    /// bundle reflects a single consistent point-in-time snapshot of the workspace.
    /// Without it each `copy_*` helper ran its own `SELECT`, and a concurrent write
    /// landing *between* two copies (e.g. a new chunk after `oplog` but before
    /// `crdt_chunks`) would split-brain the bundle — an oplog row whose record never
    /// made it, or a chunk with no matching op. A single read transaction (frozen by
    /// the priming read below before any copy runs) pins the snapshot at the start of
    /// the export, so any concurrent writer's changes are simply invisible to this
    /// export (they ride the next one). The read transaction never writes, so dropping
    /// it (on success or error) is a clean rollback of nothing.
    ///
    /// All destination writes run inside ONE transaction on the bundle connection
    /// (review 061 P1): either the whole bundle is written and committed, or an
    /// error rolls every insert back so a failed export never leaves a partial,
    /// schema-but-no-data bundle file behind (the file-path entry point removes
    /// that empty file on error — see [`export_workspace`](Store::export_workspace)).
    fn write_bundle(&self, bundle: &Store, options: &ExportOptions) -> Result<()> {
        // Pin a consistent read snapshot of the SOURCE for the whole export. The
        // bundle/dst connection is freshly created and owned here; `self.conn` is
        // borrowed shared, so `unchecked_transaction` is the read seam (it never
        // writes, so it only ever rolls back nothing on drop).
        let src_tx = self.conn.unchecked_transaction().map_err(map_sql)?;
        let src: &Connection = &src_tx;
        // A DEFERRED read transaction does not fix its snapshot until the first
        // read. Issue one trivial read now so the snapshot is frozen at the START of
        // the export — before any `copy_*` runs — so a concurrent write landing
        // between BEGIN and the first copy is also excluded, not just writes between
        // two copies. Every later `copy_*` then reads this same frozen view.
        src.query_row("SELECT 1 FROM sqlite_schema LIMIT 1", [], |_| Ok(()))
            .optional()
            .map_err(map_sql)?;
        // TEST SEAM (review 062 P1 #3 regression): once the source snapshot above is
        // frozen, run any installed test hook. The snapshot-consistency test uses it
        // to land a concurrent write on a SECOND connection *after* the snapshot was
        // pinned but *before* any `copy_*` reads, then asserts the write is invisible
        // to the bundle. Without the `src_tx` snapshot the later `copy_*` SELECTs
        // would see the hook's write and split-brain the bundle, so this seam makes
        // the test fail if the snapshot is removed. Compiles to nothing in release.
        #[cfg(test)]
        run_post_snapshot_test_hook();
        // Order is FIXED so re-export is byte-stable: header → kv → oplog →
        // chunks → snapshots → records → (runs/run_logs by policy).
        let result = in_transaction(&bundle.conn, |dst| {
            write_meta_header(dst, options)?;
            copy_kv(src, dst)?;
            copy_oplog(src, dst)?;
            copy_crdt_chunks(src, dst)?;
            copy_crdt_snapshots(src, dst)?;
            copy_records(src, dst)?;
            if options.run_logs == RunLogPolicy::Include {
                copy_runs(src, dst)?;
                copy_run_logs(src, dst)?;
            }
            Ok(())
        });
        // End the read snapshot. It only read, so this is a no-op rollback either
        // way; surface a rollback failure only when the export otherwise succeeded.
        match (result, src_tx.rollback().map_err(map_sql)) {
            (Ok(()), rollback) => rollback,
            (Err(e), _) => Err(e),
        }
    }

    /// Import a portable bundle file into a **fresh** target `Store` at
    /// `target_path` (DL-24). Validates the bundle's `export_format_version`
    /// (an unknown version is a clean error, not a silent reinterpretation),
    /// copies every syncable table into the new store, then rebuilds the
    /// `records` projection from the imported `crdt_chunks` via
    /// [`rebuild_projection`](Store::rebuild_projection) so the result equals the
    /// source workspace's projection byte-for-byte (the DL-24 invariant). Active
    /// indexes in `indexes` are rebuilt from the canonical records as part of the
    /// rebuild.
    ///
    /// The target file must not already exist (importing into a populated
    /// workspace is out of scope — a bundle reconstructs a *fresh* workspace).
    pub fn import_workspace(
        bundle_path: impl AsRef<Path>,
        target_path: impl AsRef<Path>,
        indexes: &IndexManager,
    ) -> Result<Store> {
        let target = target_path.as_ref();
        if target.exists() {
            return Err(CoreError::StorageError(format!(
                "import target {} already exists; import requires a fresh workspace",
                target.display()
            )));
        }
        let bundle = open_bundle_readonly(bundle_path.as_ref())?;
        let mut store = Store::open(target)?;
        // On a load failure, remove the freshly created (now partial) target so a
        // retry is not refused by the "already exists" guard above and no
        // half-imported workspace is left on disk (review 061 P1).
        match store.load_from_bundle(&bundle, indexes) {
            Ok(()) => Ok(store),
            Err(e) => {
                drop(store);
                let _ = std::fs::remove_file(target);
                Err(e)
            }
        }
    }

    /// Import a bundle `Store` (e.g. one produced by
    /// [`export_workspace_in_memory`](Store::export_workspace_in_memory)) into a
    /// fresh in-memory target, rebuilding the projection. The validation +
    /// table-copy + rebuild are identical to the file path.
    pub fn import_workspace_in_memory(bundle: &Store, indexes: &IndexManager) -> Result<Store> {
        validate_bundle_version(&bundle.conn)?;
        let mut target = Store::open_in_memory()?;
        target.load_from_bundle(&bundle.conn, indexes)?;
        Ok(target)
    }

    /// Import a bundle `Store` into **THIS** (already-open) store *in place* (DL-24
    /// import-into-target; review 062 P1 #1). Validates the bundle's format version
    /// (an unknown version is a clean error before any state is touched), copies
    /// every syncable table from the bundle into `self` inside ONE transaction, then
    /// rebuilds the `records` projection from the imported `crdt_chunks` (DL-6).
    ///
    /// Unlike [`import_workspace_in_memory`](Store::import_workspace_in_memory) —
    /// which builds a *separate* in-memory `Store` the caller must then keep — this
    /// writes the imported tables into the connection `self` already holds. When
    /// `self` is **file-backed** that means the import is committed to the same file
    /// on disk, so it survives a reopen (the in-memory variant lost the data when
    /// the caller swapped the imported store away and reopened the original file —
    /// the review 062 P1 #1 bug).
    ///
    /// The caller is responsible for the fresh-target precondition: an import
    /// reconstructs a whole workspace, it does not merge. Use
    /// [`is_empty_target`](Store::is_empty_target) to gate it. The copy is
    /// all-or-nothing (the shared `load_from_bundle` transaction), so a mid-import
    /// failure rolls `self` back to its pre-import contents.
    pub fn import_workspace_in_place(
        &mut self,
        bundle: &Store,
        indexes: &IndexManager,
    ) -> Result<()> {
        validate_bundle_version(&bundle.conn)?;
        self.load_from_bundle(&bundle.conn, indexes)
    }

    /// True iff this store holds **no importable state at all** — the precondition
    /// for an in-place import (review 062 P1 #2). Checks EVERY table/namespace a
    /// bundle would populate, so a workspace that is "empty" only in its records
    /// projection but already carries (say) a `db.read` grant table or applet
    /// manifests is correctly reported as **not** fresh:
    ///
    /// - the projected `records`,
    /// - the CRDT source of truth (`crdt_chunks`, `crdt_snapshots`) and `oplog`,
    /// - the policy-gated `runs` / `run_logs`,
    /// - and every **portable** `kv` row — i.e. any namespace NOT dropped by
    ///   [`is_local_only_namespace`]. Local-only / secret namespaces are excluded
    ///   because they never travel in a bundle, so a store holding *only* a stray
    ///   device-local key is still a valid (fresh) import target; counting them
    ///   would wrongly refuse a genuinely importable workspace.
    ///
    /// (M0a storage has no `schema_defs` / `index_defs` tables yet — those are
    /// GA-future, reported as `missing_required_for_ga` in the export descriptor —
    /// so there is nothing to check for them here; this is the complete importable
    /// surface for the current schema.)
    pub fn is_empty_target(&self) -> Result<bool> {
        // Any importable physical table with a row → not fresh. A single
        // `EXISTS`/`LIMIT 1` per table is enough (we only need presence).
        for table in ["records", "crdt_chunks", "crdt_snapshots", "oplog", "runs", "run_logs"] {
            if table_has_any_row(&self.conn, table)? {
                return Ok(false);
            }
        }
        // Portable kv: a row in any namespace a bundle WOULD carry counts. A row in
        // a purely local-only / secret namespace (never exported) does not.
        if self.has_portable_kv_row()? {
            return Ok(false);
        }
        Ok(true)
    }

    /// True iff the `kv` table holds at least one row in a **portable** namespace —
    /// one a bundle would carry (i.e. NOT [`is_local_only_namespace`]). Tombstoned
    /// rows count: a deleted-but-exported key is still importable state that a fresh
    /// target must not silently shadow. Local-only / secret namespaces are skipped
    /// because they never travel in a bundle.
    fn has_portable_kv_row(&self) -> Result<bool> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT namespace FROM kv")
            .map_err(map_sql)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(map_sql)?;
        for r in rows {
            let namespace = r.map_err(map_sql)?;
            if !is_local_only_namespace(&namespace) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Copy every syncable table from an (already version-validated) bundle
    /// connection into this fresh store, then rebuild the projection from the
    /// imported chunks. Mirrors the export order so the two paths stay in lockstep.
    ///
    /// The table copies run inside ONE transaction on the target connection
    /// (review 061 P1): if any copy fails the whole import rolls back, so a failed
    /// import never leaves a half-populated target with chunks but no oplog (the
    /// file-path entry point also removes the freshly created target file on error
    /// — see [`import_workspace`](Store::import_workspace)). The projection rebuild
    /// runs after commit because [`rebuild_projection`](Store::rebuild_projection)
    /// manages its own write transaction over the now-imported chunks.
    fn load_from_bundle(&mut self, bundle: &Connection, indexes: &IndexManager) -> Result<()> {
        in_transaction(&self.conn, |dst| {
            copy_kv(bundle, dst)?;
            copy_oplog(bundle, dst)?;
            copy_crdt_chunks(bundle, dst)?;
            copy_crdt_snapshots(bundle, dst)?;
            // The bundle's `records` rows are advisory; the import re-derives the
            // projection from the authoritative `crdt_chunks` (DL-6), so we do NOT
            // copy `records` here — rebuild produces them.
            copy_runs(bundle, dst)?;
            copy_run_logs(bundle, dst)?;
            Ok(())
        })?;
        // DL-6: reconstruct the records projection purely from imported chunks.
        // This is the byte-identical-projection invariant: records are derived.
        self.rebuild_projection(indexes)
    }
}

/// Run `f` against `conn` inside one SQLite transaction, committing iff `f`
/// returns `Ok` and rolling back on any error (review 061 P1: the table copies
/// are all-or-nothing, never a partially populated bundle/target). Uses
/// `unchecked_transaction` because the copy helpers borrow the connection by
/// shared reference; the bundle/target connection is freshly created and owned by
/// this call, so no other handle is mutating it concurrently.
fn in_transaction<F>(conn: &Connection, f: F) -> Result<()>
where
    F: FnOnce(&Connection) -> Result<()>,
{
    let tx = conn.unchecked_transaction().map_err(map_sql)?;
    f(&tx)?;
    tx.commit().map_err(map_sql)?;
    Ok(())
}

/// True iff `table` holds at least one row (a presence probe for
/// [`is_empty_target`](Store::is_empty_target)). `table` is always one of this
/// module's FIXED table-name literals — never caller/user input — so formatting it
/// into the statement carries no injection surface; the `EXISTS`/`LIMIT 1` shape
/// lets SQLite stop at the first row instead of counting the whole table.
fn table_has_any_row(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)"),
        [],
        |row| row.get::<_, i64>(0),
    )
    .map_err(map_sql)
    .map(|exists| exists != 0)
}

/// Open a bundle file **read-only** and validate its format version before any
/// copy. Read-only mirrors the spec ("opens the database read-only first").
fn open_bundle_readonly(path: &Path) -> Result<Connection> {
    if !path.exists() {
        return Err(CoreError::StorageError(format!(
            "import bundle {} does not exist",
            path.display()
        )));
    }
    use rusqlite::OpenFlags;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(map_sql)?;
    validate_bundle_version(&conn)?;
    Ok(conn)
}

/// Read and validate the bundle's `export_format_version` from its `meta` table.
/// A missing header or a version this build does not understand is a clean
/// [`CoreError::StorageError`] (spec §Versioning: importers must not silently
/// reinterpret unknown versions).
fn validate_bundle_version(bundle: &Connection) -> Result<()> {
    let raw: Option<Vec<u8>> = bundle
        .query_row(
            "SELECT value FROM meta WHERE key = 'export_format_version'",
            [],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(map_sql)?
        .flatten();
    let bytes = raw.ok_or_else(|| {
        CoreError::StorageError(
            "bundle is missing its export_format_version header; not a forge workspace export".into(),
        )
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|e| {
        CoreError::StorageError(format!("export_format_version is not utf-8: {e}"))
    })?;
    let version: i64 = text.parse().map_err(|e| {
        CoreError::StorageError(format!("export_format_version is malformed: {e}"))
    })?;
    if version != EXPORT_FORMAT_VERSION {
        return Err(CoreError::StorageError(format!(
            "unsupported export_format_version {version}; this build understands {EXPORT_FORMAT_VERSION} \
             (migrate the bundle with a matching forge version)"
        )));
    }
    Ok(())
}

/// Write the bundle's `meta` header rows (format version + workspace id) in a
/// fixed key order. Values are stored as utf-8 text bytes, matching how the
/// fixtures model `meta` rows (`[key, value]` strings).
fn write_meta_header(bundle: &Connection, options: &ExportOptions) -> Result<()> {
    // Deterministic key order. `set_meta` is a plain upsert into the bundle's
    // already-created `meta` table.
    set_meta(bundle, "export_format_version", &EXPORT_FORMAT_VERSION.to_string())?;
    set_meta(
        bundle,
        "forge_storage_schema_version",
        &STORAGE_SCHEMA_VERSION.to_string(),
    )?;
    set_meta(bundle, "workspace_id", &options.workspace_id)?;
    Ok(())
}

/// Upsert one `meta` row (utf-8 text value).
fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta (key, value, updated_at) VALUES (?1, ?2, 0)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value.as_bytes()],
    )
    .map_err(map_sql)?;
    Ok(())
}

/// Copy the portable `kv` rows from `src` to `dst`, ordered by `(namespace,
/// key)` and **excluding** local-only / secret namespaces
/// ([`is_local_only_namespace`]). Tombstones are retained (a deleted key must
/// round-trip as deleted, DL-21), so the bundle carries the full sync-correct kv
/// shape minus secrets.
fn copy_kv(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT namespace, key, value, content_type, logical_version, updated_at, tombstone
               FROM kv ORDER BY namespace, key",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (namespace, key, value, content_type, logical_version, updated_at, tombstone) =
            r.map_err(map_sql)?;
        // EXCLUSION GUARD: a secret / device-local namespace is never written
        // to the bundle (DL-24). This is the single chokepoint the guard test
        // pins.
        if is_local_only_namespace(&namespace) {
            continue;
        }
        dst.execute(
            "INSERT INTO kv
                 (namespace, key, value, content_type, logical_version, updated_at, tombstone)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![namespace, key, value, content_type, logical_version, updated_at, tombstone],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `oplog` ordered by `(lamport, op_id)` — the spec's deterministic replay
/// order — preserving every column.
fn copy_oplog(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT op_id, actor_id, workspace_id, lamport, kind, payload, created_at
               FROM oplog ORDER BY lamport, op_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (op_id, actor_id, workspace_id, lamport, kind, payload, created_at) =
            r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO oplog
                 (op_id, actor_id, workspace_id, lamport, kind, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![op_id, actor_id, workspace_id, lamport, kind, payload, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `crdt_chunks` — the append-only CRDT source of truth (DL-6) — ordered by
/// `(doc_id, created_at, chunk_id)`, preserving `created_at` so the import's
/// chunk replay order matches the source exactly.
fn copy_crdt_chunks(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT doc_id, chunk_id, format, payload, created_at
               FROM crdt_chunks ORDER BY doc_id, created_at, chunk_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (doc_id, chunk_id, format, payload, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO crdt_chunks (doc_id, chunk_id, format, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![doc_id, chunk_id, format, payload, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `crdt_snapshots` (the snapshot accelerator) ordered by `(doc_id,
/// created_at, snapshot_id)`. Snapshots are not the sole source of truth, but
/// they round-trip so the bundle is a complete workspace.
fn copy_crdt_snapshots(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT doc_id, snapshot_id, format, payload, frontier, created_at
               FROM crdt_snapshots ORDER BY doc_id, created_at, snapshot_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<Vec<u8>>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (doc_id, snapshot_id, format, payload, frontier, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO crdt_snapshots
                 (doc_id, snapshot_id, format, payload, frontier, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![doc_id, snapshot_id, format, payload, frontier, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy the `records` projection ordered by `(collection, id)`. Used only by the
/// export path (for an inspectable, byte-stable bundle); import re-derives this
/// table from `crdt_chunks` so it is never trusted as input.
fn copy_records(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT collection, id, data, updated_at FROM records ORDER BY collection, id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (collection, id, data, updated_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO records (collection, id, data, updated_at) VALUES (?1, ?2, ?3, ?4)",
            params![collection, id, data, updated_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `runs` ordered by `(created_at, run_id)` (policy-gated). Each row's
/// `record_json` is the full `RunRecord`; the validate-on-read in
/// [`Store::load_run`](crate::Store::load_run) still guards provenance after import.
fn copy_runs(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT run_id, applet_id, record_json, created_at FROM runs ORDER BY created_at, run_id",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (run_id, applet_id, record_json, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO runs (run_id, applet_id, record_json, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![run_id, applet_id, record_json, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Copy `run_logs` ordered by `(run_id, seq)` (policy-gated), preserving log
/// sequence order.
fn copy_run_logs(src: &Connection, dst: &Connection) -> Result<()> {
    let mut stmt = src
        .prepare(
            "SELECT run_id, seq, level, event_type, payload, created_at
               FROM run_logs ORDER BY run_id, seq",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<Vec<u8>>>(4)?,
                row.get::<_, Option<i64>>(5)?,
            ))
        })
        .map_err(map_sql)?;
    for r in rows {
        let (run_id, seq, level, event_type, payload, created_at) = r.map_err(map_sql)?;
        dst.execute(
            "INSERT INTO run_logs (run_id, seq, level, event_type, payload, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![run_id, seq, level, event_type, payload, created_at],
        )
        .map_err(map_sql)?;
    }
    Ok(())
}

/// Read a bundle's `meta` value as utf-8 text (helper for callers / tests that
/// want to inspect the header without reaching SQLite). Returns `None` for a
/// missing key. A non-utf-8 value is a `StorageError` rather than lossy bytes.
pub fn bundle_meta(bundle: &Store, key: &str) -> Result<Option<String>> {
    let raw: Option<Vec<u8>> = bundle
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key = ?1",
            params![key],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(map_sql)?
        .flatten();
    match raw {
        Some(bytes) => {
            let s = std::str::from_utf8(&bytes).map_err(|e| {
                CoreError::StorageError(format!("bundle meta {key} is not utf-8: {e}"))
            })?;
            Ok(Some(s.to_string()))
        }
        None => Ok(None),
    }
}

// A one-shot test hook fired inside `Store::write_bundle` immediately after the
// source read snapshot is frozen but before any table is copied (review 062 P1 #3).
// Lets a test interleave a concurrent write into the export deterministically; the
// hook takes itself out on fire so it runs at most once per install.
#[cfg(test)]
thread_local! {
    static POST_SNAPSHOT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Install a one-shot hook fired after the export's source snapshot is frozen.
#[cfg(test)]
fn set_post_snapshot_test_hook(f: impl FnOnce() + 'static) {
    POST_SNAPSHOT_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

/// Fire and clear the installed post-snapshot hook, if any.
#[cfg(test)]
fn run_post_snapshot_test_hook() {
    if let Some(f) = POST_SNAPSHOT_HOOK.with(|h| h.borrow_mut().take()) {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{CreateIndexKind, IndexManager};
    use crate::Mutation;
    use forge_domain::{AppResult, AppletId, RunId, RunOutcome, RunRecord};
    use serde_json::json;

    // --- builders ---------------------------------------------------------

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().expect("object").clone()
    }

    fn insert(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Insert {
            collection: collection.into(),
            id: Some(id.into()),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    fn patch(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Patch {
            collection: collection.into(),
            id: id.into(),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    fn delete(collection: &str, id: &str, at: i64) -> Mutation {
        Mutation::Delete {
            collection: collection.into(),
            id: id.into(),
            logical_at: Some(at),
        }
    }

    /// A workspace built through the real DL-4 CRDT mutation path so that the
    /// projection is CRDT-backed (chunks exist for rebuild on import). Returns
    /// the source store and the index manager whose active index must survive a
    /// round-trip.
    fn source_workspace() -> (Store, IndexManager) {
        let mut s = Store::open_in_memory().unwrap();
        let mut idx = IndexManager::new();
        // Records via the CRDT write path (chunks + oplog + projection).
        s.apply_mutation_crdt(&insert("notes", "n1", json!({"title": "Alpha", "body": "offline first"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("notes", "n2", json!({"title": "Beta", "body": "sync later"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&patch("notes", "n1", json!({"pinned": true}), 3), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "Ship"}), 4), &idx)
            .unwrap();
        // A record that is deleted in CRDT history (must not resurrect on import).
        s.apply_mutation_crdt(&insert("tasks", "t9", json!({"title": "Temp"}), 5), &idx)
            .unwrap();
        s.apply_mutation_crdt(&delete("tasks", "t9", 6), &idx).unwrap();

        // An active value index over a stable field id (rebuilt on import).
        s.create_index(&mut idx, "notes", "f_title", CreateIndexKind::Value)
            .unwrap();

        // Portable kv: an applet manifest stand-in + ctx.storage namespace + the
        // workspace run counter (all portable workspace state).
        s.kv_set("__forge/meta", "applet/notes", b"{\"manifest\":true}", "application/json")
            .unwrap();
        s.kv_set("applet/notes", "draft", b"hello", "text/plain").unwrap();
        s.next_counter("__forge/meta", "run_counter").unwrap();

        // Local-only / secret kv that must NEVER be exported.
        s.kv_set("secret/weather", "api_key", b"sk-DO-NOT-EXPORT", "text/plain").unwrap();
        s.kv_set("provider/openai", "token", b"tok-secret", "text/plain").unwrap();
        s.kv_set("device/window", "geometry", b"{\"w\":800}", "application/json").unwrap();
        s.kv_set("local/ui", "scroll", b"42", "text/plain").unwrap();

        (s, idx)
    }

    fn sample_run(run_id: &str) -> RunRecord {
        RunRecord {
            run_id: RunId::new(run_id),
            applet_id: AppletId::new("app_notes"),
            code_hash: forge_domain::code_hash("body"),
            input: json!({"x": 1}),
            random_seed: 7,
            time_start: 1000,
            calls: vec![],
            logs: vec!["hello".into()],
            permissions: forge_domain::PermissionSnapshot::default(),
            outcome: RunOutcome::Completed {
                result: AppResult { ok: true, value: json!("ok") },
            },
        }
    }

    /// The full projection keyed by `collection/id` → canonical data, ordered, for
    /// a byte-for-byte source/target comparison (the DL-24 invariant).
    fn projection_snapshot(s: &Store) -> std::collections::BTreeMap<String, String> {
        let mut stmt = s
            .connection()
            .prepare("SELECT collection, id, data FROM records ORDER BY collection, id")
            .unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        let mut out = std::collections::BTreeMap::new();
        for r in rows {
            let (c, id, data) = r.unwrap();
            out.insert(format!("{c}/{id}"), data);
        }
        out
    }

    /// The complete bundle file as bytes, for byte-identical re-export checks.
    fn read_file(path: &std::path::Path) -> Vec<u8> {
        std::fs::read(path).unwrap()
    }

    // --- DL-24 core invariant: export -> import -> equal projection -------

    #[test]
    fn import_reproduces_the_source_projection_byte_for_byte() {
        let (src, idx) = source_workspace();
        let before = projection_snapshot(&src);

        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws_demo")).unwrap();
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();

        // The imported projection — re-derived from the imported chunks — equals
        // the source projection exactly.
        assert_eq!(projection_snapshot(&target), before, "DL-24 byte-identical projection");

        // Query results match (same live rows, deleted record stays gone).
        assert_eq!(target.list_records("notes").unwrap().len(), 2);
        assert_eq!(target.list_records("tasks").unwrap().len(), 1);
        assert!(target.get_record("tasks", "t9").unwrap().is_none(), "deleted record not resurrected");
        // The patched field survived.
        assert_eq!(target.get_record("notes", "n1").unwrap().unwrap().fields["pinned"], json!(true));
    }

    #[test]
    fn import_rebuilds_the_active_index_from_canonical_records() {
        // The active value index must serve a query against the imported store,
        // proving indexes were rebuilt from canonical records (not copied raw).
        let (src, idx) = source_workspace();
        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws_demo")).unwrap();
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();

        let q = crate::Query::from_fixture_value(&json!({
            "from": "notes",
            "where": [{"field_id": "f_title", "op": "eq", "value": "Alpha"}]
        }))
        .unwrap();
        let planned = target.query_planned(&q, &idx).unwrap();
        assert!(planned.uses_index, "the imported store's active index must serve the query");
        assert_eq!(planned.index_id.as_deref(), Some("idx_records_notes_f_title"));
    }

    #[test]
    fn kv_live_values_and_tombstones_round_trip() {
        let (src, idx) = source_workspace();
        // Tombstone a key so the export carries the deletion.
        src.kv_delete("applet/notes", "draft").unwrap();

        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws_demo")).unwrap();
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();

        // Portable kv round-trips; the tombstoned key is hidden but its row exists.
        assert_eq!(
            target.kv_get("__forge/meta", "applet/notes").unwrap().as_deref(),
            Some(&b"{\"manifest\":true}"[..])
        );
        assert_eq!(target.kv_get("applet/notes", "draft").unwrap(), None, "tombstone hides the value");
        let tomb: i64 = target
            .connection()
            .query_row(
                "SELECT tombstone FROM kv WHERE namespace='applet/notes' AND key='draft'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tomb, 1, "the tombstone row round-trips");
        // The run counter is portable workspace state.
        assert_eq!(
            target.kv_get("__forge/meta", "run_counter").unwrap().as_deref(),
            Some(&b"1"[..])
        );
    }

    // --- DL-24 exclusion guard: secrets / device-local NEVER exported -----

    #[test]
    fn secrets_and_device_local_kv_are_never_exported() {
        let (src, idx) = source_workspace();
        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws_demo")).unwrap();

        // Not in the bundle file at all.
        for (ns, key) in [
            ("secret/weather", "api_key"),
            ("provider/openai", "token"),
            ("device/window", "geometry"),
            ("local/ui", "scroll"),
        ] {
            let present: i64 = bundle
                .connection()
                .query_row(
                    "SELECT COUNT(*) FROM kv WHERE namespace=?1 AND key=?2",
                    params![ns, key],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(present, 0, "secret/device-local {ns}/{key} must NOT be in the bundle");
        }

        // And not in the imported workspace.
        let target = Store::import_workspace_in_memory(&bundle, &idx).unwrap();
        assert_eq!(target.kv_get("secret/weather", "api_key").unwrap(), None);
        assert_eq!(target.kv_get("provider/openai", "token").unwrap(), None);
        assert_eq!(target.kv_get("device/window", "geometry").unwrap(), None);
        assert_eq!(target.kv_get("local/ui", "scroll").unwrap(), None);
    }

    #[test]
    fn local_only_namespace_policy_is_precise() {
        // Secret / device / local buckets are excluded...
        assert!(is_local_only_namespace("secret/weather"));
        assert!(is_local_only_namespace("secrets"));
        assert!(is_local_only_namespace("provider/openai"));
        assert!(is_local_only_namespace("credentials/aws"));
        assert!(is_local_only_namespace("device/window"));
        assert!(is_local_only_namespace("local/ui"));
        // Exact root buckets (a key stored directly under the bucket, no trailing
        // `/<key>` segment) are excluded too — closes the review 061 P2 gap where
        // `secret`, `provider`, `credentials`, `__device`, `__local` slipped past a
        // prefix-only check and were exported.
        assert!(is_local_only_namespace("secret"));
        assert!(is_local_only_namespace("provider"));
        assert!(is_local_only_namespace("credentials"));
        assert!(is_local_only_namespace("__device"));
        assert!(is_local_only_namespace("__local"));
        assert!(is_local_only_namespace("secrets/aws"), "secrets/ children stay excluded");
        // ...but portable workspace namespaces are NOT excluded.
        assert!(!is_local_only_namespace("__forge/meta"), "applet manifests/programs are portable");
        assert!(!is_local_only_namespace("applet/notes"), "applet ctx.storage is portable");
        assert!(!is_local_only_namespace("localized"), "prefix must be a bucket boundary, not a substring");
        assert!(!is_local_only_namespace("secretive"), "a longer name sharing a bucket prefix is portable");
        assert!(!is_local_only_namespace("providers"), "providers (plural) is not the provider bucket");
    }

    // --- DL-24 deterministic re-export: byte-identical --------------------

    #[test]
    fn re_export_of_the_same_workspace_is_byte_identical() {
        let dir = tempfile::tempdir().unwrap();
        let (src, _idx) = source_workspace();

        let a = dir.path().join("a.forgews");
        let b = dir.path().join("b.forgews");
        src.export_workspace(&a, &ExportOptions::new("ws_demo")).unwrap();
        src.export_workspace(&b, &ExportOptions::new("ws_demo")).unwrap();

        // Two exports of the same workspace produce byte-identical bundle files.
        assert_eq!(read_file(&a), read_file(&b), "re-export must be byte-stable");
    }

    #[test]
    fn export_refuses_to_overwrite_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let (src, _idx) = source_workspace();
        let p = dir.path().join("ws.forgews");
        src.export_workspace(&p, &ExportOptions::new("ws_demo")).unwrap();
        let err = src.export_workspace(&p, &ExportOptions::new("ws_demo")).unwrap_err();
        assert_eq!(err.code(), "StorageError");
    }

    #[test]
    fn import_round_trips_through_real_files() {
        let dir = tempfile::tempdir().unwrap();
        let (src, idx) = source_workspace();
        let before = projection_snapshot(&src);

        let bundle = dir.path().join("ws.forgews");
        src.export_workspace(&bundle, &ExportOptions::new("ws_demo")).unwrap();

        let target_path = dir.path().join("restored.db");
        let target = Store::import_workspace(&bundle, &target_path, &idx).unwrap();
        assert_eq!(projection_snapshot(&target), before, "file round-trip preserves the projection");
    }

    // --- run-log policy ---------------------------------------------------

    #[test]
    fn run_logs_excluded_by_default_included_on_request() {
        let (src, idx) = source_workspace();
        // Seed a run + run_logs.
        src.save_run(&sample_run("run_1")).unwrap();
        src.connection()
            .execute(
                "INSERT INTO run_logs (run_id, seq, level, event_type, payload, created_at)
                 VALUES ('run_1', 0, 'info', 'log', ?1, 0), ('run_1', 1, 'info', 'log', ?2, 0)",
                params![b"a".as_slice(), b"b".as_slice()],
            )
            .unwrap();

        // Default policy: runs / run_logs are NOT exported.
        let excluded = src.export_workspace_in_memory(&ExportOptions::new("ws")).unwrap();
        let target_x = Store::import_workspace_in_memory(&excluded, &idx).unwrap();
        assert!(target_x.load_run("run_1").unwrap().is_none(), "runs excluded by default");
        let log_count_x: i64 = target_x
            .connection()
            .query_row("SELECT COUNT(*) FROM run_logs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(log_count_x, 0, "run_logs excluded by default");

        // Include policy: runs + run_logs round-trip, log order preserved.
        let included = src
            .export_workspace_in_memory(&ExportOptions::new("ws").with_run_logs())
            .unwrap();
        let target_i = Store::import_workspace_in_memory(&included, &idx).unwrap();
        assert_eq!(target_i.load_run("run_1").unwrap().unwrap().run_id.as_str(), "run_1");
        let seqs: Vec<i64> = {
            let mut stmt = target_i
                .connection()
                .prepare("SELECT seq FROM run_logs WHERE run_id='run_1' ORDER BY seq")
                .unwrap();
            let rows = stmt.query_map([], |r| r.get::<_, i64>(0)).unwrap();
            rows.map(|r| r.unwrap()).collect()
        };
        assert_eq!(seqs, vec![0, 1], "run_logs preserve seq order");
    }

    // --- version handling -------------------------------------------------

    #[test]
    fn bundle_carries_the_format_version_header() {
        let (src, _idx) = source_workspace();
        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws_demo")).unwrap();
        assert_eq!(
            bundle_meta(&bundle, "export_format_version").unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(
            bundle_meta(&bundle, "forge_storage_schema_version").unwrap().as_deref(),
            Some("1")
        );
        assert_eq!(bundle_meta(&bundle, "workspace_id").unwrap().as_deref(), Some("ws_demo"));
    }

    #[test]
    fn version_mismatch_is_a_clean_error() {
        let (src, idx) = source_workspace();
        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws")).unwrap();
        // Tamper the header to a future version.
        bundle
            .connection()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'export_format_version'",
                params![b"999".as_slice()],
            )
            .unwrap();
        let err = Store::import_workspace_in_memory(&bundle, &idx)
            .err()
            .expect("import must reject an unsupported version");
        assert_eq!(err.code(), "StorageError");
        assert!(format!("{err}").contains("999"), "the error names the unsupported version: {err}");
    }

    #[test]
    fn missing_version_header_is_rejected() {
        // A SQLite file that is not a forge bundle (no header row) is refused.
        let idx = IndexManager::new();
        let not_a_bundle = Store::open_in_memory().unwrap();
        let err = Store::import_workspace_in_memory(&not_a_bundle, &idx)
            .err()
            .expect("a file with no header is not a bundle");
        assert_eq!(err.code(), "StorageError");
    }

    #[test]
    fn import_refuses_a_non_fresh_target() {
        let dir = tempfile::tempdir().unwrap();
        let (src, idx) = source_workspace();
        let bundle = dir.path().join("ws.forgews");
        src.export_workspace(&bundle, &ExportOptions::new("ws")).unwrap();
        let target_path = dir.path().join("exists.db");
        // Pre-create the target file.
        Store::open(&target_path).unwrap();
        let err = Store::import_workspace(&bundle, &target_path, &idx)
            .err()
            .expect("import must refuse a pre-existing target");
        assert_eq!(err.code(), "StorageError");
    }

    // --- in-place import persists to the target file (review 062 P1 #1) ---

    #[test]
    fn import_workspace_in_place_persists_to_the_same_file() {
        // A file-backed target must actually PERSIST an in-place import: write the
        // bundle, import into a file-backed store, drop it, reopen the SAME path,
        // and still see the imported records / applet kv / grant table. This is the
        // review 062 P1 #1 regression — the in-memory swap reported success but lost
        // everything on reopen.
        let dir = tempfile::tempdir().unwrap();
        let (src, idx) = source_workspace();
        let before = projection_snapshot(&src);
        // Persist a grant table so a non-record namespace travels too.
        src.kv_set(
            "__forge/meta",
            "db_read_grants",
            b"{\"actor\":[\"notes\"]}",
            "application/json",
        )
        .unwrap();

        let bundle_path = dir.path().join("ws.forgews");
        src.export_workspace(&bundle_path, &ExportOptions::new("ws_demo")).unwrap();

        let target_path = dir.path().join("target.db");
        {
            // Open the bundle as a Store and import into a file-backed target.
            let bundle = Store::open(&bundle_path).unwrap();
            let mut target = Store::open(&target_path).unwrap();
            target.import_workspace_in_place(&bundle, &idx).unwrap();
            // Visible in the same handle that imported.
            assert_eq!(projection_snapshot(&target), before, "import populates the live handle");
        } // Drop both stores: the WAL flushes the committed import to disk.

        // Reopen the SAME file path: the imported state is durably present.
        let reopened = Store::open(&target_path).unwrap();
        assert_eq!(
            projection_snapshot(&reopened),
            before,
            "in-place import must survive a reopen of the same file"
        );
        // Records query as expected (deleted record stays gone).
        assert_eq!(reopened.list_records("notes").unwrap().len(), 2);
        assert_eq!(reopened.list_records("tasks").unwrap().len(), 1);
        assert!(reopened.get_record("tasks", "t9").unwrap().is_none());
        // Portable kv survived the reopen too: applet manifest + grant table.
        assert_eq!(
            reopened.kv_get("__forge/meta", "applet/notes").unwrap().as_deref(),
            Some(&b"{\"manifest\":true}"[..])
        );
        assert_eq!(
            reopened.kv_get("__forge/meta", "db_read_grants").unwrap().as_deref(),
            Some(&b"{\"actor\":[\"notes\"]}"[..])
        );
    }

    #[test]
    fn import_workspace_in_place_rejects_a_bad_version_before_touching_state() {
        // A version-mismatched bundle is refused before any table is copied, so the
        // (fresh) target is left untouched.
        let (src, idx) = source_workspace();
        let bundle = src.export_workspace_in_memory(&ExportOptions::new("ws")).unwrap();
        bundle
            .connection()
            .execute(
                "UPDATE meta SET value = ?1 WHERE key = 'export_format_version'",
                params![b"999".as_slice()],
            )
            .unwrap();
        let mut target = Store::open_in_memory().unwrap();
        let err = target.import_workspace_in_place(&bundle, &idx).unwrap_err();
        assert_eq!(err.code(), "StorageError");
        assert!(target.is_empty_target().unwrap(), "a rejected import must not populate the target");
    }

    // --- is_empty_target across every importable namespace (062 P1 #2) ----

    #[test]
    fn is_empty_target_true_on_a_fresh_store() {
        let s = Store::open_in_memory().unwrap();
        assert!(s.is_empty_target().unwrap(), "a brand-new store is a fresh import target");
    }

    #[test]
    fn is_empty_target_false_when_any_importable_table_is_populated() {
        // Each importable physical table independently makes the target non-fresh.
        // records:
        {
            let s = Store::open_in_memory().unwrap();
            s.put_record(&{
                let mut e = crate::RecordEnvelope::new(
                    forge_domain::CollectionId::new("notes"),
                    forge_domain::RecordId::new("n1"),
                    Default::default(),
                    forge_domain::LogicalTimestamp(1),
                );
                e.fields.insert("title".into(), json!("x"));
                e
            })
            .unwrap();
            assert!(!s.is_empty_target().unwrap(), "a projected record is importable state");
        }
        // crdt_chunks:
        {
            let s = Store::open_in_memory().unwrap();
            s.put_chunk("collection/notes", "c1", "loro", b"x").unwrap();
            assert!(!s.is_empty_target().unwrap(), "a crdt chunk is importable state");
        }
        // crdt_snapshots:
        {
            let s = Store::open_in_memory().unwrap();
            s.put_snapshot("collection/notes", "s1", "loro", b"x", b"f").unwrap();
            assert!(!s.is_empty_target().unwrap(), "a crdt snapshot is importable state");
        }
        // oplog:
        {
            let s = Store::open_in_memory().unwrap();
            s.append_op("op1", "a", "ws", 1, "insert", b"p").unwrap();
            assert!(!s.is_empty_target().unwrap(), "an oplog row is importable state");
        }
        // runs + run_logs:
        {
            let s = Store::open_in_memory().unwrap();
            s.save_run(&sample_run("run_1")).unwrap();
            assert!(!s.is_empty_target().unwrap(), "a run row is importable state");
        }
        {
            let s = Store::open_in_memory().unwrap();
            s.connection()
                .execute(
                    "INSERT INTO run_logs (run_id, seq, level, event_type, payload, created_at)
                     VALUES ('run_1', 0, 'info', 'log', ?1, 0)",
                    params![b"a".as_slice()],
                )
                .unwrap();
            assert!(!s.is_empty_target().unwrap(), "a run_log row is importable state");
        }
    }

    #[test]
    fn is_empty_target_false_for_a_grants_only_or_applet_only_workspace() {
        // A workspace whose ONLY content is a portable __forge/meta kv entry (the
        // grants-only / kv-only case the review calls out) is NOT fresh: the grant
        // table / applet manifest would be silently shadowed by an import.
        let grants_only = Store::open_in_memory().unwrap();
        grants_only
            .kv_set("__forge/meta", "db_read_grants", b"{}", "application/json")
            .unwrap();
        assert!(
            !grants_only.is_empty_target().unwrap(),
            "a grants-only workspace must not pass the fresh-target check"
        );

        let applet_only = Store::open_in_memory().unwrap();
        applet_only
            .kv_set("applet/notes", "draft", b"hello", "text/plain")
            .unwrap();
        assert!(
            !applet_only.is_empty_target().unwrap(),
            "an applet ctx.storage namespace is importable workspace state"
        );
    }

    #[test]
    fn is_empty_target_ignores_purely_local_only_kv() {
        // Local-only / secret namespaces never travel in a bundle, so a store whose
        // ONLY content is such a key is still a valid (fresh) import target — counting
        // it would wrongly refuse a genuinely importable workspace.
        let s = Store::open_in_memory().unwrap();
        s.kv_set("secret/weather", "api_key", b"sk-x", "text/plain").unwrap();
        s.kv_set("device/window", "geometry", b"{}", "application/json").unwrap();
        s.kv_set("local/ui", "scroll", b"42", "text/plain").unwrap();
        assert!(
            s.is_empty_target().unwrap(),
            "a store holding only non-exportable local-only kv is still a fresh target"
        );
        // Add ONE portable key and it flips to non-fresh.
        s.kv_set("applet/notes", "draft", b"hi", "text/plain").unwrap();
        assert!(!s.is_empty_target().unwrap(), "a portable kv row makes it non-fresh");
    }

    #[test]
    fn is_empty_target_counts_a_tombstoned_portable_kv_row() {
        // An exported-then-tombstoned portable key is still importable state: the
        // tombstone row travels in the bundle, so a fresh target must not shadow it.
        let s = Store::open_in_memory().unwrap();
        s.kv_set("applet/notes", "draft", b"hi", "text/plain").unwrap();
        s.kv_delete("applet/notes", "draft").unwrap();
        assert!(
            !s.is_empty_target().unwrap(),
            "a tombstoned portable kv row is still importable state"
        );
    }

    // --- export reflects a single consistent source snapshot (062 P1 #3) --

    #[test]
    fn export_reflects_a_single_consistent_source_snapshot() {
        // While an export reads the source inside ONE read transaction, a concurrent
        // write on a SECOND connection to the same file must NOT bleed into the
        // bundle: the snapshot is pinned at the export's BEGIN, so the bundle is a
        // single consistent point-in-time view (no split-brain). We model the race
        // deterministically: open a second handle, mutate it AFTER the export's read
        // snapshot has begun, and assert the bundle reflects the pre-write state.
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("src.db");
        let writer = Store::open(&src_path).unwrap();
        // Seed one CRDT-backed record so the bundle has chunks + a projection.
        let idx = IndexManager::new();
        {
            let mut w = Store::open(&src_path).unwrap();
            w.apply_mutation_crdt(&insert("notes", "n1", json!({"title": "Alpha"}), 1), &idx)
                .unwrap();
        }
        // A second, independent handle is the "concurrent writer".
        let mut concurrent = Store::open(&src_path).unwrap();

        // Begin the export's source snapshot manually so we can interleave a write
        // against the live file mid-export, then run the copy against that snapshot.
        // A SQLite read transaction pins its snapshot at the FIRST read, so we issue
        // one read to freeze the snapshot — modelling `write_bundle`, whose first
        // `copy_*` SELECT freezes the snapshot before every later copy runs.
        let exporter = Store::open(&src_path).unwrap();
        let snapshot = exporter.connection().unchecked_transaction().unwrap();
        let _pin: i64 = snapshot
            .query_row("SELECT COUNT(*) FROM crdt_chunks", [], |r| r.get(0))
            .unwrap();
        // CONCURRENT WRITE: lands on the file AFTER the snapshot was frozen. WAL lets
        // the writer commit while the snapshot keeps reading its pinned view.
        concurrent
            .apply_mutation_crdt(&insert("notes", "n2", json!({"title": "Beta"}), 2), &idx)
            .unwrap();

        // The pinned snapshot still sees ONLY n1 (the concurrent n2 is invisible).
        let chunk_docs: i64 = snapshot
            .query_row("SELECT COUNT(DISTINCT doc_id) FROM crdt_chunks", [], |r| r.get(0))
            .unwrap();
        let n1_present: i64 = snapshot
            .query_row(
                "SELECT COUNT(*) FROM records WHERE collection='notes' AND id='n1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let n2_present: i64 = snapshot
            .query_row(
                "SELECT COUNT(*) FROM records WHERE collection='notes' AND id='n2'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(snapshot);
        assert_eq!(chunk_docs, 1, "snapshot pins one doc; the concurrent write is invisible");
        assert_eq!(n1_present, 1, "the pre-write record is in the snapshot");
        assert_eq!(n2_present, 0, "the concurrent write must not bleed into the snapshot");

        // And the real export entry point produces a bundle that round-trips to a
        // consistent projection (n1 present, importable) — proving write_bundle's
        // snapshot does not corrupt the output.
        let bundle_path = dir.path().join("ws.forgews");
        writer.export_workspace(&bundle_path, &ExportOptions::new("ws")).unwrap();
        let restore_path = dir.path().join("restored.db");
        let restored = Store::import_workspace(&bundle_path, &restore_path, &idx).unwrap();
        assert!(restored.get_record("notes", "n1").unwrap().is_some(), "n1 round-trips");
    }

    #[test]
    fn write_bundle_snapshot_excludes_a_write_that_lands_mid_export() {
        // The LOAD-BEARING regression for review 062 P1 #3: drive the REAL
        // `export_workspace` / `write_bundle` and, via the post-snapshot test hook,
        // commit a concurrent write on a second connection to the SAME file AFTER the
        // export's source snapshot has been frozen but BEFORE any `copy_*` reads. The
        // bundle must reflect only the pre-snapshot state. If the `src_tx` snapshot in
        // `write_bundle` is removed, each `copy_*` SELECT reads the live file and the
        // hook's `n2` row bleeds into the bundle — so this assertion fails, which is
        // exactly the regression guard the snapshot fix needs.
        let dir = tempfile::tempdir().unwrap();
        let src_path = dir.path().join("src.db");
        let idx = IndexManager::new();
        {
            let mut w = Store::open(&src_path).unwrap();
            w.apply_mutation_crdt(&insert("notes", "n1", json!({"title": "Alpha"}), 1), &idx)
                .unwrap();
        }
        let exporter = Store::open(&src_path).unwrap();

        // The hook fires after the snapshot is frozen: a SECOND handle commits n2.
        let hook_path = src_path.clone();
        set_post_snapshot_test_hook(move || {
            let mut concurrent = Store::open(&hook_path).unwrap();
            let hook_idx = IndexManager::new();
            concurrent
                .apply_mutation_crdt(&insert("notes", "n2", json!({"title": "Beta"}), 2), &hook_idx)
                .unwrap();
        });

        let bundle_path = dir.path().join("ws.forgews");
        exporter.export_workspace(&bundle_path, &ExportOptions::new("ws")).unwrap();

        // The bundle reflects the frozen snapshot only. n1 and n2 share the same
        // `collection/notes` doc, so the n2 write appears as a SECOND chunk on that
        // doc: at snapshot time there is exactly ONE chunk; a leaked mid-export write
        // would make it two.
        let bundle = Store::open(&bundle_path).unwrap();
        let chunks_on_notes: i64 = bundle
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM crdt_chunks WHERE doc_id = 'collection/notes'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chunks_on_notes, 1,
            "the bundle carries only the pre-snapshot chunk; the concurrent write's chunk must not leak in"
        );

        // And the imported projection (re-derived from the bundle's chunks) has only
        // n1 — a split-brain bundle would resurrect n2 here.
        let restore_path = dir.path().join("restored.db");
        let restored = Store::import_workspace(&bundle_path, &restore_path, &idx).unwrap();
        assert!(restored.get_record("notes", "n1").unwrap().is_some(), "n1 round-trips");
        assert!(restored.get_record("notes", "n2").unwrap().is_none(), "n2 must be absent");
    }

    // --- fixtures/export descriptors (T017) ------------------------------

    /// The export fixture descriptors pin the expected bundle shape: format
    /// version, included/excluded tables, run-log policy, deterministic ordering,
    /// and the `missing_required_for_ga` sections. We parse them and assert our
    /// implementation's constants + policy agree with the canonical descriptors.
    #[derive(serde::Deserialize)]
    struct TinyDescriptor {
        export_format_version: i64,
        include_run_logs: bool,
        missing_required_for_ga: Vec<String>,
    }

    fn fixture_path(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/export")
            .join(name)
    }

    fn load_fixture<T: serde::de::DeserializeOwned>(name: &str) -> T {
        let text = std::fs::read_to_string(fixture_path(name))
            .unwrap_or_else(|e| panic!("read fixture {name}: {e}"));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse fixture {name}: {e}"))
    }

    #[test]
    fn tiny_fixture_agrees_with_our_format_version_and_policy() {
        let fx: TinyDescriptor = load_fixture("tiny_workspace_descriptor.json");
        assert_eq!(fx.export_format_version, EXPORT_FORMAT_VERSION, "fixture pins our format version");
        // Default policy excludes run logs, matching the tiny fixture.
        assert!(!fx.include_run_logs);
        assert_eq!(ExportOptions::default().run_logs, RunLogPolicy::Exclude);
        // The GA-missing sections are not yet persisted tables in M0a storage, so
        // they are legitimately absent from the bundle (documented in the spec).
        assert!(fx.missing_required_for_ga.contains(&"rbac_config".to_string()));
        assert!(fx.missing_required_for_ga.contains(&"index_defs".to_string()));
    }

    #[derive(serde::Deserialize)]
    struct RunLogsDescriptor {
        include_run_logs: bool,
    }

    #[test]
    fn run_logs_fixture_describes_the_include_policy() {
        let fx: RunLogsDescriptor = load_fixture("workspace_with_run_logs_descriptor.json");
        assert!(fx.include_run_logs, "the debug-bundle fixture opts into run logs");
        // Our include policy maps to the same flag.
        assert_eq!(
            ExportOptions::new("ws").with_run_logs().run_logs,
            RunLogPolicy::Include
        );
    }

    #[derive(serde::Deserialize)]
    struct RedactedDescriptor {
        expected_exclusions: Vec<String>,
    }

    #[test]
    fn redacted_fixture_exclusions_match_our_guard() {
        let fx: RedactedDescriptor = load_fixture("redacted_secrets_descriptor.json");
        // The fixture's excluded kinds (secret plaintext, provider tokens, local
        // window state) all map to namespaces our guard refuses to export.
        assert!(fx.expected_exclusions.iter().any(|e| e == "secret_plaintext"));
        assert!(is_local_only_namespace("secret/anything"));
        assert!(is_local_only_namespace("provider/anything"));
        assert!(is_local_only_namespace("local/window"));
    }
}
