//! The query DSL AST: operators, field references, filter tree, aggregate,
//! ordering, text search, and the top-level [`Query`] (DL-15/16/17).

use super::json_path::{field_id_json_path, quote_json_path_key};

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

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Asc,
    Desc,
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
    /// name is validated by [`validate_ident`](super::validate_ident) before
    /// reaching here, so the returned path can never carry SQL.
    ///
    /// The leaf identifier is **double-quoted** (`$.field_ids."<id>"`) so a
    /// stable field id that contains a `.` (e.g. `f_dev.01_0`, mintable from an
    /// actor id like `dev.01`) addresses the literal key instead of being parsed
    /// by JSON1 as nested keys (`field_ids.f_dev.01.0`), which would silently
    /// read `NULL`. The allowlist forbids a `"` in the identifier, so the quote
    /// cannot be escaped; we still double any quote defensively.
    pub(crate) fn json_path(&self) -> String {
        match self {
            FieldRef::Name(s) => format!("$.fields.{}", quote_json_path_key(s)),
            // Delegate the `$.field_ids."<id>"` construction to the single helper
            // the index DDL / FTS population / text-search scan also use, so the
            // stable-id path quoting is byte-identical on every surface.
            FieldRef::Id(s) => field_id_json_path(s),
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
