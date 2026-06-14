//! Result model, the spec total-order (`finalize_rows`), grouping keys, and the
//! numeric aggregate reducer (query-dsl.md §Result).

use super::{Aggregate, Dir, FieldRef, Query};
use forge_domain::RecordEnvelope;
use std::cmp::Ordering;

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

/// Whether a JSON value is an orderable scalar (number/string/bool). Null and
/// non-scalar arrays/objects are NOT orderable and sort LAST (independent of
/// direction) per query-dsl.md §Result.
fn is_orderable(v: &serde_json::Value) -> bool {
    matches!(
        v,
        serde_json::Value::Number(_) | serde_json::Value::String(_) | serde_json::Value::Bool(_)
    )
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

/// Whether `field` is the entity-id sort key (`id` / `entity_id`), which sorts
/// directly by the stable `records.id` rather than a `$.fields`/`$.field_ids`
/// value. Only meaningful as a display name (`field_ids` never names `id`).
fn is_entity_id_key(field: &FieldRef) -> bool {
    matches!(field, FieldRef::Name(n) if n == "id" || n == "entity_id")
}

/// Apply ordering (spec total order), then offset, then limit, in Rust so the
/// result is platform-stable. `id` is always the secondary tie-break.
///
/// Two direction rules from query-dsl.md §Result are kept *separate* so a
/// descending sort does not corrupt either:
///
/// - **Nulls (and other non-orderable values) sort LAST regardless of
///   direction.** A naive `primary.reverse()` for `desc` would also reverse the
///   null rank and float nulls to the front; instead, present-vs-absent is a
///   higher-priority discriminator that is never reversed, and only the
///   value-vs-value comparison flips for `desc`.
/// - **The `entity_id` tie-break is always ascending** for a value sort, so ties
///   are stable independent of direction. The exception is an explicit
///   `orderBy("id"/"entity_id", …)`: there the entity id *is* the primary key,
///   so `desc` reverses it (it is a real sortable key, not a no-op tie-break).
pub fn finalize_rows(mut rows: Vec<QueryRow>, q: &Query) -> Vec<QueryRow> {
    if let Some(ob) = &q.order_by {
        let desc = ob.dir == Dir::Desc;
        if is_entity_id_key(&ob.field) {
            // The entity id is the real sort key: honor the direction (a `desc`
            // id sort must actually descend, not collapse to the ascending
            // tie-break).
            rows.sort_by(|a, b| {
                let primary = a.id.cmp(&b.id);
                if desc {
                    primary.reverse()
                } else {
                    primary
                }
            });
        } else {
            rows.sort_by(|a, b| {
                let va = row_field(&a.envelope, &ob.field);
                let vb = row_field(&b.envelope, &ob.field);
                // Present-before-absent is fixed regardless of direction so
                // nulls/non-orderables stay LAST even for `desc`.
                let a_absent = !is_orderable(va);
                let b_absent = !is_orderable(vb);
                let presence = a_absent.cmp(&b_absent); // false(present) < true(absent)
                let value = match (a_absent, b_absent) {
                    (false, false) => {
                        let c = cmp_json(va, vb);
                        if desc {
                            c.reverse()
                        } else {
                            c
                        }
                    }
                    // At least one side is absent: presence already decided it
                    // (or both absent -> Equal), and we never reverse that.
                    _ => Ordering::Equal,
                };
                // entity_id tie-break is ALWAYS ascending (stable secondary
                // order), independent of the primary direction.
                presence.then(value).then_with(|| a.id.cmp(&b.id))
            });
        }
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
