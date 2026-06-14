//! Canonical JSON1 path quoting for the query/index/text surfaces.
//!
//! Consolidated here (/simplify #8) so the expression-index DDL, the FTS
//! population, the text-search scan, and the query predicate all compile to the
//! **identical** JSON1 path string — a mismatch would silently disable the index.

/// Double-quote a validated JSON-path key segment per JSON1's quoting rule, so a
/// key containing a `.`/`-`/`/` addresses the literal key rather than a nested
/// path. The key is already restricted to `[A-Za-z0-9_./-]` by
/// [`validate_ident`](super::validate_ident), so it can never contain a `"`; we
/// double any quote anyway as belt-and-suspenders. Exposed to the index/text
/// modules so the expression index DDL and the query predicate compile to the
/// **identical** path string (a mismatch would silently disable the index).
pub(crate) fn quote_json_path_key(key: &str) -> String {
    format!("\"{}\"", key.replace('"', "\"\""))
}

/// The canonical `$.field_ids."<id>"` JSON path for a stable field id, with the
/// id double-quoted (see [`quote_json_path_key`]). Used by the index DDL / FTS
/// population / text-search scan so every surface reads the same JSON1 path.
pub(crate) fn field_id_json_path(field_id: &str) -> String {
    format!("$.field_ids.{}", quote_json_path_key(field_id))
}
