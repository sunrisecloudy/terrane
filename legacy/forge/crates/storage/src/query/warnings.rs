//! Planner full-scan warnings + the index-aware [`PlannedQuery`] outcome
//! (`dynamic-indexes.md` §Full-Scan Warnings).

use super::{FieldRef, QueryResult};

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
