//! Identifier + raw-SQL guards (DL-16): the index-definition identifier check
//! and the SQL-like string rejection that keeps caller SQL out of the engine.

use super::parse::validate_ident;
use forge_domain::{CoreError, Result};

/// Validate a `collection`/`field_id` before it is placed into index DDL (the
/// index name and the partial-predicate collection literal). Same allowlist as
/// [`validate_ident`]; exposed to the index module so a hostile identifier is
/// refused at definition time rather than interpolated into structure.
pub(crate) fn validate_index_ident(kind: &str, name: &str) -> Result<()> {
    validate_ident(kind, name)
}

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
        "insert ",
        "update ",
        "delete ",
        "drop ",
        "alter ",
        "create ",
        "pragma ",
        "attach ",
        "detach ",
        "replace ",
        "vacuum ",
        "reindex ",
        "truncate ",
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
