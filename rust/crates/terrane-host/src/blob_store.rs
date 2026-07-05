use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest as _, Sha256};
use terrane_core::{Error, Result};

pub const BLOB_DB_NAME: &str = "blobs.sqlite3";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobHealth {
    Ok { hash: String, size: u64 },
    Missing { hash: String },
    Corrupt { hash: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobGcPlan {
    pub stale_hashes: Vec<String>,
    pub deleted: usize,
}

pub fn db_path(home: &Path) -> PathBuf {
    home.join(BLOB_DB_NAME)
}

pub fn insert_if_absent(home: &Path, hash: &str, bytes: &[u8]) -> Result<()> {
    let computed = sha256_hex(bytes);
    if computed != hash {
        return Err(Error::Storage(format!(
            "blob hash mismatch before insert: expected {hash}, got {computed}"
        )));
    }
    let conn = open(home)?;
    let size = i64::try_from(bytes.len())
        .map_err(|_| Error::Storage("blob too large for sqlite size column".into()))?;
    conn.execute(
        "INSERT OR IGNORE INTO blobs (hash, size, bytes) VALUES (?1, ?2, ?3)",
        params![hash, size, bytes],
    )
    .map_err(|e| Error::Storage(format!("insert blob: {e}")))?;
    Ok(())
}

pub fn read_verified_base64(home: &Path, hash: &str) -> Result<String> {
    let bytes = read_verified(home, hash)?;
    Ok(B64.encode(bytes))
}

pub fn read_verified(home: &Path, hash: &str) -> Result<Vec<u8>> {
    let Some((size, bytes)) = read_row(home, hash)? else {
        return Err(Error::Storage(format!("blob missing: {hash}")));
    };
    let actual_size = i64::try_from(bytes.len())
        .map_err(|_| Error::Storage("blob byte length overflow".into()))?;
    if actual_size != size {
        return Err(Error::Storage(format!(
            "blob corrupt: {hash} size metadata {size} but bytes length {actual_size}"
        )));
    }
    let actual = sha256_hex(&bytes);
    if actual != hash {
        return Err(Error::Storage(format!(
            "blob corrupt: {hash} hash verification failed (got {actual})"
        )));
    }
    Ok(bytes)
}

pub fn verify_hash(home: &Path, hash: &str) -> Result<BlobHealth> {
    match read_verified(home, hash) {
        Ok(bytes) => Ok(BlobHealth::Ok {
            hash: hash.to_string(),
            size: u64::try_from(bytes.len())
                .map_err(|_| Error::Storage("blob byte length overflow".into()))?,
        }),
        Err(Error::Storage(message)) if message.starts_with("blob missing:") => {
            Ok(BlobHealth::Missing {
                hash: hash.to_string(),
            })
        }
        Err(Error::Storage(message)) if message.starts_with("blob corrupt:") => {
            Ok(BlobHealth::Corrupt {
                hash: hash.to_string(),
                reason: message,
            })
        }
        Err(e) => Err(e),
    }
}

pub fn verify_hashes(
    home: &Path,
    hashes: impl IntoIterator<Item = String>,
) -> Result<Vec<BlobHealth>> {
    let mut out = Vec::new();
    for hash in hashes {
        out.push(verify_hash(home, &hash)?);
    }
    Ok(out)
}

pub fn gc(home: &Path, live_hashes: &BTreeSet<String>, dry_run: bool) -> Result<BlobGcPlan> {
    let conn = open(home)?;
    let mut stmt = conn
        .prepare("SELECT hash FROM blobs ORDER BY hash")
        .map_err(|e| Error::Storage(format!("query blob hashes: {e}")))?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| Error::Storage(format!("query blob hashes: {e}")))?;
    let mut stale_hashes = Vec::new();
    for row in rows {
        let hash = row.map_err(|e| Error::Storage(format!("read blob hash: {e}")))?;
        if !live_hashes.contains(&hash) {
            stale_hashes.push(hash);
        }
    }
    let mut deleted = 0usize;
    if !dry_run {
        for hash in &stale_hashes {
            deleted += conn
                .execute("DELETE FROM blobs WHERE hash = ?1", params![hash])
                .map_err(|e| Error::Storage(format!("delete blob: {e}")))?;
        }
    }
    Ok(BlobGcPlan {
        stale_hashes,
        deleted,
    })
}

pub fn copy_hashes_from_home(
    local_home: &Path,
    source_home: &Path,
    hashes: &[String],
) -> Result<usize> {
    let local = open(local_home)?;
    let source_path = db_path(source_home);
    if !source_path.exists() {
        return Ok(0);
    }
    let mut copied = 0usize;
    for hash in hashes {
        let Some((_, bytes)) = read_row(source_home, hash)? else {
            continue;
        };
        let before = local
            .query_row("SELECT 1 FROM blobs WHERE hash = ?1", params![hash], |_| {
                Ok(())
            })
            .optional()
            .map_err(|e| Error::Storage(format!("check local blob: {e}")))?;
        insert_if_absent(local_home, hash, &bytes)?;
        if before.is_none() {
            copied += 1;
        }
    }
    Ok(copied)
}

fn read_row(home: &Path, hash: &str) -> Result<Option<(i64, Vec<u8>)>> {
    let conn = open(home)?;
    conn.query_row(
        "SELECT size, bytes FROM blobs WHERE hash = ?1",
        params![hash],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
    )
    .optional()
    .map_err(|e| Error::Storage(format!("read blob: {e}")))
}

fn open(home: &Path) -> Result<Connection> {
    std::fs::create_dir_all(home).map_err(|e| Error::Storage(format!("create blob home: {e}")))?;
    let conn = Connection::open(db_path(home))
        .map_err(|e| Error::Storage(format!("open blob sqlite: {e}")))?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS blobs (
            hash  TEXT PRIMARY KEY,
            size  INTEGER NOT NULL,
            bytes BLOB NOT NULL
        ) STRICT;
        PRAGMA user_version = 1;",
    )
    .map_err(|e| Error::Storage(format!("init blob sqlite schema: {e}")))
}

fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
