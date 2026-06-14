//! The export/import orchestrators: the `impl Store` entry points that drive the
//! deterministic table copy in a FIXED order inside one transaction, plus the
//! frozen-source-snapshot machinery (DL-24 byte-identical re-export; review 062
//! P1 #3) and its post-snapshot test hook.

use super::policy::{ExportOptions, RunLogPolicy};
use super::table_copy::{
    copy_crdt_chunks, copy_crdt_snapshots, copy_kv, copy_oplog, copy_records, copy_run_logs,
    copy_runs, write_meta_header,
};
use super::transaction::in_transaction;
use super::version::{open_bundle_readonly, validate_bundle_version};
use crate::index::IndexManager;
use crate::{map_sql, Store};
use forge_domain::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

impl Store {
    /// Export this workspace's syncable state into a brand-new portable SQLite
    /// **bundle file** at `bundle_path` (DL-24). The bundle is a self-describing
    /// `Store` written table-by-table in a fully deterministic order, so two
    /// exports of the same workspace produce byte-identical files.
    ///
    /// Included: `meta` (with the format-version header), portable `kv`
    /// (secrets / device-local namespaces excluded — see
    /// [`is_local_only_namespace`](super::is_local_only_namespace)), `oplog`,
    /// `crdt_chunks` + `crdt_snapshots` (the CRDT source of truth), the `records`
    /// projection, and — when `options.run_logs` is [`RunLogPolicy::Include`] —
    /// `runs` + `run_logs`.
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
pub(super) fn set_post_snapshot_test_hook(f: impl FnOnce() + 'static) {
    POST_SNAPSHOT_HOOK.with(|h| *h.borrow_mut() = Some(Box::new(f)));
}

/// Fire and clear the installed post-snapshot hook, if any.
#[cfg(test)]
fn run_post_snapshot_test_hook() {
    if let Some(f) = POST_SNAPSHOT_HOOK.with(|h| h.borrow_mut().take()) {
        f();
    }
}
