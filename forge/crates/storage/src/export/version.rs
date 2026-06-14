//! Bundle open-read-only + format-version validation (spec §Versioning: an
//! importer refuses a version it does not understand, never silently
//! reinterprets unknown data).

use super::policy::EXPORT_FORMAT_VERSION;
use crate::map_sql;
use forge_domain::{CoreError, Result};
use rusqlite::{Connection, OptionalExtension};
use std::path::Path;

/// Open a bundle file **read-only** and validate its format version before any
/// copy. Read-only mirrors the spec ("opens the database read-only first").
pub(super) fn open_bundle_readonly(path: &Path) -> Result<Connection> {
    if !path.exists() {
        return Err(CoreError::StorageError(format!(
            "import bundle {} does not exist",
            path.display()
        )));
    }
    use rusqlite::OpenFlags;
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(map_sql)?;
    validate_bundle_version(&conn)?;
    Ok(conn)
}

/// Read and validate the bundle's `export_format_version` from its `meta` table.
/// A missing header or a version this build does not understand is a clean
/// [`CoreError::StorageError`] (spec §Versioning: importers must not silently
/// reinterpret unknown versions).
pub(super) fn validate_bundle_version(bundle: &Connection) -> Result<()> {
    let raw: Option<Vec<u8>> = bundle
        .query_row(
            "SELECT value FROM meta WHERE key = 'export_format_version'",
            [],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(map_sql)?
        .flatten();
    let bytes = raw.ok_or_else(|| {
        CoreError::StorageError(
            "bundle is missing its export_format_version header; not a forge workspace export"
                .into(),
        )
    })?;
    let text = std::str::from_utf8(&bytes)
        .map_err(|e| CoreError::StorageError(format!("export_format_version is not utf-8: {e}")))?;
    let version: i64 = text
        .parse()
        .map_err(|e| CoreError::StorageError(format!("export_format_version is malformed: {e}")))?;
    if version != EXPORT_FORMAT_VERSION {
        return Err(CoreError::StorageError(format!(
            "unsupported export_format_version {version}; this build understands {EXPORT_FORMAT_VERSION} \
             (migrate the bundle with a matching forge version)"
        )));
    }
    Ok(())
}
