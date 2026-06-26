//! Fixture/plan (de)serialization into the [`Query`] AST, plus the identifier
//! allowlist that keeps field/collection names out of the SQL structure (DL-16).

use super::{Aggregate, Dir, FieldRef, Filter, Op, OrderBy, Predicate, Query, TextSearch};
use forge_domain::{CoreError, Result};

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

/// Validate that a field/collection name is a safe identifier before it is
/// placed into the JSON path of an (otherwise constant) statement. Names are
/// caller-supplied, so they must never be able to carry SQL or break out of the
/// `$.fields.<name>` path (DL-16). Allowed: ASCII alphanumerics, `_`, `-`, `.`
/// and `/` (the latter two appear in entity ids / nested display names).
pub(super) fn validate_ident(kind: &str, name: &str) -> Result<()> {
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
        .ok_or_else(|| {
            CoreError::QueryError("field reference missing 'field'/'field_id'".into())
        })?;
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
            Ok(Filter::And(
                items.iter().map(parse_filter).collect::<Result<_>>()?,
            ))
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
