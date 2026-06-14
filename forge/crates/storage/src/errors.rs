//! Shared storage error mappers.
//!
//! These helpers translate the substrate's raw failure modes —
//! `rusqlite::Error`, `serde_json::Error`, and corrupt persisted counter bytes —
//! into the spine's [`CoreError`], plus the [`CounterError`] classification the
//! `BEGIN IMMEDIATE` counter reservation uses to distinguish a retryable
//! `SQLITE_BUSY` from a permanent failure. They are used across the storage
//! crate's modules (`compaction`, `crdt_write`, `export`, and `lib`), so they
//! live here and are re-exported at the crate root.

use forge_domain::{CoreError, Result};

/// Map any `rusqlite` failure to a stable, displayable `CoreError`.
pub(crate) fn map_sql(e: rusqlite::Error) -> CoreError {
    CoreError::StorageError(e.to_string())
}

/// True iff a `rusqlite` error is a transient SQLite lock contention
/// (`SQLITE_BUSY` / `SQLITE_LOCKED`), which a serialized retry can resolve — as
/// opposed to a permanent failure (corruption, constraint, misuse) that must
/// surface. Used by [`Store::next_counter`]'s bounded retry loop.
pub(crate) fn is_busy(e: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode;
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if matches!(err.code, ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Outcome of one `BEGIN IMMEDIATE` counter reservation: either a retryable
/// lock-contention (`Busy`, carrying the raw error so the caller can surface it
/// after exhausting retries) or a permanent failure (`Fatal`, already mapped).
pub(crate) enum CounterError {
    Busy(rusqlite::Error),
    Fatal(CoreError),
}

impl CounterError {
    /// Classify a raw `rusqlite` error from the BEGIN/commit boundary: a
    /// `SQLITE_BUSY`/`SQLITE_LOCKED` is retryable, everything else is fatal.
    pub(crate) fn from_sql(e: rusqlite::Error) -> Self {
        if is_busy(&e) {
            CounterError::Busy(e)
        } else {
            CounterError::Fatal(map_sql(e))
        }
    }
}

/// Map a serde_json (de)serialization failure on the storage path.
pub(crate) fn map_json(ctx: &str, e: serde_json::Error) -> CoreError {
    CoreError::StorageError(format!("{ctx}: {e}"))
}

/// Parse a persisted counter value (utf-8 decimal `u64`) for
/// [`Store::next_counter`], surfacing a `StorageError` on corruption rather than
/// silently resetting to zero.
pub(crate) fn parse_counter_value(bytes: &[u8]) -> Result<u64> {
    let s = std::str::from_utf8(bytes)
        .map_err(|e| CoreError::StorageError(format!("counter value is not utf-8: {e}")))?;
    s.parse::<u64>()
        .map_err(|e| CoreError::StorageError(format!("counter value is malformed: {e}")))
}
