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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::FieldRef;

    #[test]
    fn dotted_field_id_addresses_the_literal_key() {
        // A field id minted from a dotted actor id (`dev.01` → `f_dev.01_0`) must
        // address the literal key, not a nested `field_ids.f_dev.01.0` path.
        assert_eq!(quote_json_path_key("f_dev.01_0"), "\"f_dev.01_0\"");
        assert_eq!(field_id_json_path("f_dev.01_0"), "$.field_ids.\"f_dev.01_0\"");
    }

    #[test]
    fn field_ref_json_path_matches_the_shared_helpers() {
        // The query AST, the index DDL/FTS, and the text-search scan all resolve a
        // FieldRef through these helpers, so the produced path must be identical.
        assert_eq!(
            FieldRef::Id("f_dev.01_0".into()).json_path(),
            field_id_json_path("f_dev.01_0")
        );
        assert_eq!(
            FieldRef::Name("status".into()).json_path(),
            format!("$.fields.{}", quote_json_path_key("status"))
        );
        // A dotted display name double-quotes the same way.
        assert_eq!(FieldRef::Name("a.b".into()).json_path(), "$.fields.\"a.b\"");
    }
}
