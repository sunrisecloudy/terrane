//! Planner: compile the AST to a **parameterized** SQLite SELECT over the
//! `records` projection (DL-16 — record values are always bound, never
//! interpolated into the statement text).

use super::{FieldRef, Filter, Op, Predicate, Query};
use forge_domain::{CoreError, Result};

/// A compiled, parameterized SELECT: the statement text plus the ordered bind
/// values. Record values live **only** in `params`, never in `sql` (DL-16).
#[derive(Debug, Clone)]
pub struct CompiledSelect {
    pub sql: String,
    pub params: Vec<serde_json::Value>,
}

/// The JSON path a field reference resolves to in the envelope: `$.fields.<name>`
/// for a display name, `$.field_ids.<id>` for a stable id. The inner identifier
/// is validated by [`validate_ident`](super::validate_ident) before reaching here.
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

/// A single equality term `<col> = ?N`, type-guarded so a JSON boolean operand
/// cannot coerce-match a numeric `0`/`1`.
///
/// `json_extract` (and a bound `serde_json::Bool`) both render a JSON boolean as
/// the SQL integers `0`/`1`, which collide with a stored JSON number `0`/`1`
/// (verified against SQLite's json1). `json_type` *does* distinguish them
/// (`'true'`/`'false'` vs `'integer'`/`'real'`), so when the operand is a boolean
/// we require the stored value to itself be a JSON boolean. This enforces the
/// query-dsl.md §Result rule that comparisons do not coerce types — the same rule
/// that makes `"2" > 10` an error — for the canonical `f.done.eq(false)` case.
fn eq_term(col: &str, ty: &str, value: &serde_json::Value, bind: usize) -> String {
    if value.is_boolean() {
        format!("({ty} IN ('true','false') AND {col} = ?{bind})")
    } else {
        format!("{col} = ?{bind}")
    }
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
            let ty = type_expr(&p.field);
            // NULL/missing never equals a concrete value; for `ne` it should be
            // true (the value differs), so guard explicitly. Booleans carry an
            // extra json_type guard so `eq(false)` does not also match a stored
            // numeric `0` (and a stored `0` *does* differ for `ne(false)`).
            if p.op == Op::Eq {
                Ok(eq_term(&col, &ty, &p.value, bind))
            } else if p.value.is_boolean() {
                Ok(format!(
                    "({col} IS NULL OR {ty} NOT IN ('true','false') OR {col} <> ?{bind})"
                ))
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
            // `in` is a disjunction of type-guarded equality terms (the same
            // [`eq_term`] eq uses), so a boolean member cannot coerce-match a
            // stored numeric `0`/`1`. A plain `col IN (...)` would reintroduce the
            // bool/number collision because json1 renders both as integer `0`/`1`.
            let ty = type_expr(&p.field);
            let mut terms = Vec::with_capacity(arr.len());
            for val in arr {
                if val.is_null() || val.is_array() || val.is_object() {
                    return Err(CoreError::QueryError(format!(
                        "`in` on field '{}' requires scalar values",
                        p.field.as_str()
                    )));
                }
                let bind = bind_index(params, val)?;
                terms.push(eq_term(&col, &ty, val, bind));
            }
            Ok(format!("({})", terms.join(" OR ")))
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
