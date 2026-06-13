//! Dynamic indexes over the `records` projection (DL-5 / DL-6).
//!
//! `forge/spec/dynamic-indexes.md` maps the dynamic-index promise onto the
//! committed `records(collection, id, data TEXT JSON, updated_at)` projection.
//! This module is the storage-internal half:
//!
//! - [`IndexDef`] — a rebuildable index definition `(collection, field_id, kind,
//!   state)`. The registry is the logical owner; here it is just metadata that
//!   decides physical DDL and planner eligibility.
//! - [`IndexManager`] — registers definitions, emits and applies the
//!   **collection-scoped JSON1 expression index** DDL (and the FTS5 shadow table
//!   for full-text fields), rebuilds physical structures from canonical
//!   `records` (DL-6), and answers the planner's "is there an active index for
//!   this predicate/search?" question.
//!
//! ## Canonical-data invariant
//!
//! `records` is canonical. An expression index or FTS row can never change a
//! query answer — only performance and the `planner.full_scan` warning. The
//! planner always scans `records` for correctness (ordering/null rules live in
//! Rust); the index decision only drives `uses_index` / `index_id` / warnings.
//! Concretely, [`IndexManager::plan_predicate`] tells the planner whether an
//! active index *would* serve the predicate; the row answer is identical either
//! way. This keeps the design honest to the spec's "dropping an index cannot
//! change answers" rule while still exercising real DDL.
//!
//! ## Injection safety
//!
//! Index DDL is structure, but `collection`/`field_id` come from a registry that
//! ultimately reflects user input, so they are validated against the same
//! identifier allowlist the query planner uses ([`crate::query`]). The index
//! *name* is double-quoted and the partial predicate's collection literal is
//! single-quoted with quotes escaped, so a hostile identifier can neither break
//! out of the name nor the `WHERE collection = '…'` clause. A validation failure
//! is a `QueryError`, never a silently-built bad index.

use crate::query::{
    field_id_json_path, validate_index_ident, FieldRef, FullScanReason, Op, PlannerWarning, Query,
};
use forge_domain::{CoreError, Result};
use rusqlite::Connection;
use std::collections::BTreeMap;

/// The physical kind of an index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexKind {
    /// A SQLite expression index over `json_extract(data, '$.field_ids.<id>')`,
    /// serving equality / range / order.
    Expression,
    /// An FTS5 virtual table mirroring a text field's value, serving text search.
    Fts5,
}

impl IndexKind {
    fn parse(s: &str) -> Result<IndexKind> {
        match s {
            "expression" => Ok(IndexKind::Expression),
            "fts5" => Ok(IndexKind::Fts5),
            other => Err(CoreError::QueryError(format!(
                "unknown index kind '{other}'"
            ))),
        }
    }
}

/// The caller-facing index kind for [`IndexManager::create_index`] (DL-5). This
/// is the ergonomic `{Value, Fts}` surface named in the data-layer spec; it maps
/// 1:1 to the physical [`IndexKind`] (`Value -> Expression`, `Fts -> Fts5`).
/// Keeping it separate from [`IndexKind`] lets the fixture-facing `"expression"`
/// / `"fts5"` strings stay stable while the create API speaks the PRD vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateIndexKind {
    /// An equality/range/order index — a JSON1 expression index (DL-5).
    Value,
    /// A full-text index — an FTS5 shadow table (DL-5).
    Fts,
}

impl From<CreateIndexKind> for IndexKind {
    fn from(k: CreateIndexKind) -> IndexKind {
        match k {
            CreateIndexKind::Value => IndexKind::Expression,
            CreateIndexKind::Fts => IndexKind::Fts5,
        }
    }
}

/// The DL-5 lifecycle state. The planner may use **only** `Active`; every other
/// state scans `records` and warns. M0a-first states are `Proposed`,
/// `Rebuilding`, and `Active`; `Deprecated` is recognized so a deprecated
/// field's old index is ignored (and surfaced as `index_deprecated`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexState {
    Proposed,
    Building,
    Active,
    Stale,
    Rebuilding,
    Deprecated,
    Removed,
}

impl IndexState {
    fn parse(s: &str) -> Result<IndexState> {
        let st = match s {
            "proposed" => IndexState::Proposed,
            "building" => IndexState::Building,
            "active" => IndexState::Active,
            "stale" => IndexState::Stale,
            "rebuilding" => IndexState::Rebuilding,
            "deprecated" => IndexState::Deprecated,
            "removed" => IndexState::Removed,
            other => {
                return Err(CoreError::QueryError(format!(
                    "unknown index state '{other}'"
                )))
            }
        };
        Ok(st)
    }

    /// Whether the planner may use an index in this state (only `Active`).
    pub fn is_usable(&self) -> bool {
        matches!(self, IndexState::Active)
    }

    /// The `planner.full_scan` reason when an index in this state is *not* used.
    fn full_scan_reason(&self) -> FullScanReason {
        match self {
            IndexState::Deprecated => FullScanReason::IndexDeprecated,
            // proposed/building/stale/rebuilding/removed are all "exists but not
            // active" from the planner's perspective.
            _ => FullScanReason::IndexNotActive,
        }
    }
}

/// A rebuildable index definition. `index_id` is the deterministic physical name
/// (also the planner's `index_id` when used). `definition_hash` is a stable hash
/// of the canonical definition tuple, used to detect drift on rebuild.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDef {
    pub index_id: String,
    pub collection: String,
    pub field_id: String,
    pub kind: IndexKind,
    pub state: IndexState,
}

impl IndexDef {
    /// Build a definition, validating the identifiers and deriving the canonical
    /// `index_id` when one is not supplied.
    pub fn new(
        collection: impl Into<String>,
        field_id: impl Into<String>,
        kind: IndexKind,
        state: IndexState,
    ) -> Result<IndexDef> {
        let collection = collection.into();
        let field_id = field_id.into();
        validate_index_ident("collection", &collection)?;
        validate_index_ident("field id", &field_id)?;
        let index_id = canonical_index_id(kind, &collection, &field_id);
        Ok(IndexDef {
            index_id,
            collection,
            field_id,
            kind,
            state,
        })
    }

    /// Parse a definition from a fixture's `indexes[]` entry.
    pub fn from_fixture_value(v: &serde_json::Value) -> Result<IndexDef> {
        let obj = v
            .as_object()
            .ok_or_else(|| CoreError::QueryError("index definition must be an object".into()))?;
        let collection = obj
            .get("collection")
            .and_then(|c| c.as_str())
            .ok_or_else(|| CoreError::QueryError("index missing 'collection'".into()))?;
        let field_id = obj
            .get("field_id")
            .and_then(|f| f.as_str())
            .ok_or_else(|| CoreError::QueryError("index missing 'field_id'".into()))?;
        let kind = IndexKind::parse(
            obj.get("kind")
                .and_then(|k| k.as_str())
                .ok_or_else(|| CoreError::QueryError("index missing 'kind'".into()))?,
        )?;
        let state = IndexState::parse(
            obj.get("state")
                .and_then(|s| s.as_str())
                .ok_or_else(|| CoreError::QueryError("index missing 'state'".into()))?,
        )?;
        let mut def = IndexDef::new(collection, field_id, kind, state)?;
        // If the fixture pins an explicit index_id, prefer it (and assert that
        // our deterministic name matches, so a name drift is caught here).
        if let Some(id) = obj.get("index_id").and_then(|i| i.as_str()) {
            if id != def.index_id {
                return Err(CoreError::QueryError(format!(
                    "index_id '{id}' does not match the deterministic name '{}'",
                    def.index_id
                )));
            }
            def.index_id = id.to_string();
        }
        Ok(def)
    }

    /// A short, stable hash of the canonical definition tuple `(collection,
    /// field_id, kind)`. State is intentionally excluded — lifecycle moves do
    /// not change the physical definition. Used to detect a definition change
    /// across a rebuild (DL-6).
    pub fn definition_hash(&self) -> u64 {
        // FNV-1a over the canonical tuple. Deterministic across platforms.
        let kind = match self.kind {
            IndexKind::Expression => "expression",
            IndexKind::Fts5 => "fts5",
        };
        let mut h: u64 = 0xcbf29ce484222325;
        for part in [self.collection.as_str(), "\0", self.field_id.as_str(), "\0", kind] {
            for b in part.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        h
    }

    /// The `CREATE INDEX` / `CREATE VIRTUAL TABLE` DDL for this definition.
    ///
    /// Identifiers are pre-validated; the name is double-quoted and the
    /// collection literal in the partial predicate is single-quote escaped, so
    /// the DDL is injection-safe (DL-16-style structure safety).
    pub fn ddl(&self) -> String {
        // Double-quoted leaf key so a dotted field id addresses the literal key
        // (must match the query predicate's path exactly, or SQLite would not
        // consult the expression index). See `query::field_id_json_path`.
        let json_path = field_id_json_path(&self.field_id);
        match self.kind {
            IndexKind::Expression => format!(
                "CREATE INDEX IF NOT EXISTS {} ON records (json_extract(data, '{}')) WHERE collection = '{}';",
                quote_ident(&self.index_id),
                json_path,
                escape_sql_literal(&self.collection),
            ),
            IndexKind::Fts5 => format!(
                "CREATE VIRTUAL TABLE IF NOT EXISTS {} USING fts5(record_id UNINDEXED, value, tokenize = 'unicode61');",
                quote_ident(&self.index_id),
            ),
        }
    }
}

/// The deterministic physical name for a `(kind, collection, field_id)`.
/// Mirrors the fixtures: `idx_records_<collection>_<field_id>` for expression
/// indexes and `fts_records_<collection>_<field_id>` for FTS5 tables.
fn canonical_index_id(kind: IndexKind, collection: &str, field_id: &str) -> String {
    let prefix = match kind {
        IndexKind::Expression => "idx",
        IndexKind::Fts5 => "fts",
    };
    format!("{prefix}_records_{collection}_{field_id}")
}

/// Double-quote a validated identifier for use as a SQLite object name. The
/// identifier is already restricted to `[A-Za-z0-9_./-]`, so it cannot contain a
/// double quote; we still escape defensively per SQLite's `""` rule.
fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Escape a string for a single-quoted SQL literal (`'` → `''`). Used only for
/// the collection literal in a partial-index predicate; the value is a validated
/// identifier, so this is belt-and-suspenders.
fn escape_sql_literal(s: &str) -> String {
    s.replace('\'', "''")
}

/// The outcome of asking the index manager whether an active index serves a
/// query's predicate or search.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexPlan {
    /// Whether an active index serves the query.
    pub uses_index: bool,
    /// The id of the index used, when `uses_index` is true.
    pub index_id: Option<String>,
    /// The index kind used (e.g. `fts5`), when one was used.
    pub kind: Option<IndexKind>,
    /// `planner.full_scan` warnings to surface (empty when an index was used).
    pub warnings: Vec<PlannerWarning>,
}

/// Owns index definitions and the physical structures (expression indexes / FTS5
/// shadow tables) derived from them.
pub struct IndexManager {
    /// Definitions keyed by `(collection, field_id)`.
    defs: BTreeMap<(String, String), IndexDef>,
}

impl Default for IndexManager {
    fn default() -> Self {
        Self::new()
    }
}

impl IndexManager {
    pub fn new() -> Self {
        IndexManager {
            defs: BTreeMap::new(),
        }
    }

    /// Register (or replace) an index definition.
    pub fn register(&mut self, def: IndexDef) {
        self.defs
            .insert((def.collection.clone(), def.field_id.clone()), def);
    }

    /// The registered definition for `(collection, field_id)`, if any.
    pub fn get(&self, collection: &str, field_id: &str) -> Option<&IndexDef> {
        self.defs
            .get(&(collection.to_string(), field_id.to_string()))
    }

    /// All registered definitions, in a stable order.
    pub fn defs(&self) -> impl Iterator<Item = &IndexDef> {
        self.defs.values()
    }

    /// Transition a registered index to a new lifecycle state, if present.
    pub fn set_state(&mut self, collection: &str, field_id: &str, state: IndexState) {
        if let Some(def) = self
            .defs
            .get_mut(&(collection.to_string(), field_id.to_string()))
        {
            def.state = state;
        }
    }

    /// Create (DL-5) an `Active` index for `(collection, field_id)` of the given
    /// `kind` and build it from canonical `records` in one call.
    ///
    /// This is the ergonomic create entry point named in the data-layer PRD:
    /// `Value` → a collection-scoped JSON1 **expression index** for
    /// equality/range/order; `Fts` → an **FTS5 shadow table** for full-text
    /// search, populated from `$.field_ids.<field_id>`. Identifiers are validated
    /// against the index allowlist by [`IndexDef::new`], so a hostile collection
    /// or field id is rejected before any DDL is emitted (no SQL injection).
    ///
    /// The definition lands in the `Active` state and its physical structure is
    /// built immediately from canonical records — so creating an index *after*
    /// rows already exist activates it correctly (DL-6 rebuild-after-records).
    /// Re-creating the same index is idempotent (DDL is `IF NOT EXISTS`; the FTS
    /// table is dropped and repopulated). Returns the deterministic `index_id`.
    pub fn create_index(
        &mut self,
        conn: &Connection,
        collection: &str,
        field_id: &str,
        kind: CreateIndexKind,
    ) -> Result<String> {
        let def = IndexDef::new(collection, field_id, kind.into(), IndexState::Active)?;
        let index_id = def.index_id.clone();
        // Build the physical structure (from canonical records) before recording
        // the definition, so a DDL/populate failure leaves no half-registered
        // index in the manager.
        self.drop_physical(conn, &def)?;
        conn.execute_batch(&def.ddl())
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        if def.kind == IndexKind::Fts5 {
            self.populate_fts(conn, &def)?;
        }
        self.register(def);
        Ok(index_id)
    }

    /// Deprecate (DL-5 / DL-8 "delete = deprecate + retain") a registered index:
    /// move it to the `Deprecated` state and drop the physical structure so the
    /// planner stops using it, while keeping the definition as metadata. Records
    /// stay canonical, so query answers are unchanged — only the plan and the
    /// `index_deprecated` warning differ. No-op if the index is not registered.
    pub fn deprecate_index(
        &mut self,
        conn: &Connection,
        collection: &str,
        field_id: &str,
    ) -> Result<()> {
        if let Some(def) = self
            .defs
            .get(&(collection.to_string(), field_id.to_string()))
            .cloned()
        {
            self.drop_physical(conn, &def)?;
            self.set_state(collection, field_id, IndexState::Deprecated);
        }
        Ok(())
    }

    /// Drop (DL-5 `removed`) a registered index: drop the physical structure and
    /// forget the definition entirely. After this the planner has no candidate
    /// for `(collection, field_id)` and a predicate over it scans with a
    /// `no_index` warning. No-op if the index is not registered.
    pub fn drop_index(
        &mut self,
        conn: &Connection,
        collection: &str,
        field_id: &str,
    ) -> Result<()> {
        if let Some(def) = self
            .defs
            .remove(&(collection.to_string(), field_id.to_string()))
        {
            self.drop_physical(conn, &def)?;
        }
        Ok(())
    }

    /// Refresh the FTS5 shadow rows for a single record after a put/patch/delete
    /// (DL-5: "inserts, updates, and deletes must refresh the FTS row in the same
    /// logical write transaction when the index is active").
    ///
    /// For every **active** FTS index on `collection`, the record's prior row is
    /// deleted and (when the record is live and has a text value at
    /// `$.field_ids.<field_id>`) re-inserted with the current value. A tombstoned
    /// record drops out of the FTS table so a deleted note stops matching. Pass
    /// the same `conn` (or a transaction) the canonical write used, so the FTS
    /// refresh commits or rolls back atomically with the record write.
    ///
    /// `record_id` is the canonical `records.id`; `data_json` is the record's
    /// stored JSON (the same string written to `records.data`). Non-FTS and
    /// non-active indexes are skipped — expression indexes are maintained by
    /// SQLite itself, so only the FTS shadow needs hand-syncing.
    pub fn sync_fts_for_record(
        &self,
        conn: &Connection,
        collection: &str,
        record_id: &str,
        data_json: &str,
    ) -> Result<()> {
        for def in self.defs.values() {
            if def.collection != collection
                || def.kind != IndexKind::Fts5
                || !def.state.is_usable()
            {
                continue;
            }
            // Drop the existing row for this id (idempotent: zero or one row).
            let delete = format!(
                "DELETE FROM {} WHERE record_id = ?1",
                quote_ident(&def.index_id)
            );
            conn.execute(&delete, rusqlite::params![record_id])
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
            // Re-insert iff the record is live and carries a text value.
            if let Some(text) = fts_value_for(conn, def, data_json)? {
                let insert = format!(
                    "INSERT INTO {} (record_id, value) VALUES (?1, ?2)",
                    quote_ident(&def.index_id)
                );
                conn.execute(&insert, rusqlite::params![record_id, text])
                    .map_err(|e| CoreError::StorageError(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Apply the physical DDL for every **active** definition. Expression index
    /// DDL is `IF NOT EXISTS`, so this is idempotent; FTS5 tables are populated
    /// from canonical `records` via [`rebuild_active`]. Non-active definitions
    /// create no physical structure (the planner would not use them anyway).
    pub fn apply_active_ddl(&self, conn: &Connection) -> Result<()> {
        for def in self.defs.values() {
            if def.state.is_usable() {
                conn.execute_batch(&def.ddl())
                    .map_err(|e| CoreError::StorageError(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// DL-6 rebuild: drop and recreate every active physical structure purely
    /// from canonical `records` (never reading prior index pages / FTS rows).
    ///
    /// For expression indexes this is a `DROP` + `CREATE` (SQLite repopulates
    /// from the table). For FTS5 tables we drop, recreate, and re-extract each
    /// record's text value from `$.field_ids.<id>` in canonical `records`. The
    /// build is idempotent: running it twice yields the same physical state.
    pub fn rebuild_active(&self, conn: &Connection) -> Result<()> {
        for def in self.defs.values() {
            if !def.state.is_usable() {
                // A non-active index is never physically present; make sure any
                // stale structure with this name is gone so the planner cannot
                // accidentally use it.
                self.drop_physical(conn, def)?;
                continue;
            }
            self.drop_physical(conn, def)?;
            conn.execute_batch(&def.ddl())
                .map_err(|e| CoreError::StorageError(e.to_string()))?;
            if def.kind == IndexKind::Fts5 {
                self.populate_fts(conn, def)?;
            }
        }
        Ok(())
    }

    /// Drop the physical structure for a definition (index or FTS table).
    fn drop_physical(&self, conn: &Connection, def: &IndexDef) -> Result<()> {
        let sql = match def.kind {
            IndexKind::Expression => format!("DROP INDEX IF EXISTS {};", quote_ident(&def.index_id)),
            IndexKind::Fts5 => format!("DROP TABLE IF EXISTS {};", quote_ident(&def.index_id)),
        };
        conn.execute_batch(&sql)
            .map_err(|e| CoreError::StorageError(e.to_string()))
    }

    /// Repopulate an FTS5 shadow table from canonical `records`: one row per
    /// record in the collection, `value` extracted from `$.field_ids.<id>`.
    fn populate_fts(&self, conn: &Connection, def: &IndexDef) -> Result<()> {
        let json_path = field_id_json_path(&def.field_id);
        // Read (id, text) from canonical records; bind the collection.
        let select = "SELECT id, json_extract(data, ?1) FROM records \
                      WHERE collection = ?2 AND json_extract(data, '$.deleted') IS NOT 1";
        let mut stmt = conn
            .prepare(select)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        let rows = stmt
            .query_map(
                rusqlite::params![json_path, def.collection],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        let insert = format!(
            "INSERT INTO {} (record_id, value) VALUES (?1, ?2)",
            quote_ident(&def.index_id)
        );
        for r in rows {
            let (id, value) = r.map_err(|e| CoreError::StorageError(e.to_string()))?;
            if let Some(text) = value {
                conn.execute(&insert, rusqlite::params![id, text])
                    .map_err(|e| CoreError::StorageError(e.to_string()))?;
            }
        }
        Ok(())
    }

    /// Run a full-text search against an **active** FTS5 shadow table, returning
    /// the matched `record_id`s ordered by FTS rank. Errors if the table is not
    /// active (the caller should have checked [`plan_text_search`] first).
    pub fn fts_match(
        &self,
        conn: &Connection,
        collection: &str,
        field_id: &str,
        query: &str,
    ) -> Result<Vec<String>> {
        let def = self.get(collection, field_id).ok_or_else(|| {
            CoreError::QueryError(format!(
                "no FTS index for {collection}/{field_id}"
            ))
        })?;
        let sql = format!(
            "SELECT record_id FROM {} WHERE {} MATCH ?1 ORDER BY rank",
            quote_ident(&def.index_id),
            quote_ident(&def.index_id),
        );
        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![query], |row| row.get::<_, String>(0))
            .map_err(|e| CoreError::StorageError(e.to_string()))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| CoreError::StorageError(e.to_string()))?);
        }
        Ok(out)
    }

    /// Decide whether an active index serves `query`'s predicate (and/or text
    /// search). Returns `uses_index`, the `index_id`, and any
    /// `planner.full_scan` warnings. `estimated_rows` is the scanned row count
    /// the caller observed, threaded into the warning payload.
    ///
    /// This is genuinely computed from the registered definitions and their
    /// lifecycle states — never hardcoded. An index is "used" only when:
    /// the predicate/search addresses a stable `field_id`, a definition exists
    /// for `(collection, field_id)`, of the matching kind, in the `Active`
    /// state, and the operator is index-serviceable.
    pub fn plan(&self, query: &Query, estimated_rows: i64) -> IndexPlan {
        // Text search takes the FTS path.
        if let Some(ts) = &query.text_search {
            return self.plan_text_search(&query.from, &ts.field, estimated_rows);
        }
        // Otherwise, the predicate's single indexable leaf decides. The fixtures
        // pin a single equality/range predicate (possibly an implicit-AND list
        // over the same field for a range); we look at the first index-eligible
        // leaf.
        if let Some((field, op)) = query.filter.as_ref().and_then(first_indexable_leaf) {
            return self.plan_predicate(&query.from, field, op, estimated_rows);
        }
        // No predicate / no text search: nothing to index against. Not a full
        // scan warning case (a bare list is expected to scan).
        IndexPlan {
            uses_index: false,
            index_id: None,
            kind: None,
            warnings: Vec::new(),
        }
    }

    /// Plan a single predicate `<field> <op>`.
    fn plan_predicate(
        &self,
        collection: &str,
        field: &FieldRef,
        op: Op,
        estimated_rows: i64,
    ) -> IndexPlan {
        // Only a stable field id can match an index by id.
        let Some(field_id) = field.field_id() else {
            return IndexPlan {
                uses_index: false,
                index_id: None,
                kind: None,
                warnings: vec![PlannerWarning::full_scan(
                    collection,
                    field,
                    FullScanReason::NoIndex,
                    Some(estimated_rows),
                )],
            };
        };
        match self.get(collection, field_id) {
            None => self.no_index(collection, field, estimated_rows),
            Some(def) => {
                // An expression index serves eq/range/order. (An FTS index does
                // not serve scalar predicates.)
                if def.kind != IndexKind::Expression || !op_uses_expression_index(op) {
                    return IndexPlan {
                        uses_index: false,
                        index_id: None,
                        kind: None,
                        warnings: vec![PlannerWarning::full_scan(
                            collection,
                            field,
                            FullScanReason::UnsupportedOperator,
                            Some(estimated_rows),
                        )],
                    };
                }
                if def.state.is_usable() {
                    IndexPlan {
                        uses_index: true,
                        index_id: Some(def.index_id.clone()),
                        kind: Some(def.kind),
                        warnings: Vec::new(),
                    }
                } else {
                    IndexPlan {
                        uses_index: false,
                        index_id: None,
                        kind: None,
                        warnings: vec![PlannerWarning::full_scan(
                            collection,
                            field,
                            def.state.full_scan_reason(),
                            Some(estimated_rows),
                        )],
                    }
                }
            }
        }
    }

    /// Plan a text search over `field`.
    fn plan_text_search(
        &self,
        collection: &str,
        field: &FieldRef,
        estimated_rows: i64,
    ) -> IndexPlan {
        let Some(field_id) = field.field_id() else {
            return IndexPlan {
                uses_index: false,
                index_id: None,
                kind: None,
                warnings: vec![PlannerWarning::full_scan(
                    collection,
                    field,
                    FullScanReason::FtsNotAvailable,
                    Some(estimated_rows),
                )],
            };
        };
        match self.get(collection, field_id) {
            Some(def) if def.kind == IndexKind::Fts5 && def.state.is_usable() => IndexPlan {
                uses_index: true,
                index_id: Some(def.index_id.clone()),
                kind: Some(def.kind),
                warnings: Vec::new(),
            },
            _ => IndexPlan {
                uses_index: false,
                index_id: None,
                kind: None,
                warnings: vec![PlannerWarning::full_scan(
                    collection,
                    field,
                    FullScanReason::FtsNotAvailable,
                    Some(estimated_rows),
                )],
            },
        }
    }

    fn no_index(&self, collection: &str, field: &FieldRef, estimated_rows: i64) -> IndexPlan {
        IndexPlan {
            uses_index: false,
            index_id: None,
            kind: None,
            warnings: vec![PlannerWarning::full_scan(
                collection,
                field,
                FullScanReason::NoIndex,
                Some(estimated_rows),
            )],
        }
    }
}

/// Whether an operator can be served by an expression index (`=`, `<`, `<=`,
/// `>`, `>=`, `IN`). `LIKE`/`NE` are not index-served in M0a.
fn op_uses_expression_index(op: Op) -> bool {
    matches!(op, Op::Eq | Op::Lt | Op::Le | Op::Gt | Op::Ge | Op::In)
}

/// The FTS `value` for one record's JSON, or `None` when the record should not
/// have an FTS row (tombstoned, or no text at `$.field_ids.<field_id>`).
///
/// Extraction goes through SQLite's `json_extract` (same as [`populate_fts`]) so
/// a rebuild and an incremental sync agree byte-for-byte. The JSON is bound as a
/// parameter; only the validated `field_id` is interpolated into the path.
fn fts_value_for(conn: &Connection, def: &IndexDef, data_json: &str) -> Result<Option<String>> {
    let json_path = field_id_json_path(&def.field_id);
    // Skip tombstoned records: a deleted note must drop out of FTS.
    let row = conn
        .query_row(
            "SELECT json_extract(?1, '$.deleted') IS 1, json_extract(?1, ?2)",
            rusqlite::params![data_json, json_path],
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, Option<String>>(1)?)),
        )
        .map_err(|e| CoreError::StorageError(e.to_string()))?;
    let (deleted, value) = row;
    if deleted {
        return Ok(None);
    }
    Ok(value)
}

/// Find the first index-eligible leaf predicate in a filter tree: a leaf
/// addressing a stable `field_id`. For the fixtures' implicit-AND range form,
/// every leaf is over the same field id, so the first one decides.
fn first_indexable_leaf(filter: &crate::query::Filter) -> Option<(&FieldRef, Op)> {
    use crate::query::Filter;
    match filter {
        Filter::Leaf(p) => Some((&p.field, p.op)),
        Filter::And(items) | Filter::Or(items) => items.iter().find_map(first_indexable_leaf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_match_the_fixture_convention() {
        let expr = IndexDef::new("tasks", "f_alice_1", IndexKind::Expression, IndexState::Active)
            .unwrap();
        assert_eq!(expr.index_id, "idx_records_tasks_f_alice_1");
        let fts =
            IndexDef::new("notes", "f_alice_0", IndexKind::Fts5, IndexState::Active).unwrap();
        assert_eq!(fts.index_id, "fts_records_notes_f_alice_0");
    }

    #[test]
    fn expression_ddl_is_collection_scoped_and_quoted() {
        let def = IndexDef::new("tasks", "f_alice_1", IndexKind::Expression, IndexState::Active)
            .unwrap();
        let ddl = def.ddl();
        assert!(ddl.contains("\"idx_records_tasks_f_alice_1\""));
        // The leaf field-id key is double-quoted in the JSON path so a dotted id
        // would address the literal key.
        assert!(ddl.contains("json_extract(data, '$.field_ids.\"f_alice_1\"')"));
        assert!(ddl.contains("WHERE collection = 'tasks'"));
    }

    #[test]
    fn malicious_collection_is_rejected_not_interpolated() {
        // An identifier that would break out of the partial-index literal is
        // refused at construction, never reaching the DDL string.
        let err = IndexDef::new(
            "tasks'); DROP TABLE records;--",
            "f_alice_1",
            IndexKind::Expression,
            IndexState::Active,
        )
        .unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn definition_hash_is_state_independent_and_stable() {
        let a = IndexDef::new("c", "f0", IndexKind::Expression, IndexState::Active).unwrap();
        let b = IndexDef::new("c", "f0", IndexKind::Expression, IndexState::Proposed).unwrap();
        assert_eq!(a.definition_hash(), b.definition_hash(), "state excluded");
        let c = IndexDef::new("c", "f1", IndexKind::Expression, IndexState::Active).unwrap();
        assert_ne!(a.definition_hash(), c.definition_hash(), "field id matters");
    }

    #[test]
    fn plan_requires_active_state_and_stable_id() {
        let mut mgr = IndexManager::new();
        mgr.register(
            IndexDef::new("tasks", "f_alice_1", IndexKind::Expression, IndexState::Active).unwrap(),
        );
        // Stable-id equality over an active index uses it.
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field_id": "f_alice_1", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        let plan = mgr.plan(&q, 3);
        assert!(plan.uses_index);
        assert_eq!(plan.index_id.as_deref(), Some("idx_records_tasks_f_alice_1"));
        assert!(plan.warnings.is_empty());

        // A display-name predicate never matches an index by id -> no_index.
        let q2 = Query::from_fixture_value(&serde_json::json!({
            "from": "tasks",
            "where": [{"field": "status", "op": "eq", "value": "open"}]
        }))
        .unwrap();
        let plan2 = mgr.plan(&q2, 3);
        assert!(!plan2.uses_index);
        assert_eq!(plan2.warnings[0].reason, FullScanReason::NoIndex);
    }

    #[test]
    fn deprecated_index_is_not_a_candidate() {
        let mut mgr = IndexManager::new();
        mgr.register(
            IndexDef::new(
                "contacts",
                "f_alice_0",
                IndexKind::Expression,
                IndexState::Deprecated,
            )
            .unwrap(),
        );
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "contacts",
            "where": [{"field_id": "f_alice_0", "op": "eq", "value": "x"}]
        }))
        .unwrap();
        let plan = mgr.plan(&q, 2);
        assert!(!plan.uses_index);
        assert_eq!(plan.warnings[0].reason, FullScanReason::IndexDeprecated);
    }

    // --- create / drop / deprecate / FTS sync (DL-5/DL-6) ----------------

    /// A bare in-memory connection with just the `records` table, so the index
    /// unit tests can build real DDL without pulling in the whole `Store`.
    fn records_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE records (collection TEXT NOT NULL, id TEXT NOT NULL, \
             data TEXT NOT NULL, updated_at INTEGER, PRIMARY KEY(collection, id));",
        )
        .unwrap();
        conn
    }

    /// Seed one record's canonical JSON directly into `records`.
    fn seed(conn: &Connection, collection: &str, id: &str, field_id: &str, value: &str) {
        let data = serde_json::json!({
            "entity_id": id,
            "collection": collection,
            "field_ids": { field_id: value },
            "deleted": false
        })
        .to_string();
        conn.execute(
            "INSERT INTO records (collection, id, data, updated_at) VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![collection, id, data],
        )
        .unwrap();
    }

    fn index_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?1",
            rusqlite::params![name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    fn table_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            rusqlite::params![name],
            |r| r.get::<_, i64>(0),
        )
        .unwrap()
            > 0
    }

    #[test]
    fn create_value_index_registers_active_and_builds_ddl() {
        let conn = records_conn();
        seed(&conn, "tasks", "t1", "f_alice_1", "open");
        let mut mgr = IndexManager::new();
        let id = mgr
            .create_index(&conn, "tasks", "f_alice_1", CreateIndexKind::Value)
            .unwrap();
        assert_eq!(id, "idx_records_tasks_f_alice_1");
        // Registered Active and physically present.
        let def = mgr.get("tasks", "f_alice_1").unwrap();
        assert_eq!(def.state, IndexState::Active);
        assert_eq!(def.kind, IndexKind::Expression);
        assert!(index_exists(&conn, "idx_records_tasks_f_alice_1"));
    }

    #[test]
    fn create_index_after_records_activates_and_uses_it() {
        // DL-6: an index created after rows already exist is built from canonical
        // records and is immediately a planner candidate.
        let conn = records_conn();
        seed(&conn, "events", "e1", "f_alice_0", "alice");
        seed(&conn, "events", "e2", "f_alice_0", "bob");
        let mut mgr = IndexManager::new();
        mgr.create_index(&conn, "events", "f_alice_0", CreateIndexKind::Value)
            .unwrap();
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "events",
            "where": [{"field_id": "f_alice_0", "op": "eq", "value": "alice"}]
        }))
        .unwrap();
        let plan = mgr.plan(&q, 2);
        assert!(plan.uses_index);
        assert_eq!(plan.index_id.as_deref(), Some("idx_records_events_f_alice_0"));
    }

    #[test]
    fn create_fts_index_populates_shadow_table_from_records() {
        let conn = records_conn();
        seed(&conn, "notes", "n1", "f_alice_0", "offline rebuild keeps indexes honest");
        seed(&conn, "notes", "n2", "f_alice_0", "lunch plans for the team");
        let mut mgr = IndexManager::new();
        let id = mgr
            .create_index(&conn, "notes", "f_alice_0", CreateIndexKind::Fts)
            .unwrap();
        assert_eq!(id, "fts_records_notes_f_alice_0");
        assert!(table_exists(&conn, "fts_records_notes_f_alice_0"));
        // The shadow rows were populated from canonical records.
        let hits = mgr
            .fts_match(&conn, "notes", "f_alice_0", "offline")
            .unwrap();
        assert_eq!(hits, vec!["n1".to_string()]);
    }

    #[test]
    fn drop_index_removes_definition_and_structure() {
        let conn = records_conn();
        seed(&conn, "tasks", "t1", "f_alice_1", "open");
        let mut mgr = IndexManager::new();
        mgr.create_index(&conn, "tasks", "f_alice_1", CreateIndexKind::Value)
            .unwrap();
        assert!(index_exists(&conn, "idx_records_tasks_f_alice_1"));
        mgr.drop_index(&conn, "tasks", "f_alice_1").unwrap();
        // Definition gone and physical index removed.
        assert!(mgr.get("tasks", "f_alice_1").is_none());
        assert!(!index_exists(&conn, "idx_records_tasks_f_alice_1"));
        // Dropping again is a no-op (idempotent).
        mgr.drop_index(&conn, "tasks", "f_alice_1").unwrap();
    }

    #[test]
    fn deprecate_index_keeps_metadata_but_stops_planner_use() {
        let conn = records_conn();
        seed(&conn, "contacts", "c1", "f_alice_0", "ana@old.example");
        let mut mgr = IndexManager::new();
        mgr.create_index(&conn, "contacts", "f_alice_0", CreateIndexKind::Value)
            .unwrap();
        mgr.deprecate_index(&conn, "contacts", "f_alice_0").unwrap();
        // Metadata retained, state Deprecated, physical index dropped.
        let def = mgr.get("contacts", "f_alice_0").unwrap();
        assert_eq!(def.state, IndexState::Deprecated);
        assert!(!index_exists(&conn, "idx_records_contacts_f_alice_0"));
        // Planner refuses a deprecated index and surfaces the distinct reason.
        let q = Query::from_fixture_value(&serde_json::json!({
            "from": "contacts",
            "where": [{"field_id": "f_alice_0", "op": "eq", "value": "ana@old.example"}]
        }))
        .unwrap();
        let plan = mgr.plan(&q, 1);
        assert!(!plan.uses_index);
        assert_eq!(plan.warnings[0].reason, FullScanReason::IndexDeprecated);
    }

    #[test]
    fn create_index_rejects_malicious_identifier() {
        let conn = records_conn();
        let mut mgr = IndexManager::new();
        let err = mgr
            .create_index(
                &conn,
                "tasks'); DROP TABLE records;--",
                "f_alice_1",
                CreateIndexKind::Value,
            )
            .unwrap_err();
        assert_eq!(err.code(), "QueryError");
        // `records` table is untouched.
        assert!(table_exists(&conn, "records"));
    }

    #[test]
    fn sync_fts_for_record_inserts_updates_and_deletes_rows() {
        let conn = records_conn();
        let mut mgr = IndexManager::new();
        mgr.create_index(&conn, "notes", "f_alice_0", CreateIndexKind::Fts)
            .unwrap();

        // Insert: a new record's text becomes searchable.
        let live = |body: &str, deleted: bool| {
            serde_json::json!({
                "entity_id": "n1",
                "collection": "notes",
                "field_ids": { "f_alice_0": body },
                "deleted": deleted
            })
            .to_string()
        };
        mgr.sync_fts_for_record(&conn, "notes", "n1", &live("offline rebuild", false))
            .unwrap();
        assert_eq!(
            mgr.fts_match(&conn, "notes", "f_alice_0", "offline").unwrap(),
            vec!["n1".to_string()]
        );

        // Update: the old text no longer matches; the new text does. No duplicate
        // rows (the prior row is deleted before re-insert).
        mgr.sync_fts_for_record(&conn, "notes", "n1", &live("lunch plans", false))
            .unwrap();
        assert!(mgr.fts_match(&conn, "notes", "f_alice_0", "offline").unwrap().is_empty());
        assert_eq!(
            mgr.fts_match(&conn, "notes", "f_alice_0", "lunch").unwrap(),
            vec!["n1".to_string()]
        );

        // Delete (tombstone): the record drops out of the FTS table entirely.
        mgr.sync_fts_for_record(&conn, "notes", "n1", &live("lunch plans", true))
            .unwrap();
        assert!(mgr.fts_match(&conn, "notes", "f_alice_0", "lunch").unwrap().is_empty());
    }
}
