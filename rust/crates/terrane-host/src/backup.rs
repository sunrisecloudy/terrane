use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tar::{Archive, Builder, Header};
use tempfile::TempDir;
use terrane_core::{fold_records_in_memory, read_log, write_log, Core, NoEffects, Request, State};

use crate::{blob_store, log_path_for_home, open_at_home};

const FORMAT_VERSION: u32 = 1;
const MANIFEST: &str = "manifest.json";
const LOG: &str = "log.bin";
const SNAPSHOT: &str = "snapshot.bin";
const APPS: &str = "apps";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    #[serde(rename = "formatVersion")]
    pub format_version: u32,
    #[serde(rename = "createdAtMs")]
    pub created_at_ms: u128,
    #[serde(rename = "terraneVersion")]
    pub terrane_version: String,
    #[serde(rename = "logRecords")]
    pub log_records: usize,
    pub peer: Option<String>,
    pub kind: String,
    pub app: Option<String>,
    pub files: Vec<BackupFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupFile {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupInfo {
    pub manifest: BackupManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupCreateOutcome {
    pub manifest: BackupManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupRestoreOutcome {
    pub manifest: BackupManifest,
    pub replay_matches: bool,
    pub cloned: bool,
    pub peer: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOutcome {
    pub manifest: BackupManifest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportOutcome {
    pub app: String,
    pub records: usize,
    pub blobs: usize,
}

pub fn create_backup(home: &Path, archive_path: &Path, level: i32) -> Result<BackupCreateOutcome, String> {
    let log_path = log_path_for_home(home);
    let _lock = terrane_core::filelock::acquire(&log_path).map_err(|e| e.to_string())?;
    let temp = TempDir::new().map_err(io)?;
    let root = temp.path();
    materialize_home_backup(home, root)?;
    let core = Core::<NoEffects>::open(root.join(LOG)).map_err(|e| e.to_string())?;
    let manifest = manifest_for(root, "backup", None, &core)?;
    write_archive(root, archive_path, &manifest, level)?;
    Ok(BackupCreateOutcome { manifest })
}

pub fn backup_info(archive_path: &Path) -> Result<BackupInfo, String> {
    let file = File::open(archive_path).map_err(io)?;
    let decoder = zstd::stream::read::Decoder::new(file).map_err(io)?;
    let mut archive = Archive::new(decoder);
    let mut entries = archive.entries().map_err(io)?;
    let Some(first) = entries.next() else {
        return Err("backup archive is empty".into());
    };
    let mut entry = first.map_err(io)?;
    let path = entry.path().map_err(io)?.into_owned();
    if path != Path::new(MANIFEST) {
        return Err("backup manifest must be the first archive entry".into());
    }
    let mut json = String::new();
    entry.read_to_string(&mut json).map_err(io)?;
    let manifest = serde_json::from_str::<BackupManifest>(&json)
        .map_err(|e| format!("backup manifest json: {e}"))?;
    Ok(BackupInfo { manifest })
}

pub fn restore_backup(
    archive_path: &Path,
    into: &Path,
    clone_identity: bool,
) -> Result<BackupRestoreOutcome, String> {
    ensure_absent_or_empty(into)?;
    let temp = TempDir::new().map_err(io)?;
    let manifest = unpack_verified(archive_path, temp.path())?;
    if manifest.kind != "backup" {
        return Err(format!("archive kind is {}, not backup", manifest.kind));
    }
    copy_dir(temp.path(), into)?;
    let mut core = open_at_home(into)?;
    if clone_identity {
        core.dispatch(Request::trusted_host("replica.rotate", Vec::new()))
            .map_err(|e| e.to_string())?;
    }
    let replay_matches = core.replay_matches().map_err(|e| e.to_string())?;
    Ok(BackupRestoreOutcome {
        manifest,
        replay_matches,
        cloned: clone_identity,
        peer: core.state().replica.peer,
    })
}

pub fn export_app(home: &Path, app: &str, archive_path: &Path, level: i32) -> Result<ExportOutcome, String> {
    let core = open_at_home(home)?;
    if !core.state().app.apps.contains_key(app) {
        return Err(format!("app not found: {app}"));
    }
    let temp = TempDir::new().map_err(io)?;
    let root = temp.path();
    fs::create_dir_all(root).map_err(io)?;
    let records = core
        .log_records()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|record| core.app_of_record(record).as_deref() == Some(app))
        .collect::<Vec<_>>();
    write_log(&root.join(LOG), &records).map_err(|e| e.to_string())?;
    let source_app = home.join(APPS).join(app);
    if source_app.exists() {
        copy_dir(&source_app, &root.join(APPS).join(app))?;
    }
    let hashes = terrane_cap_blob::live_hashes_for_app(&core.state().blob, app);
    let _ = blob_store::copy_hashes_from_home(root, home, &hashes).map_err(|e| e.to_string())?;
    let sliced = Core::<NoEffects>::open(root.join(LOG)).map_err(|e| e.to_string())?;
    let manifest = manifest_for(root, "export", Some(app.to_string()), &sliced)?;
    write_archive(root, archive_path, &manifest, level)?;
    Ok(ExportOutcome { manifest })
}

pub fn import_app(home: &Path, archive_path: &Path) -> Result<ImportOutcome, String> {
    let temp = TempDir::new().map_err(io)?;
    let manifest = unpack_verified(archive_path, temp.path())?;
    if manifest.kind != "export" {
        return Err(format!("archive kind is {}, not export", manifest.kind));
    }
    let app = manifest
        .app
        .clone()
        .ok_or_else(|| "export archive is missing app id".to_string())?;
    let mut core = open_at_home(home)?;
    if core.state().app.apps.contains_key(&app) {
        return Err(format!("app already exists: {app}"));
    }
    let records = read_log(&temp.path().join(LOG)).map_err(|e| e.to_string())?;
    let records_len = records.len();
    core.append_recorded(records).map_err(|e| e.to_string())?;
    let mut state = State::default();
    let temp_records = read_log(&temp.path().join(LOG)).map_err(|e| e.to_string())?;
    fold_records_in_memory(&mut state, &temp_records).map_err(|e| e.to_string())?;
    let hashes = state.blob.refs.keys().cloned().collect::<Vec<_>>();
    let blobs = blob_store::copy_hashes_from_home(home, temp.path(), &hashes)
        .map_err(|e| e.to_string())?;
    Ok(ImportOutcome {
        app,
        records: records_len,
        blobs,
    })
}

fn materialize_home_backup(home: &Path, root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(io)?;
    let log = home.join(LOG);
    if log.exists() {
        fs::copy(&log, root.join(LOG)).map_err(io)?;
    } else {
        write_log(&root.join(LOG), &[]).map_err(|e| e.to_string())?;
    }
    let snapshot = home.join(SNAPSHOT);
    if snapshot.exists() {
        fs::copy(snapshot, root.join(SNAPSHOT)).map_err(io)?;
    }
    let apps = home.join(APPS);
    if apps.exists() {
        copy_dir(&apps, &root.join(APPS))?;
    }
    let blobs = blob_store::db_path(home);
    if blobs.exists() {
        copy_sqlite(&blobs, &root.join(blob_store::BLOB_DB_NAME))?;
    }
    Ok(())
}

fn manifest_for(
    root: &Path,
    kind: &str,
    app: Option<String>,
    core: &Core<NoEffects>,
) -> Result<BackupManifest, String> {
    let files = collect_files(root)?;
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_millis();
    Ok(BackupManifest {
        format_version: FORMAT_VERSION,
        created_at_ms,
        terrane_version: env!("CARGO_PKG_VERSION").to_string(),
        log_records: core.log_records().map_err(|e| e.to_string())?.len(),
        peer: core.state().replica.peer.map(|peer| format!("{peer:016x}")),
        kind: kind.to_string(),
        app,
        files,
    })
}

fn write_archive(
    root: &Path,
    archive_path: &Path,
    manifest: &BackupManifest,
    level: i32,
) -> Result<(), String> {
    if let Some(parent) = archive_path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(io)?;
        }
    }
    let file = File::create(archive_path).map_err(io)?;
    let encoder = zstd::stream::write::Encoder::new(file, level).map_err(io)?;
    let mut builder = Builder::new(encoder);
    let json = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    append_bytes(&mut builder, MANIFEST, &json)?;
    for file in &manifest.files {
        builder
            .append_path_with_name(root.join(&file.path), &file.path)
            .map_err(io)?;
    }
    let encoder = builder.into_inner().map_err(io)?;
    encoder.finish().map_err(io)?;
    Ok(())
}

fn unpack_verified(archive_path: &Path, root: &Path) -> Result<BackupManifest, String> {
    let file = File::open(archive_path).map_err(io)?;
    let decoder = zstd::stream::read::Decoder::new(file).map_err(io)?;
    let mut archive = Archive::new(decoder);
    archive.unpack(root).map_err(io)?;
    let manifest_path = root.join(MANIFEST);
    let manifest_json = fs::read_to_string(&manifest_path).map_err(io)?;
    let manifest = serde_json::from_str::<BackupManifest>(&manifest_json)
        .map_err(|e| format!("backup manifest json: {e}"))?;
    if manifest.format_version != FORMAT_VERSION {
        return Err(format!("unsupported backup formatVersion {}", manifest.format_version));
    }
    for file in &manifest.files {
        validate_relative(&file.path)?;
        let path = root.join(&file.path);
        let bytes = fs::read(&path).map_err(io)?;
        if u64::try_from(bytes.len()).map_err(|_| "file too large".to_string())? != file.bytes {
            return Err(format!("backup file size mismatch: {}", file.path));
        }
        let hash = sha256_hex(&bytes);
        if hash != file.sha256 {
            return Err(format!("backup file hash mismatch: {}", file.path));
        }
    }
    let records = read_log(&root.join(LOG)).map_err(|e| e.to_string())?;
    if records.len() != manifest.log_records {
        return Err("backup log record count mismatch".into());
    }
    let mut state = State::default();
    fold_records_in_memory(&mut state, &records).map_err(|e| e.to_string())?;
    let mut fresh = State::default();
    fold_records_in_memory(&mut fresh, &records).map_err(|e| e.to_string())?;
    if fresh != state {
        return Err("backup replay identity check failed".into());
    }
    Ok(manifest)
}

fn collect_files(root: &Path) -> Result<Vec<BackupFile>, String> {
    let mut out = Vec::new();
    collect_files_inner(root, root, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn collect_files_inner(root: &Path, dir: &Path, out: &mut Vec<BackupFile>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let path = entry.path();
        let meta = entry.metadata().map_err(io)?;
        if meta.is_dir() {
            collect_files_inner(root, &path, out)?;
        } else if meta.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if rel == MANIFEST {
                continue;
            }
            let bytes = fs::read(&path).map_err(io)?;
            out.push(BackupFile {
                path: rel,
                sha256: sha256_hex(&bytes),
                bytes: u64::try_from(bytes.len()).map_err(|_| "file too large".to_string())?,
            });
        }
    }
    Ok(())
}

fn append_bytes(builder: &mut Builder<zstd::stream::write::Encoder<'_, File>>, path: &str, bytes: &[u8]) -> Result<(), String> {
    let mut header = Header::new_gnu();
    header.set_size(u64::try_from(bytes.len()).map_err(|_| "manifest too large".to_string())?);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, path, bytes).map_err(io)
}

fn ensure_absent_or_empty(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    if !path.is_dir() {
        return Err(format!("restore target exists and is not a directory: {}", path.display()));
    }
    if fs::read_dir(path).map_err(io)?.next().is_some() {
        return Err(format!("restore target is not empty: {}", path.display()));
    }
    Ok(())
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    fs::create_dir_all(to).map_err(io)?;
    for entry in fs::read_dir(from).map_err(io)? {
        let entry = entry.map_err(io)?;
        let ty = entry.file_type().map_err(io)?;
        let dest = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else if ty.is_file() {
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).map_err(io)?;
            }
            fs::copy(entry.path(), dest).map_err(io)?;
        }
    }
    Ok(())
}

fn copy_sqlite(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent).map_err(io)?;
    }
    let conn = Connection::open(from).map_err(|e| format!("open sqlite for backup: {e}"))?;
    let to_sql = to.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{to_sql}'"))
        .map_err(|e| format!("copy sqlite for backup: {e}"))
}

fn validate_relative(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if p.is_absolute()
        || p.components().any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!("unsafe backup path: {path}"));
    }
    Ok(())
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

fn io(e: std::io::Error) -> String {
    e.to_string()
}
