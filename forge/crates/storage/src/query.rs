//! Query DSL AST + planner over the `records` projection (DL-15/16/17).
//!
//! prd-merged/02-data-layer-prd.md §6 and `forge/spec/query-dsl.md` pin the v1
//! query surface. This module is the storage-internal half: a typed [`Query`]
//! AST that (de)serializes directly from the Codex fixture plans, a planner that
//! compiles the AST to **parameterized** SQLite over the rebuildable `records`
//! projection (via JSON1 `json_extract`), and a [`QueryResult`] row/aggregate
//! set.
//!
//! DL-16 contract: raw SQL is never exposed. The only query surface is this
//! AST; record values are always bound as SQL parameters, never interpolated
//! into the statement text. A SQL-like string is accepted only after passing
//! [`reject_raw_sql`], which compiles to the same AST rather than executing
//! caller SQL.
//!
//! ## Field addressing
//!
//! Query plans address fields two ways and the planner resolves each to a
//! distinct canonical envelope JSON path:
//!
//! - **Display name** (`status`, `prio`) → `$.fields.<name>` — applet
//!   ergonomics; the query-DSL surface.
//! - **Stable field id** (`f_alice_1`) → `$.field_ids.<id>` — the merge/index
//!   correct addressing the dynamic-index engine and its fixtures use
//!   (`dynamic-indexes.md`). A `field_id` key in a plan resolves to the
//!   stable-id path, never the display path.
//!
//! Either path component is validated against an identifier allowlist before it
//! is placed in the (otherwise constant) statement text, so a field reference
//! can never carry SQL (DL-16).
//!
//! ## Semantics (pinned by `query-dsl.md` §Result)
//!
//! - Comparisons **do not coerce types**: `"2" > 10` is a [`CoreError::QueryError`],
//!   not silently `false`.
//! - A missing field compares as JSON `null` only for `eq(null)` / `ne(null)`;
//!   range/`like`/`in` over a missing or `null` value is `false`.
//! - Sort order is numbers < strings < booleans < nulls-last, with `entity_id`
//!   as the stable secondary key. Ordering is resolved in Rust so the total
//!   order is identical on every platform (SQLite's native affinity ordering
//!   does not match this spec rule).
//! - `LIKE` uses `%`/`_` with backslash escape and is ASCII case-insensitive,
//!   matching SQLite's portable default.

use forge_domain::{CoreError, RecordEnvelope, Result};
use serde::Deserialize;
use std::cmp::Ordering;

/// A comparison/membership operator over a single field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    In,
    Like,
}

impl Op {
    /// Parse the operator token used in fixture plans. Both the SQL-symbol form
    /// (`=`, `!=`, `<`, …, used by the array-tuple plans) and the named form
    /// (`eq`, `ne`, `gt`, …, used by the object plans) are accepted so every
    /// Codex vector parses directly.
    fn parse(token: &str) -> Result<Op> {
        let op = match token {
            "=" | "eq" => Op::Eq,
            "!=" | "ne" => Op::Ne,
            "<" | "lt" => Op::Lt,
            "<=" | "le" => Op::Le,
            ">" | "gt" => Op::Gt,
            ">=" | "ge" => Op::Ge,
            "in" | "IN" => Op::In,
            "like" | "LIKE" => Op::Like,
            other => {
                return Err(CoreError::QueryError(format!(
                    "unknown filter operator '{other}'"
                )))
            }
        };
        Ok(op)
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
}

impl Dir {
    fn parse(token: &str) -> Result<Dir> {
        match token.to_ascii_lowercase().as_str() {
            "asc" => Ok(Dir::Asc),
            "desc" => Ok(Dir::Desc),
            other => Err(CoreError::QueryError(format!(
                "unknown sort direction '{other}'"
            ))),
        }
    }
}

/// How a plan addresses a field: by display name (`$.fields.<name>`) or by
/// stable schema field id (`$.field_ids.<id>`). The two are distinct JSON paths
/// in the envelope, so the planner must not collapse one onto the other (a
/// `field_id` resolved as a display name reads `NULL` for index-fixture
/// records). See module docs and `dynamic-indexes.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldRef {
    /// A display name under `$.fields`.
    Name(String),
    /// A stable schema field id under `$.field_ids`.
    Id(String),
}

impl FieldRef {
    /// The raw identifier (display name or field id) for diagnostics/ordering.
    pub fn as_str(&self) -> &str {
        match self {
            FieldRef::Name(s) | FieldRef::Id(s) => s,
        }
    }

    /// The stable field id when this reference is one (used to match an index
    /// candidate; a display name never matches an index by id).
    pub fn field_id(&self) -> Option<&str> {
        match self {
            FieldRef::Id(s) => Some(s),
            FieldRef::Name(_) => None,
        }
    }

    /// The canonical envelope JSON path this reference resolves to. The inner
    /// name is validated by [`validate_ident`] before reaching here, so the
    /// returned path can never carry SQL.
    fn json_path(&self) -> String {
        match self {
            FieldRef::Name(s) => format!("$.fields.{s}"),
            FieldRef::Id(s) => format!("$.field_ids.{s}"),
        }
    }
}

/// A single leaf predicate: `<field> <op> <value>`.
#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    pub field: FieldRef,
    pub op: Op,
    pub value: serde_json::Value,
}

/// A boolean filter tree. Leaves are [`Predicate`]s; internal nodes are
/// explicit `and`/`or` (no implicit precedence — the plan form is fully
/// parenthesized by construction).
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Leaf(Predicate),
    And(Vec<Filter>),
    Or(Vec<Filter>),
}

/// An aggregate request. `count` ignores `field`; the numeric aggregates carry
/// the field they reduce over.
#[derive(Debug, Clone, PartialEq)]
pub struct Aggregate {
    pub count: bool,
    pub sum: Option<FieldRef>,
    pub avg: Option<FieldRef>,
    pub min: Option<FieldRef>,
    pub max: Option<FieldRef>,
}

/// An ordering key.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderBy {
    pub field: FieldRef,
    pub dir: Dir,
}

/// A full-text search request over a single field (P1 in the query DSL, but
/// pinned by the dynamic-index fixtures: an active FTS5 shadow table answers it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSearch {
    pub field: FieldRef,
    pub query: String,
}

/// The compiled query AST. Round-trips the structured `plan`/`query` shapes the
/// Codex fixtures carry; see [`Query::from_fixture_value`].
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub from: String,
    pub filter: Option<Filter>,
    pub order_by: Option<OrderBy>,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub aggregate: Option<Aggregate>,
    pub group_by: Option<FieldRef>,
    /// A full-text search request (dynamic-indexes.md): matched against an
    /// active FTS5 shadow table when one exists, else a `like`-style scan with a
    /// `planner.full_scan` warning.
    pub text_search: Option<TextSearch>,
    /// Whether tombstoned (`deleted`) rows are included. Normal queries hide
    /// them (query-dsl.md §Data Model).
    pub include_deleted: bool,
    /// Whether a deprecated field's stored values are still queryable. The
    /// deprecated-index fixture sets this; it does not affect rows (records keep
    /// the old field), only that a deprecated index is not a planner candidate.
    pub include_deprecated: bool,
    /// A P1 feature (join) was requested. The planner records the requested
    /// feature so the runner can surface an `unsupported_feature` warning
    /// instead of executing an unimplemented path.
    pub unsupported: Option<String>,
}

impl Query {
    /// A bare scan of one collection (no filter, default order).
    pub fn from(collection: impl Into<String>) -> Self {
        Query {
            from: collection.into(),
            filter: None,
            order_by: None,
            limit: None,
            offset: None,
            aggregate: None,
            group_by: None,
            text_search: None,
            include_deleted: false,
            include_deprecated: false,
            unsupported: None,
        }
    }
}

// --- Fixture (de)serialization --------------------------------------------

/// Validate that a field/collection name is a safe identifier before it is
/// placed into the JSON path of an (otherwise constant) statement. Names are
/// caller-supplied, so they must never be able to carry SQL or break out of the
/// `$.fields.<name>` path (DL-16). Allowed: ASCII alphanumerics, `_`, `-`, `.`
/// and `/` (the latter two appear in entity ids / nested display names).
fn validate_ident(kind: &str, name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(CoreError::QueryError(format!("empty {kind} name")));
    }
    let ok = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/'));
    if !ok {
        return Err(CoreError::QueryError(format!(
            "{kind} name '{name}' contains characters not allowed in a field path"
        )));
    }
    Ok(())
}

/// Validate a `collection`/`field_id` before it is placed into index DDL (the
/// index name and the partial-predicate collection literal). Same allowlist as
/// [`validate_ident`]; exposed to the index module so a hostile identifier is
/// refused at definition time rather than interpolated into structure.
pub(crate) fn validate_index_ident(kind: &str, name: &str) -> Result<()> {
    validate_ident(kind, name)
}

impl Query {
    /// Parse a fixture's structured query value into the AST.
    ///
    /// Two shapes appear in the corpus and both are accepted:
    ///
    /// - **Array-tuple plan** (`{"from":…, "where":["status","=","todo"],
    ///   "orderBy":["prio","asc"], "limit":…, "aggregate":…, "groupBy":…}`):
    ///   `where` leaves are `[field, op, value]`; `and`/`or` nodes are
    ///   `{"and":[…]}` / `{"or":[…]}`; ops use SQL symbols.
    /// - **Object plan** (`{"from":…, "where":{"field":…,"op":"gt","value":…},
    ///   "orderBy":[{"field":…,"dir":…}]}`): `where` leaves are
    ///   `{field, op, value}`; ops use names; `orderBy` is a list (first key
    ///   used). `text`/`join` mark a P1 unsupported feature.
    pub fn from_fixture_value(v: &serde_json::Value) -> Result<Query> {
        let obj = v
            .as_object()
            .ok_or_else(|| CoreError::QueryError("query plan must be a JSON object".into()))?;

        let from = obj
            .get("from")
            .and_then(|f| f.as_str())
            .ok_or_else(|| CoreError::QueryError("query plan missing 'from' collection".into()))?
            .to_string();
        validate_ident("collection", &from)?;

        let mut q = Query::from(from);

        // `text`/`text_search`: a full-text search request. The index-fixture
        // form is the structured `text_search: {field_id|field, match}`; the
        // query-DSL P1 form is a bare `text` marker. The structured form is a
        // first-class request answered by an FTS5 shadow table (or a scan with a
        // warning); the bare marker stays an unsupported-feature flag.
        if let Some(ts) = obj.get("text_search") {
            q.text_search = Some(parse_text_search(ts)?);
        } else if obj.contains_key("text") {
            q.unsupported = Some("text_search".into());
        }
        // P1 join: record the requested feature; the planner falls back to a
        // best-effort scan and the runner surfaces the unsupported warning.
        if obj.contains_key("join") {
            q.unsupported = Some("join".into());
        }

        if let Some(w) = obj.get("where") {
            if !w.is_null() {
                q.filter = Some(parse_filter(w)?);
            }
        }

        // orderBy / order_by: array tuple [field, dir] OR array of {field, dir}
        // OR a single {field, dir}.
        let order_value = obj.get("orderBy").or_else(|| obj.get("order_by"));
        if let Some(ob) = order_value {
            q.order_by = parse_order_by(ob)?;
        }

        if let Some(g) = obj.get("groupBy").or_else(|| obj.get("group_by")) {
            // A bare string is a display name; an object form may carry field_id.
            if let Some(s) = g.as_str() {
                q.group_by = Some(field_ref_from_name(s)?);
            } else if g.is_object() {
                q.group_by = Some(field_ref_from_obj(g.as_object().unwrap())?);
            }
        }

        if let Some(lim) = obj.get("limit") {
            q.limit = Some(parse_nonneg_int("limit", lim)?);
        }
        if let Some(off) = obj.get("offset") {
            q.offset = Some(parse_nonneg_int("offset", off)?);
        }

        if let Some(agg) = obj.get("aggregate") {
            q.aggregate = Some(parse_aggregate(agg)?);
        }

        if let Some(inc) = obj
            .get("includeDeleted")
            .or_else(|| obj.get("include_deleted"))
            .and_then(|b| b.as_bool())
        {
            q.include_deleted = inc;
        }
        if let Some(inc) = obj
            .get("includeDeprecated")
            .or_else(|| obj.get("include_deprecated"))
            .and_then(|b| b.as_bool())
        {
            q.include_deprecated = inc;
        }

        Ok(q)
    }
}

/// Resolve a display name into a [`FieldRef::Name`] after validating it.
fn field_ref_from_name(name: &str) -> Result<FieldRef> {
    validate_ident("field", name)?;
    Ok(FieldRef::Name(name.to_string()))
}

/// Resolve an object that names a field. `field_id` wins (stable-id path);
/// otherwise `field`/`name` is a display name. Both are validated.
fn field_ref_from_obj(obj: &serde_json::Map<String, serde_json::Value>) -> Result<FieldRef> {
    if let Some(id) = obj.get("field_id").and_then(|f| f.as_str()) {
        validate_ident("field id", id)?;
        return Ok(FieldRef::Id(id.to_string()));
    }
    let name = obj
        .get("field")
        .or_else(|| obj.get("name"))
        .and_then(|f| f.as_str())
        .ok_or_else(|| CoreError::QueryError("field reference missing 'field'/'field_id'".into()))?;
    field_ref_from_name(name)
}

/// Parse a `text_search: {field_id|field, match|query}` request.
fn parse_text_search(v: &serde_json::Value) -> Result<TextSearch> {
    let obj = v
        .as_object()
        .ok_or_else(|| CoreError::QueryError("text_search must be an object".into()))?;
    let field = field_ref_from_obj(obj)?;
    let query = obj
        .get("match")
        .or_else(|| obj.get("query"))
        .and_then(|m| m.as_str())
        .ok_or_else(|| CoreError::QueryError("text_search missing 'match'".into()))?
        .to_string();
    if query.is_empty() {
        return Err(CoreError::QueryError(
            "text_search 'match' must be non-empty".into(),
        ));
    }
    Ok(TextSearch { field, query })
}

fn parse_nonneg_int(name: &str, v: &serde_json::Value) -> Result<i64> {
    let n = v
        .as_i64()
        .ok_or_else(|| CoreError::QueryError(format!("{name} must be an integer")))?;
    if n < 0 {
        return Err(CoreError::QueryError(format!("{name} must be >= 0")));
    }
    Ok(n)
}

fn parse_filter(v: &serde_json::Value) -> Result<Filter> {
    match v {
        serde_json::Value::Array(items) => {
            // Two array shapes:
            //  - a single tuple leaf `["field", "op", value]` (query-DSL plans),
            //  - a list of sub-filters treated as an implicit AND (the
            //    dynamic-index `where: [{...}, {...}]` form).
            if is_tuple_leaf(items) {
                let field = field_ref_from_name(items[0].as_str().unwrap())?;
                let op = Op::parse(items[1].as_str().unwrap())?;
                return Ok(Filter::Leaf(Predicate {
                    field,
                    op,
                    value: items[2].clone(),
                }));
            }
            if items.is_empty() {
                return Err(CoreError::QueryError(
                    "array filter must be [field, op, value] or a non-empty list".into(),
                ));
            }
            // Implicit AND over the listed sub-filters.
            Ok(Filter::And(items.iter().map(parse_filter).collect::<Result<_>>()?))
        }
        serde_json::Value::Object(obj) => {
            if let Some(items) = obj.get("and") {
                return Ok(Filter::And(parse_filter_list(items)?));
            }
            if let Some(items) = obj.get("or") {
                return Ok(Filter::Or(parse_filter_list(items)?));
            }
            // Object leaf: {field|field_id, op, value}
            let field = field_ref_from_obj(obj)?;
            let op = Op::parse(
                obj.get("op")
                    .and_then(|o| o.as_str())
                    .ok_or_else(|| CoreError::QueryError("object filter missing 'op'".into()))?,
            )?;
            let value = obj.get("value").cloned().unwrap_or(serde_json::Value::Null);
            Ok(Filter::Leaf(Predicate { field, op, value }))
        }
        _ => Err(CoreError::QueryError(
            "filter must be an array leaf or an and/or/leaf object".into(),
        )),
    }
}

/// Whether an array is a `[field, op, value]` tuple leaf (first two elements are
/// strings). A list whose first element is an object/array is an implicit-AND
/// list of sub-filters instead.
fn is_tuple_leaf(items: &[serde_json::Value]) -> bool {
    items.len() == 3 && items[0].is_string() && items[1].is_string()
}

fn parse_filter_list(v: &serde_json::Value) -> Result<Vec<Filter>> {
    let arr = v
        .as_array()
        .ok_or_else(|| CoreError::QueryError("and/or must contain an array of filters".into()))?;
    if arr.is_empty() {
        return Err(CoreError::QueryError(
            "and/or filter list must be non-empty".into(),
        ));
    }
    arr.iter().map(parse_filter).collect()
}

fn parse_order_by(v: &serde_json::Value) -> Result<Option<OrderBy>> {
    match v {
        serde_json::Value::Array(items) => {
            // Either ["field","dir"] (two strings) or [{field|field_id,dir}, …].
            if items.len() == 2 && items[0].is_string() && items[1].is_string() {
                let field = field_ref_from_name(items[0].as_str().unwrap())?;
                let dir = Dir::parse(items[1].as_str().unwrap())?;
                return Ok(Some(OrderBy { field, dir }));
            }
            // Array of objects; the planner supports one key (first).
            if let Some(first) = items.first() {
                return parse_order_obj(first);
            }
            Ok(None)
        }
        serde_json::Value::Object(_) => parse_order_obj(v),
        _ => Err(CoreError::QueryError(
            "orderBy must be [field, dir] or a list of {field, dir}".into(),
        )),
    }
}

fn parse_order_obj(v: &serde_json::Value) -> Result<Option<OrderBy>> {
    let obj = v
        .as_object()
        .ok_or_else(|| CoreError::QueryError("orderBy entry must be an object".into()))?;
    let field = field_ref_from_obj(obj)?;
    let dir = obj
        .get("dir")
        .and_then(|d| d.as_str())
        .map(Dir::parse)
        .transpose()?
        .unwrap_or(Dir::Asc);
    Ok(Some(OrderBy { field, dir }))
}

fn parse_aggregate(v: &serde_json::Value) -> Result<Aggregate> {
    let obj = v
        .as_object()
        .ok_or_else(|| CoreError::QueryError("aggregate must be an object".into()))?;
    let mut agg = Aggregate {
        count: false,
        sum: None,
        avg: None,
        min: None,
        max: None,
    };
    // {"op":"count"} form.
    if let Some(op) = obj.get("op").and_then(|o| o.as_str()) {
        match op {
            "count" => agg.count = true,
            other => {
                return Err(CoreError::QueryError(format!(
                    "unknown aggregate op '{other}'"
                )))
            }
        }
    }
    // {"sum":"field", "avg":"field", …} bundle form. A string is a display
    // name; an object form may carry `field_id` for stable-id addressing.
    let field_for = |key: &str| -> Result<Option<FieldRef>> {
        match obj.get(key) {
            Some(serde_json::Value::String(s)) => Ok(Some(field_ref_from_name(s)?)),
            Some(serde_json::Value::Object(o)) => Ok(Some(field_ref_from_obj(o)?)),
            Some(serde_json::Value::Null) | None => Ok(None),
            Some(_) => Err(CoreError::QueryError(format!(
                "aggregate '{key}' must name a field"
            ))),
        }
    };
    agg.sum = field_for("sum")?;
    agg.avg = field_for("avg")?;
    agg.min = field_for("min")?;
    agg.max = field_for("max")?;
    if obj.get("count").and_then(|c| c.as_bool()) == Some(true) {
        agg.count = true;
    }
    if !agg.count
        && agg.sum.is_none()
        && agg.avg.is_none()
        && agg.min.is_none()
        && agg.max.is_none()
    {
        return Err(CoreError::QueryError(
            "aggregate request selected no aggregate".into(),
        ));
    }
    Ok(agg)
}

// --- Compiled SQL ----------------------------------------------------------

/// A compiled, parameterized SELECT: the statement text plus the ordered bind
/// values. Record values live **only** in `params`, never in `sql` (DL-16).
#[derive(Debug, Clone)]
pub struct CompiledSelect {
    pub sql: String,
    pub params: Vec<serde_json::Value>,
}

/// The JSON path a field reference resolves to in the envelope: `$.fields.<name>`
/// for a display name, `$.field_ids.<id>` for a stable id. The inner identifier
/// is validated by [`validate_ident`] before reaching here.
fn field_path(field: &FieldRef) -> String {
    field.json_path()
}

/// Compile the filter tree to a parameterized SQL boolean expression, pushing
/// every record value onto `params` as a bound parameter.
///
/// Type-coercion rule (query-dsl.md §Result): a comparison whose operand types
/// disagree (e.g. string vs number) is a `QueryError`, surfaced here at compile
/// time rather than producing a silently-wrong answer. Equality/inequality are
/// permitted across types (they are simply false/true). Range ops require a
/// numeric operand; the column value is matched at runtime via JSON1 type
/// guards so a non-numeric stored value does not coerce.
fn compile_filter(filter: &Filter, params: &mut Vec<serde_json::Value>) -> Result<String> {
    match filter {
        Filter::And(items) => compile_join(items, "AND", params),
        Filter::Or(items) => compile_join(items, "OR", params),
        Filter::Leaf(p) => compile_leaf(p, params),
    }
}

fn compile_join(
    items: &[Filter],
    sep: &str,
    params: &mut Vec<serde_json::Value>,
) -> Result<String> {
    let mut parts = Vec::with_capacity(items.len());
    for f in items {
        parts.push(compile_filter(f, params)?);
    }
    Ok(format!("({})", parts.join(&format!(" {sep} "))))
}

/// `json_extract` expression for a field's value.
fn extract_expr(field: &FieldRef) -> String {
    format!("json_extract(data, '{}')", field_path(field))
}

/// `json_type` of a field's value (NULL when the path is absent), used to guard
/// range comparisons so only numeric stored values participate.
fn type_expr(field: &FieldRef) -> String {
    format!("json_type(data, '{}')", field_path(field))
}

fn compile_leaf(p: &Predicate, params: &mut Vec<serde_json::Value>) -> Result<String> {
    let col = extract_expr(&p.field);
    match p.op {
        Op::Eq | Op::Ne => {
            // eq(null)/ne(null) test JSON null / path absence; otherwise bind.
            if p.value.is_null() {
                // A missing path and a stored JSON null both read as SQL NULL via
                // json_extract, matching the "missing compares as null for
                // eq(null)/ne(null)" rule.
                let expr = if p.op == Op::Eq {
                    format!("{col} IS NULL")
                } else {
                    format!("{col} IS NOT NULL")
                };
                return Ok(expr);
            }
            let bind = bind_index(params, &p.value)?;
            // NULL/missing never equals a concrete value; for `ne` it should be
            // true (the value differs), so guard explicitly.
            if p.op == Op::Eq {
                Ok(format!("{col} = ?{bind}"))
            } else {
                Ok(format!("({col} IS NULL OR {col} <> ?{bind})"))
            }
        }
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            // Range comparisons require a numeric operand and a numeric stored
            // value; missing/null/non-numeric stored values are false
            // (query-dsl.md §Result). Reject a non-numeric query operand as a
            // type-coercion error.
            if !p.value.is_number() {
                return Err(CoreError::QueryError(format!(
                    "range operator on field '{}' requires a numeric value, got {}",
                    p.field.as_str(),
                    p.value
                )));
            }
            let sym = match p.op {
                Op::Lt => "<",
                Op::Le => "<=",
                Op::Gt => ">",
                Op::Ge => ">=",
                _ => unreachable!(),
            };
            let bind = bind_index(params, &p.value)?;
            let ty = type_expr(&p.field);
            // Only compare when the stored value is itself numeric.
            Ok(format!(
                "({ty} IN ('integer','real') AND {col} {sym} ?{bind})"
            ))
        }
        Op::In => {
            let arr = p.value.as_array().ok_or_else(|| {
                CoreError::QueryError(format!(
                    "`in` on field '{}' requires an array",
                    p.field.as_str()
                ))
            })?;
            if arr.is_empty() {
                return Err(CoreError::QueryError(format!(
                    "`in` on field '{}' requires a non-empty array",
                    p.field.as_str()
                )));
            }
            let mut placeholders = Vec::with_capacity(arr.len());
            for val in arr {
                if val.is_null() || val.is_array() || val.is_object() {
                    return Err(CoreError::QueryError(format!(
                        "`in` on field '{}' requires scalar values",
                        p.field.as_str()
                    )));
                }
                let bind = bind_index(params, val)?;
                placeholders.push(format!("?{bind}"));
            }
            Ok(format!("{col} IN ({})", placeholders.join(", ")))
        }
        Op::Like => {
            let pat = p.value.as_str().ok_or_else(|| {
                CoreError::QueryError(format!(
                    "`like` on field '{}' requires a string",
                    p.field.as_str()
                ))
            })?;
            // Bind the pattern; backslash escapes the LIKE metacharacters. LIKE
            // is ASCII case-insensitive (SQLite default).
            let bind = bind_index(params, &serde_json::Value::String(pat.to_string()))?;
            Ok(format!("{col} LIKE ?{bind} ESCAPE '\\'"))
        }
    }
}

/// Push `value` and return its 1-based bind index. Only JSON scalars are
/// bindable (objects/arrays are never compared directly).
fn bind_index(params: &mut Vec<serde_json::Value>, value: &serde_json::Value) -> Result<usize> {
    if value.is_array() || value.is_object() {
        return Err(CoreError::QueryError(
            "cannot bind a non-scalar value in a predicate".into(),
        ));
    }
    params.push(value.clone());
    Ok(params.len())
}

/// Compile the row-returning select: always reads `id`, `data` for the matched
/// rows, scoped to the collection, with the filter applied. Ordering, limit, and
/// offset are applied in Rust (so the spec's total order is platform-stable), so
/// the SQL is an unordered match set.
pub fn compile_select(q: &Query) -> Result<CompiledSelect> {
    let mut params = Vec::new();
    // Collection is validated as an identifier; bind it anyway for defense in
    // depth (it is data, not structure).
    let mut where_parts = vec!["collection = ?1".to_string()];
    params.push(serde_json::Value::String(q.from.clone()));
    if !q.include_deleted {
        where_parts.push("json_extract(data, '$.deleted') IS NOT 1".to_string());
    }
    if let Some(filter) = &q.filter {
        where_parts.push(compile_filter(filter, &mut params)?);
    }
    let sql = format!(
        "SELECT id, data FROM records WHERE {}",
        where_parts.join(" AND ")
    );
    Ok(CompiledSelect { sql, params })
}

// --- Result model + ordering ----------------------------------------------

/// One returned row: its `entity_id` and the reconstructed envelope.
#[derive(Debug, Clone, PartialEq)]
pub struct QueryRow {
    pub id: String,
    pub envelope: RecordEnvelope,
}

/// A numeric/count aggregate result bundle.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AggregateResult {
    pub count: Option<i64>,
    pub sum: Option<f64>,
    pub avg: Option<f64>,
    pub min: Option<serde_json::Value>,
    pub max: Option<serde_json::Value>,
}

/// One group bucket: its key plus the aggregate over the bucket's rows.
#[derive(Debug, Clone, PartialEq)]
pub struct GroupResult {
    pub key: serde_json::Value,
    pub aggregate: AggregateResult,
}

/// The shape returned by [`crate::Store::query`]: either a row set, a single
/// aggregate, or grouped aggregates. Warnings (e.g. unsupported P1 features)
/// ride alongside.
#[derive(Debug, Clone, PartialEq)]
pub enum QueryResult {
    Rows(Vec<QueryRow>),
    Aggregate(AggregateResult),
    Groups(Vec<GroupResult>),
}

impl QueryResult {
    /// The ordered `entity_id`s of a row result (empty for aggregate/group
    /// results). Convenience for the fixture assertions.
    pub fn ids(&self) -> Vec<String> {
        match self {
            QueryResult::Rows(rows) => rows.iter().map(|r| r.id.clone()).collect(),
            _ => Vec::new(),
        }
    }
}

/// Why the planner fell back to a full scan instead of an active index, mirroring
/// the `dynamic-indexes.md` §Full-Scan Warnings `reason` codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FullScanReason {
    /// No index definition exists for the predicate's field.
    NoIndex,
    /// An index exists but is not in the `active` state.
    IndexNotActive,
    /// The matching index belongs to a deprecated field.
    IndexDeprecated,
    /// The operator cannot be served by the available index kind.
    UnsupportedOperator,
    /// A text search was requested but no active FTS shadow table covers it.
    FtsNotAvailable,
}

impl FullScanReason {
    /// The stable wire string used in the warning payload (matches the fixtures).
    pub fn code(&self) -> &'static str {
        match self {
            FullScanReason::NoIndex => "no_index",
            FullScanReason::IndexNotActive => "index_not_active",
            FullScanReason::IndexDeprecated => "index_deprecated",
            FullScanReason::UnsupportedOperator => "unsupported_operator",
            FullScanReason::FtsNotAvailable => "fts_not_available",
        }
    }
}

/// A `planner.full_scan` warning surfaced when the planner scans `records` for a
/// predicate/sort/search that no active index covers (`dynamic-indexes.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerWarning {
    /// Always `planner.full_scan` for these warnings.
    pub code: String,
    pub collection: String,
    /// The stable field id when known, else the display field name.
    pub field_id: Option<String>,
    pub field_name: Option<String>,
    pub reason: FullScanReason,
    /// The number of records scanned, when known.
    pub estimated_rows: Option<i64>,
}

impl PlannerWarning {
    /// Build a `planner.full_scan` warning for `field` over `collection`.
    pub fn full_scan(
        collection: &str,
        field: &FieldRef,
        reason: FullScanReason,
        estimated_rows: Option<i64>,
    ) -> Self {
        let (field_id, field_name) = match field {
            FieldRef::Id(id) => (Some(id.clone()), None),
            FieldRef::Name(name) => (None, Some(name.clone())),
        };
        PlannerWarning {
            code: "planner.full_scan".to_string(),
            collection: collection.to_string(),
            field_id,
            field_name,
            reason,
            estimated_rows,
        }
    }
}

/// The full planner outcome: the row/aggregate/group result plus the index
/// decision (`uses_index` / `index_id`) and any `planner.full_scan` warnings.
/// [`crate::Store::query`] returns the bare [`QueryResult`]; the index-aware
/// surface ([`crate::Store::query_planned`]) returns this.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedQuery {
    pub result: QueryResult,
    /// Whether an active index served the query's predicate/search.
    pub uses_index: bool,
    /// The id of the index used, when `uses_index` is true.
    pub index_id: Option<String>,
    pub warnings: Vec<PlannerWarning>,
}

impl PlannedQuery {
    /// Convenience: the ordered entity ids of the result.
    pub fn ids(&self) -> Vec<String> {
        self.result.ids()
    }
}

/// JSON value sort rank for the spec order: numbers < strings < booleans <
/// null (last). Used as the primary ordering discriminator.
fn type_rank(v: &serde_json::Value) -> u8 {
    match v {
        serde_json::Value::Number(_) => 0,
        serde_json::Value::String(_) => 1,
        serde_json::Value::Bool(_) => 2,
        // Arrays/objects are not orderable scalars; treat them just before null
        // so they remain deterministic without panicking.
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => 3,
        serde_json::Value::Null => 4,
    }
}

/// Total order over two JSON scalars per query-dsl.md §Result. Within a type,
/// numbers compare numerically, strings/bools lexically/by value; across types
/// the [`type_rank`] decides. Always a total order (never `Equal` unless truly
/// equal), so the caller appends `entity_id` as the stable tie-break.
fn cmp_json(a: &serde_json::Value, b: &serde_json::Value) -> Ordering {
    let (ra, rb) = (type_rank(a), type_rank(b));
    if ra != rb {
        return ra.cmp(&rb);
    }
    match (a, b) {
        (serde_json::Value::Number(x), serde_json::Value::Number(y)) => x
            .as_f64()
            .unwrap_or(f64::NAN)
            .partial_cmp(&y.as_f64().unwrap_or(f64::NAN))
            .unwrap_or(Ordering::Equal),
        (serde_json::Value::String(x), serde_json::Value::String(y)) => x.cmp(y),
        (serde_json::Value::Bool(x), serde_json::Value::Bool(y)) => x.cmp(y),
        _ => Ordering::Equal,
    }
}

/// Public total order over two JSON scalars (spec order), exposed for the
/// group-by bucket sort in [`crate::Store::query`].
pub fn cmp_json_pub(a: &serde_json::Value, b: &serde_json::Value) -> Ordering {
    cmp_json(a, b)
}

/// The value a row exposes for `field` (or JSON null when absent), used for
/// ordering and grouping. A display name reads `$.fields`; a stable id reads
/// `$.field_ids` — the same split the SQL planner applies, so Rust-side ordering
/// and SQL-side filtering agree.
fn row_field<'a>(env: &'a RecordEnvelope, field: &FieldRef) -> &'a serde_json::Value {
    const NULL: serde_json::Value = serde_json::Value::Null;
    match field {
        FieldRef::Name(name) => env.fields.get(name).unwrap_or(&NULL),
        FieldRef::Id(id) => env.field_ids.get(id).unwrap_or(&NULL),
    }
}

/// The value a row exposes for grouping by a [`FieldRef`] (owned).
pub fn group_key(env: &RecordEnvelope, field: &FieldRef) -> serde_json::Value {
    row_field(env, field).clone()
}

/// Apply ordering (spec total order), then offset, then limit, in Rust so the
/// result is platform-stable. `id` is always the secondary tie-break.
pub fn finalize_rows(mut rows: Vec<QueryRow>, q: &Query) -> Vec<QueryRow> {
    if let Some(ob) = &q.order_by {
        // `id`/`entity_id` (only meaningful as a display name) order purely by
        // the stable entity-id tie-break below.
        let by_entity_id = matches!(&ob.field, FieldRef::Name(n) if n == "id" || n == "entity_id");
        rows.sort_by(|a, b| {
            let primary = if by_entity_id {
                Ordering::Equal
            } else {
                cmp_json(row_field(&a.envelope, &ob.field), row_field(&b.envelope, &ob.field))
            };
            let primary = match ob.dir {
                Dir::Asc => primary,
                Dir::Desc => primary.reverse(),
            };
            // entity_id tie-break is ALWAYS ascending (stable secondary order),
            // independent of the primary direction (query-dsl.md §Result).
            primary.then_with(|| a.id.cmp(&b.id))
        });
    } else {
        // No explicit order: stable by entity_id (matches list_records and the
        // fixtures' default ordering expectation).
        rows.sort_by(|a, b| a.id.cmp(&b.id));
    }
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

/// Compute the numeric aggregate bundle over a set of rows for the requested
/// `aggregate`. Sum/avg consider only numeric values; min/max use the spec sort
/// order; count is the row count. Empty inputs yield `count=0`, `sum=0`, and
/// `None` for avg/min/max (SQL semantics).
pub fn compute_aggregate(rows: &[&RecordEnvelope], agg: &Aggregate) -> AggregateResult {
    let mut out = AggregateResult::default();
    if agg.count {
        out.count = Some(rows.len() as i64);
    }
    if let Some(field) = &agg.sum {
        let mut sum = 0.0;
        for env in rows {
            if let Some(n) = row_field(env, field).as_f64() {
                sum += n;
            }
        }
        out.sum = Some(sum);
    }
    if let Some(field) = &agg.avg {
        let mut sum = 0.0;
        let mut n = 0u64;
        for env in rows {
            if let Some(x) = row_field(env, field).as_f64() {
                sum += x;
                n += 1;
            }
        }
        out.avg = if n == 0 { None } else { Some(sum / n as f64) };
    }
    if let Some(field) = &agg.min {
        out.min = rows
            .iter()
            .map(|e| row_field(e, field))
            .filter(|v| !v.is_null())
            .min_by(|a, b| cmp_json(a, b))
            .cloned();
    }
    if let Some(field) = &agg.max {
        out.max = rows
            .iter()
            .map(|e| row_field(e, field))
            .filter(|v| !v.is_null())
            .max_by(|a, b| cmp_json(a, b))
            .cloned();
    }
    out
}

// --- Mutation plans (DL-17) ------------------------------------------------

/// A mutation as carried in a fixture's `mutations[]` or a `transact` group.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub enum Mutation {
    Insert {
        collection: String,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        fields: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Update {
        collection: String,
        id: String,
        #[serde(default)]
        fields: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Patch {
        collection: String,
        id: String,
        #[serde(default)]
        fields: serde_json::Map<String, serde_json::Value>,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Delete {
        collection: String,
        id: String,
        #[serde(default)]
        logical_at: Option<i64>,
    },
    Transact {
        items: Vec<Mutation>,
    },
}

// --- SQL-like string rejection (DL-16) -------------------------------------

/// Reject a SQL-like string that escapes the validated subset. The applet-facing
/// surface is the AST; a string form is accepted only by the data browser/SDK
/// and must compile to the AST, never execute as raw SQL. This guard refuses the
/// raw-SQL / out-of-subset cases the fixtures pin (`DROP`, `;`, comments, DDL/
/// DML keywords) with a `QueryError` carrying the contract phrase.
pub fn reject_raw_sql(sql_like: &str) -> Result<()> {
    let lowered = sql_like.to_ascii_lowercase();
    // Statement terminators / comments / multiple statements are never allowed.
    if sql_like.contains(';') || lowered.contains("--") || lowered.contains("/*") {
        return Err(CoreError::QueryError(
            "raw SQL is not exposed: statement terminators and comments are rejected".into(),
        ));
    }
    // Any DDL/DML/PRAGMA keyword is outside the read-only validated subset.
    const BANNED: &[&str] = &[
        "insert ", "update ", "delete ", "drop ", "alter ", "create ", "pragma ", "attach ",
        "detach ", "replace ", "vacuum ", "reindex ", "truncate ",
    ];
    for kw in BANNED {
        if lowered.contains(kw) {
            return Err(CoreError::QueryError(format!(
                "raw SQL is not exposed: '{}' is outside the validated query subset",
                kw.trim()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn plan(v: serde_json::Value) -> Query {
        Query::from_fixture_value(&v).expect("parse plan")
    }

    // --- AST parse: both fixture shapes ----------------------------------

    #[test]
    fn parses_array_tuple_leaf() {
        let q = plan(json!({"from": "tasks", "where": ["status", "=", "todo"]}));
        assert_eq!(q.from, "tasks");
        match q.filter.unwrap() {
            Filter::Leaf(p) => {
                assert_eq!(p.field, FieldRef::Name("status".into()));
                assert_eq!(p.op, Op::Eq);
                assert_eq!(p.value, json!("todo"));
            }
            other => panic!("expected leaf, got {other:?}"),
        }
    }

    #[test]
    fn parses_object_leaf_named_op() {
        let q = plan(json!({"from": "tasks", "where": {"field": "prio", "op": "gt", "value": 2}}));
        match q.filter.unwrap() {
            Filter::Leaf(p) => {
                assert_eq!(p.op, Op::Gt);
                assert_eq!(p.value, json!(2));
            }
            other => panic!("expected leaf, got {other:?}"),
        }
    }

    #[test]
    fn parses_nested_and_or() {
        let q = plan(json!({
            "from": "tasks",
            "where": {"and": [["status", "=", "todo"], {"or": [["prio", ">", 2], ["tag", "=", "home"]]}]}
        }));
        match q.filter.unwrap() {
            Filter::And(items) => {
                assert_eq!(items.len(), 2);
                assert!(matches!(items[1], Filter::Or(_)));
            }
            other => panic!("expected AND, got {other:?}"),
        }
    }

    #[test]
    fn parses_order_limit_offset_both_forms() {
        let a = plan(json!({"from": "t", "orderBy": ["prio", "desc"], "limit": 5, "offset": 2}));
        let ob = a.order_by.unwrap();
        assert_eq!(ob.field, FieldRef::Name("prio".into()));
        assert_eq!(ob.dir, Dir::Desc);
        assert_eq!(a.limit, Some(5));
        assert_eq!(a.offset, Some(2));

        let b = plan(json!({"from": "t", "order_by": [{"field": "prio", "dir": "asc"}]}));
        assert_eq!(b.order_by.unwrap().dir, Dir::Asc);
    }

    // --- validation: identifiers + bad shapes ----------------------------

    #[test]
    fn rejects_field_with_sql_metacharacters() {
        let err = Query::from_fixture_value(&json!({
            "from": "tasks",
            "where": ["status'); DROP TABLE records;--", "=", "x"]
        }))
        .unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn rejects_negative_limit() {
        let err = Query::from_fixture_value(&json!({"from": "t", "limit": -1})).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    #[test]
    fn rejects_unknown_operator() {
        let err =
            Query::from_fixture_value(&json!({"from": "t", "where": ["a", "~~", 1]})).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    // --- compile: parameterization (DL-16) -------------------------------

    #[test]
    fn values_are_bound_never_interpolated() {
        let q = plan(json!({"from": "tasks", "where": ["status", "=", "todo'; DROP"]}));
        let c = compile_select(&q).unwrap();
        // The dangerous value is a bound parameter, not in the SQL text.
        assert!(!c.sql.contains("DROP"), "value must not appear in SQL: {}", c.sql);
        assert!(c.sql.contains("?"), "predicate must use a placeholder");
        assert!(c.params.iter().any(|p| p == &json!("todo'; DROP")));
    }

    #[test]
    fn compile_in_lists_one_placeholder_per_value() {
        let q = plan(json!({"from": "t", "where": ["status", "in", ["a", "b", "c"]]}));
        let c = compile_select(&q).unwrap();
        // 1 (collection) + 3 (in values).
        assert_eq!(c.params.len(), 4);
    }

    #[test]
    fn empty_in_is_rejected() {
        let q = plan(json!({"from": "t", "where": ["status", "in", []]}));
        let err = compile_select(&q).unwrap_err();
        assert_eq!(err.code(), "QueryError");
    }

    // --- coercion rule: no type coercion (query-dsl.md §Result) -----------

    #[test]
    fn range_op_rejects_non_numeric_operand() {
        // `"2" > 10`-style: a string operand to a range op is a coercion error.
        let q = plan(json!({"from": "t", "where": ["amount", ">", "10"]}));
        let err = compile_select(&q).unwrap_err();
        assert_eq!(err.code(), "QueryError", "{err}");
    }

    #[test]
    fn range_op_accepts_numeric_operand() {
        let q = plan(json!({"from": "t", "where": ["amount", ">=", 10]}));
        assert!(compile_select(&q).is_ok());
    }

    // --- ordering: spec total order --------------------------------------

    fn env_with(id: &str, field: &str, value: serde_json::Value) -> RecordEnvelope {
        let mut fields = std::collections::BTreeMap::new();
        if !value.is_null() {
            fields.insert(field.to_string(), value);
        }
        RecordEnvelope {
            envelope_version: 1,
            entity_id: forge_domain::RecordId::new(id),
            collection: forge_domain::CollectionId::new("t"),
            fields,
            field_ids: Default::default(),
            unknown_fields: Default::default(),
            extensions: Default::default(),
            created_at: Default::default(),
            updated_at: Default::default(),
            deleted: false,
        }
    }

    fn rows(items: Vec<RecordEnvelope>) -> Vec<QueryRow> {
        items
            .into_iter()
            .map(|e| QueryRow {
                id: e.entity_id.as_str().to_string(),
                envelope: e,
            })
            .collect()
    }

    /// A display-name [`FieldRef`] for terse test construction.
    fn name(s: &str) -> FieldRef {
        FieldRef::Name(s.to_string())
    }

    #[test]
    fn order_is_numbers_then_strings_then_bools_then_nulls() {
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Asc,
        });
        let input = rows(vec![
            env_with("e_null", "v", json!(null)),
            env_with("e_bool", "v", json!(true)),
            env_with("e_str", "v", json!("a")),
            env_with("e_num", "v", json!(5)),
        ]);
        let out = finalize_rows(input, &q);
        let ids: Vec<_> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["e_num", "e_str", "e_bool", "e_null"]);
    }

    #[test]
    fn entity_id_tie_break_is_always_ascending_even_for_desc() {
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Desc,
        });
        // Same primary value (1); ids must still ascend within the tie.
        let input = rows(vec![
            env_with("b", "v", json!(1)),
            env_with("a", "v", json!(1)),
        ]);
        let out = finalize_rows(input, &q);
        let ids: Vec<_> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
    }

    #[test]
    fn offset_then_limit_applied_after_order() {
        let mut q = Query::from("t");
        q.order_by = Some(OrderBy {
            field: name("v"),
            dir: Dir::Asc,
        });
        q.offset = Some(1);
        q.limit = Some(2);
        let input = rows(vec![
            env_with("e1", "v", json!(1)),
            env_with("e2", "v", json!(2)),
            env_with("e3", "v", json!(3)),
            env_with("e4", "v", json!(4)),
        ]);
        let out = finalize_rows(input, &q);
        let ids: Vec<_> = out.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, vec!["e2", "e3"]);
    }

    // --- aggregates -------------------------------------------------------

    #[test]
    fn aggregate_sum_avg_min_max_over_numbers() {
        let agg = Aggregate {
            count: true,
            sum: Some(name("v")),
            avg: Some(name("v")),
            min: Some(name("v")),
            max: Some(name("v")),
        };
        let e1 = env_with("a", "v", json!(2));
        let e2 = env_with("b", "v", json!(4));
        let e3 = env_with("c", "v", json!(6));
        let refs = vec![&e1, &e2, &e3];
        let out = compute_aggregate(&refs, &agg);
        assert_eq!(out.count, Some(3));
        assert_eq!(out.sum, Some(12.0));
        assert_eq!(out.avg, Some(4.0));
        assert_eq!(out.min, Some(json!(2)));
        assert_eq!(out.max, Some(json!(6)));
    }

    #[test]
    fn aggregate_over_empty_is_count_zero_sum_zero_none_else() {
        let agg = Aggregate {
            count: true,
            sum: Some(name("v")),
            avg: Some(name("v")),
            min: Some(name("v")),
            max: Some(name("v")),
        };
        let out = compute_aggregate(&[], &agg);
        assert_eq!(out.count, Some(0));
        assert_eq!(out.sum, Some(0.0));
        assert_eq!(out.avg, None);
        assert_eq!(out.min, None);
        assert_eq!(out.max, None);
    }

    // --- raw-SQL rejection (DL-16) ---------------------------------------

    #[test]
    fn reject_raw_sql_blocks_ddl_dml_and_terminators() {
        for bad in [
            "DROP TABLE records",
            "SELECT * FROM t; DELETE FROM t",
            "SELECT 1 -- comment",
            "INSERT INTO t VALUES (1)",
            "UPDATE t SET x = 1",
            "PRAGMA table_info(records)",
        ] {
            assert!(reject_raw_sql(bad).is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn reject_raw_sql_allows_a_read_only_select() {
        assert!(reject_raw_sql("SELECT id FROM tasks WHERE prio > 1").is_ok());
    }
}
