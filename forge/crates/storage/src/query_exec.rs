//! Query execution entrypoints (DL-15/16): the scalar `query`, the index-aware
//! `query_planned`, the text-search pipeline, and the index build/create
//! conveniences, plus the JSON→SQL parameter binders the scan paths share.

use forge_domain::{CoreError, RecordEnvelope, Result};
use rusqlite::params;

use crate::errors::{map_json, map_sql};
use crate::index;
use crate::query::{self, compile_select, GroupResult, Query, QueryResult, QueryRow};
use crate::store::Store;

impl Store {
    // --- Query engine (DL-15/16) -----------------------------------------

    /// Run a compiled [`Query`] against the `records` projection (DL-15).
    ///
    /// The AST is compiled to a **parameterized** SELECT over JSON1
    /// (`json_extract`); record values are bound, never interpolated (DL-16, no
    /// raw-SQL surface). Filtering happens in SQL; ordering, limit/offset, and
    /// aggregation are finalized in Rust so the spec's platform-stable total
    /// order and null-handling rules hold exactly (`query-dsl.md` §Result).
    ///
    /// Returns rows, a single aggregate, or grouped aggregates depending on the
    /// query shape.
    pub fn query(&self, q: &Query) -> Result<QueryResult> {
        // Unsupported P1 features (a bare `text`/`join` marker) must be refused
        // BEFORE planning: scanning anyway would silently return bogus rows
        // (e.g. a `join` predicate over `assignee.name` compiles to a literal
        // `$.fields."assignee.name"` path and matches nothing/garbage). Surface
        // the typed `unsupported_feature` error so the caller sees the contract,
        // not a wrong answer (review 040 finding 7; query-dsl.md §Result).
        if let Some(feature) = &q.unsupported {
            return Err(CoreError::QueryError(format!(
                "unsupported_feature: '{feature}' is not supported in M0a (P1)"
            )));
        }
        let matched = self.scan_matched(q)?;

        // Group-by: bucket by the (display) group field, then aggregate each
        // bucket. Group keys are emitted in ascending spec order.
        if let Some(group_field) = &q.group_by {
            let agg = q.aggregate.clone().unwrap_or(query::Aggregate {
                count: true,
                sum: None,
                avg: None,
                min: None,
                max: None,
            });
            let mut buckets: Vec<(serde_json::Value, Vec<&RecordEnvelope>)> = Vec::new();
            for env in &matched {
                let key = query::group_key(env, group_field);
                match buckets.iter_mut().find(|(k, _)| k == &key) {
                    Some((_, v)) => v.push(env),
                    None => buckets.push((key, vec![env])),
                }
            }
            buckets.sort_by(|a, b| query::cmp_json_pub(&a.0, &b.0));
            let groups = buckets
                .into_iter()
                .map(|(key, rows)| GroupResult {
                    key,
                    aggregate: query::compute_aggregate(&rows, &agg),
                })
                .collect();
            return Ok(QueryResult::Groups(groups));
        }

        // Bare aggregate over the matched set.
        if let Some(agg) = &q.aggregate {
            let refs: Vec<&RecordEnvelope> = matched.iter().collect();
            return Ok(QueryResult::Aggregate(query::compute_aggregate(&refs, agg)));
        }

        // Row result: wrap, then order/offset/limit in Rust.
        let rows: Vec<QueryRow> = matched
            .into_iter()
            .map(|env| QueryRow {
                id: env.entity_id.as_str().to_string(),
                envelope: env,
            })
            .collect();
        Ok(QueryResult::Rows(query::finalize_rows(rows, q)))
    }

    /// Execute the compiled filter and return the matched envelopes (unordered).
    /// Shared by the row, aggregate, and group-by paths.
    fn scan_matched(&self, q: &Query) -> Result<Vec<RecordEnvelope>> {
        let compiled = compile_select(q)?;
        let mut stmt = self.conn.prepare(&compiled.sql).map_err(map_sql)?;
        let bound = to_sql_params(&compiled.params)?;
        let param_refs: Vec<&dyn rusqlite::ToSql> =
            bound.iter().map(|b| b as &dyn rusqlite::ToSql).collect();
        let rows = stmt
            .query_map(param_refs.as_slice(), |row| row.get::<_, String>(1))
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let json = r.map_err(map_sql)?;
            out.push(serde_json::from_str(&json).map_err(|e| map_json("query", e))?);
        }
        Ok(out)
    }

    /// Count live records in `collection` (the planner's `estimated_rows`).
    fn count_records(&self, collection: &str, include_deleted: bool) -> Result<i64> {
        let sql = if include_deleted {
            "SELECT COUNT(*) FROM records WHERE collection = ?1"
        } else {
            "SELECT COUNT(*) FROM records WHERE collection = ?1 \
             AND json_extract(data, '$.deleted') IS NOT 1"
        };
        self.conn
            .query_row(sql, params![collection], |r| r.get::<_, i64>(0))
            .map_err(map_sql)
    }

    // --- Index-aware planner (DL-5/DL-6) ---------------------------------

    /// Run a [`Query`] with index awareness against `indexes` (DL-5/DL-6).
    ///
    /// Returns the same rows/aggregates as [`query`](Self::query) — `records` is
    /// canonical, so the answer never depends on whether an index exists — plus
    /// the planner decision: `uses_index`, the `index_id` used, and any
    /// `planner.full_scan` warnings. The index decision is computed from the
    /// registered definitions and their lifecycle states (never hardcoded): an
    /// active expression index serves eq/range/order over its stable field id; an
    /// active FTS5 shadow table serves a text search; every other case scans and
    /// warns.
    ///
    /// A text search is not a bypass: the FTS shadow table (or a portable
    /// fallback scan) produces a MATCH set in rank order, then the same
    /// `filter`/`group`/`aggregate`/`order`/`limit`/`offset` pipeline is applied
    /// to that set as for a scalar query (DL-15; review 041/042 finding 4). FTS
    /// rank order is preserved unless an explicit non-rank `order_by` overrides it.
    pub fn query_planned(
        &self,
        q: &Query,
        indexes: &index::IndexManager,
    ) -> Result<query::PlannedQuery> {
        // Same guard as `query`: refuse an unsupported P1 feature before planning
        // so we never plan/scan a query that would return bogus rows (review 040
        // finding 7).
        if let Some(feature) = &q.unsupported {
            return Err(CoreError::QueryError(format!(
                "unsupported_feature: '{feature}' is not supported in M0a (P1)"
            )));
        }
        let estimated = self.count_records(&q.from, q.include_deleted)?;
        let plan = indexes.plan(q, estimated);

        // Text-search path: rows come from the FTS table when it is active,
        // otherwise from a portable `like`-style scan over the records.
        if let Some(ts) = &q.text_search {
            let result = self.run_text_search(q, ts, &plan, indexes)?;
            return Ok(query::PlannedQuery {
                result,
                uses_index: plan.uses_index,
                index_id: plan.index_id,
                warnings: plan.warnings,
            });
        }

        // Scalar path: identical to `query`, with the planner decision attached.
        let result = self.query(q)?;
        Ok(query::PlannedQuery {
            result,
            uses_index: plan.uses_index,
            index_id: plan.index_id,
            warnings: plan.warnings,
        })
    }

    /// Resolve a text search as a **MATCH source inside the normal query
    /// pipeline** (DL-15). The FTS5 shadow table (or a portable fallback scan)
    /// produces the candidate id set in rank order; the rest of the query —
    /// `filter`, `group_by`, `aggregate`, `order_by`/`limit`/`offset` — is then
    /// applied to exactly that set, just like a non-text query (review 041/042
    /// finding 4). FTS rank ordering is preserved when the query requests it (or
    /// leaves the order default); an explicit non-rank `order_by` wins.
    fn run_text_search(
        &self,
        q: &Query,
        ts: &query::TextSearch,
        plan: &index::IndexPlan,
        indexes: &index::IndexManager,
    ) -> Result<QueryResult> {
        // 1. The MATCH set: candidate ids in FTS rank order (or fallback scan).
        let match_ids: Vec<String> = if plan.uses_index {
            let field_id = ts
                .field
                .field_id()
                .ok_or_else(|| CoreError::QueryError("text search needs a stable field id".into()))?;
            indexes.fts_match(&self.conn, &q.from, field_id, &ts.query)?
        } else {
            self.text_search_scan(q, ts)?
        };
        // rank position by id (FTS already ordered by rank; index = rank).
        let rank_of: std::collections::HashMap<&str, usize> = match_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();

        // 2. Apply the query's filter over canonical records (same SQL semantics
        //    as a non-text query), then intersect with the MATCH set so the text
        //    search composes with `where`. Records are canonical, so this is the
        //    correct row set regardless of the FTS path.
        let matched: Vec<RecordEnvelope> = self
            .scan_matched(q)?
            .into_iter()
            .filter(|env| rank_of.contains_key(env.entity_id.as_str()))
            .collect();

        // 3. Group-by / aggregate over the composed set (identical to `query`).
        if let Some(group_field) = &q.group_by {
            let agg = q.aggregate.clone().unwrap_or(query::Aggregate {
                count: true,
                sum: None,
                avg: None,
                min: None,
                max: None,
            });
            let mut buckets: Vec<(serde_json::Value, Vec<&RecordEnvelope>)> = Vec::new();
            for env in &matched {
                let key = query::group_key(env, group_field);
                match buckets.iter_mut().find(|(k, _)| k == &key) {
                    Some((_, v)) => v.push(env),
                    None => buckets.push((key, vec![env])),
                }
            }
            buckets.sort_by(|a, b| query::cmp_json_pub(&a.0, &b.0));
            let groups = buckets
                .into_iter()
                .map(|(key, rows)| GroupResult {
                    key,
                    aggregate: query::compute_aggregate(&rows, &agg),
                })
                .collect();
            return Ok(QueryResult::Groups(groups));
        }
        if let Some(agg) = &q.aggregate {
            let refs: Vec<&RecordEnvelope> = matched.iter().collect();
            return Ok(QueryResult::Aggregate(query::compute_aggregate(&refs, agg)));
        }

        // 4. Row result. An explicit non-rank `order_by` is finalized with the
        //    spec total order (and its limit/offset). Otherwise FTS rank order is
        //    preserved, and the rank path's limit/offset are applied here (the
        //    bug review 041/042 finding 4 calls out: rank-path limit/offset were
        //    previously dropped).
        let rows: Vec<QueryRow> = matched
            .into_iter()
            .map(|env| QueryRow {
                id: env.entity_id.as_str().to_string(),
                envelope: env,
            })
            .collect();
        let rank_order = q
            .order_by
            .as_ref()
            .map(|ob| matches!(&ob.field, query::FieldRef::Name(n) if n == "rank"))
            .unwrap_or(true);
        let rows = if rank_order {
            self.finalize_rank_ordered(rows, &rank_of, q)
        } else {
            query::finalize_rows(rows, q)
        };
        Ok(QueryResult::Rows(rows))
    }

    /// Order a text-search row set by FTS rank (rank position, then entity id as
    /// a stable tie-break), then apply the query's `offset`/`limit`. Used when
    /// the query keeps FTS rank order (default or explicit `rank`); the rank-path
    /// limit/offset are applied here so they are not silently dropped.
    fn finalize_rank_ordered(
        &self,
        mut rows: Vec<QueryRow>,
        rank_of: &std::collections::HashMap<&str, usize>,
        q: &Query,
    ) -> Vec<QueryRow> {
        rows.sort_by(|a, b| {
            let ra = rank_of.get(a.id.as_str()).copied().unwrap_or(usize::MAX);
            let rb = rank_of.get(b.id.as_str()).copied().unwrap_or(usize::MAX);
            ra.cmp(&rb).then_with(|| a.id.cmp(&b.id))
        });
        if let Some(off) = q.offset {
            let off = off as usize;
            if off >= rows.len() {
                rows.clear();
            } else {
                rows.drain(0..off);
            }
        }
        if let Some(lim) = q.limit {
            rows.truncate(lim as usize);
        }
        rows
    }

    /// Portable text-search fallback: ASCII case-insensitive substring match over
    /// the field's stored value. Used when no active FTS table covers the search,
    /// so the rows are still correct (records are canonical) while the planner
    /// surfaces the `fts_not_available` warning.
    fn text_search_scan(&self, q: &Query, ts: &query::TextSearch) -> Result<Vec<String>> {
        // Use the SAME canonical JSON path the planner/index DDL emit, so a dotted
        // field id resolves to the literal key (not a nested path). One source of
        // truth — `FieldRef::json_path` — keys both the `$.fields.<name>` and the
        // `$.field_ids."<id>"` quoting here and in `compile`, so they can't skew.
        let path = ts.field.json_path();
        let sql = "SELECT id, json_extract(data, ?1) FROM records \
                   WHERE collection = ?2 AND json_extract(data, '$.deleted') IS NOT 1";
        let mut stmt = self.conn.prepare(sql).map_err(map_sql)?;
        let needle = ts.query.to_ascii_lowercase();
        let rows = stmt
            .query_map(params![path, q.from], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
            })
            .map_err(map_sql)?;
        let mut out = Vec::new();
        for r in rows {
            let (id, value) = r.map_err(map_sql)?;
            if let Some(text) = value {
                if text.to_ascii_lowercase().contains(&needle) {
                    out.push(id);
                }
            }
        }
        Ok(out)
    }

    /// Create the physical structures for every active index in `indexes`
    /// (idempotent expression-index DDL + populated FTS5 shadow tables), built
    /// from canonical `records`. Thin wrapper over
    /// [`IndexManager::rebuild_active`](index::IndexManager::rebuild_active) so
    /// callers need not reach the connection.
    pub fn build_indexes(&self, indexes: &index::IndexManager) -> Result<()> {
        indexes.rebuild_active(&self.conn)
    }

    /// Create (DL-5) one index over the `records` projection and build it from
    /// canonical records in a single call: `Value` → a collection-scoped JSON1
    /// expression index, `Fts` → a populated FTS5 shadow table. The definition is
    /// registered `Active` in `indexes` and its physical structure is built
    /// immediately (so creating an index *after* rows exist activates it — DL-6).
    /// Returns the deterministic `index_id`. Thin wrapper over
    /// [`IndexManager::create_index`](index::IndexManager::create_index).
    pub fn create_index(
        &self,
        indexes: &mut index::IndexManager,
        collection: &str,
        field_id: &str,
        kind: index::CreateIndexKind,
    ) -> Result<String> {
        indexes.create_index(&self.conn, collection, field_id, kind)
    }
}

/// Bind a JSON scalar as a SQLite value for a parameterized predicate. Numbers
/// bind as INTEGER/REAL (so JSON1 numeric comparisons line up), booleans as the
/// JSON1 `0`/`1` integers `json_extract` returns, strings as TEXT, and null as
/// SQL NULL. Arrays/objects are never bound (the planner rejects them upstream).
fn json_to_sql(value: &serde_json::Value) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value as V;
    let out = match value {
        serde_json::Value::Null => V::Null,
        serde_json::Value::Bool(b) => V::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                V::Integer(i)
            } else if let Some(f) = n.as_f64() {
                V::Real(f)
            } else {
                // u64 outside i64 range: store as text to avoid lossy coercion.
                V::Text(n.to_string())
            }
        }
        serde_json::Value::String(s) => V::Text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            return Err(CoreError::QueryError(
                "cannot bind a non-scalar value as a SQL parameter".into(),
            ))
        }
    };
    Ok(out)
}

/// Convert the planner's ordered JSON bind list into rusqlite values.
fn to_sql_params(values: &[serde_json::Value]) -> Result<Vec<rusqlite::types::Value>> {
    values.iter().map(json_to_sql).collect()
}
