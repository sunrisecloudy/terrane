use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest as _, Sha256};

use crate::{Error, Result};

const SNAPSHOT_HEADER: &[u8] = b"TRNSNAP\x01\n";
const SNAPSHOT_VERSION: u32 = 1;
pub(crate) const SNAPSHOT_NAME: &str = "snapshot.bin";
pub(crate) const ARCHIVE_NAME: &str = "log.bin.archive";

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotHeader {
    pub format_version: u32,
    pub seq: u64,
    pub log_head_hash: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct SnapshotSection {
    pub namespace: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFile {
    pub header: SnapshotHeader,
    pub sections: Vec<SnapshotSection>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct SnapshotBody {
    header: SnapshotHeader,
    sections: Vec<SnapshotSection>,
}

pub(crate) fn snapshot_path(log_path: &Path) -> PathBuf {
    log_path.with_file_name(SNAPSHOT_NAME)
}

pub(crate) fn archive_path(log_path: &Path) -> PathBuf {
    log_path.with_file_name(ARCHIVE_NAME)
}

pub(crate) fn tmp_snapshot_path(log_path: &Path) -> PathBuf {
    log_path.with_file_name("snapshot.bin.tmp")
}

pub(crate) fn tmp_log_path(log_path: &Path) -> PathBuf {
    log_path.with_file_name("log.bin.tmp")
}

pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn write_snapshot(
    path: &Path,
    seq: u64,
    log_head_hash: [u8; 32],
    sections: Vec<SnapshotSection>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| Error::Storage(e.to_string()))?;
        }
    }
    let body = SnapshotBody {
        header: SnapshotHeader {
            format_version: SNAPSHOT_VERSION,
            seq,
            log_head_hash,
        },
        sections,
    };
    let bytes = borsh::to_vec(&body).map_err(|e| Error::Storage(format!("snapshot encode: {e}")))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
        .map_err(|e| Error::Storage(e.to_string()))?;
    file.write_all(SNAPSHOT_HEADER)
        .map_err(|e| Error::Storage(e.to_string()))?;
    file.write_all(&bytes)
        .map_err(|e| Error::Storage(e.to_string()))?;
    file.sync_all().map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}

pub fn read_snapshot(path: &Path) -> Result<Option<SnapshotFile>> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(Error::Storage(e.to_string())),
    };
    let mut header = vec![0u8; SNAPSHOT_HEADER.len()];
    file.read_exact(&mut header)
        .map_err(|e| Error::Storage(format!("snapshot header: {e}")))?;
    if header != SNAPSHOT_HEADER {
        return Err(Error::Storage("snapshot header mismatch".into()));
    }
    let mut body = Vec::new();
    file.read_to_end(&mut body)
        .map_err(|e| Error::Storage(e.to_string()))?;
    let decoded = borsh::from_slice::<SnapshotBody>(&body)
        .map_err(|e| Error::Storage(format!("snapshot decode: {e}")))?;
    if decoded.header.format_version != SNAPSHOT_VERSION {
        return Err(Error::Storage(format!(
            "unsupported snapshot format version {}",
            decoded.header.format_version
        )));
    }
    Ok(Some(SnapshotFile {
        header: decoded.header,
        sections: decoded.sections,
    }))
}
