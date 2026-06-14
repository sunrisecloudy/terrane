//! Bundle/target write-transaction wrapper + the table-presence probe used by
//! the fresh-target check (review 061 P1 — all-or-nothing copies).

use crate::map_sql;
use forge_domain::Result;
use rusqlite::Connection;

/// Run `f` against `conn` inside one SQLite transaction, committing iff `f`
/// returns `Ok` and rolling back on any error (review 061 P1: the table copies
/// are all-or-nothing, never a partially populated bundle/target). Uses
/// `unchecked_transaction` because the copy helpers borrow the connection by
/// shared reference; the bundle/target connection is freshly created and owned by
/// this call, so no other handle is mutating it concurrently.
pub(super) fn in_transaction<F>(conn: &Connection, f: F) -> Result<()>
where
    F: FnOnce(&Connection) -> Result<()>,
{
    let tx = conn.unchecked_transaction().map_err(map_sql)?;
    f(&tx)?;
    tx.commit().map_err(map_sql)?;
    Ok(())
}

/// True iff `table` holds at least one row (a presence probe for
/// [`is_empty_target`](crate::Store::is_empty_target)). `table` is always one of
/// this module's FIXED table-name literals — never caller/user input — so
/// formatting it into the statement carries no injection surface; the
/// `EXISTS`/`LIMIT 1` shape lets SQLite stop at the first row instead of counting
/// the whole table.
pub(super) fn table_has_any_row(conn: &Connection, table: &str) -> Result<bool> {
    conn.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {table} LIMIT 1)"),
        [],
        |row| row.get::<_, i64>(0),
    )
    .map_err(map_sql)
    .map(|exists| exists != 0)
}
