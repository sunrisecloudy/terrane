//! Host-side per-app logging ring buffer.
//!
//! Lives at `$TERRANE_HOME/logs/<app>/current.jsonl`, rotated at
//! [`ROTATE_BYTES`] into `1.jsonl` … `3.jsonl` (oldest dropped); one line per
//! entry: `{ts, level, msg, data, source?}`. Written only by this host edge;
//! the core never opens it (same stance as `blobs.sqlite3`). Fold never reads
//! the filesystem — replay reproduces `TelemetryState` from the recorded
//! `telemetry.error` events alone.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use terrane_core::{Error, Result};

use terrane_cap_telemetry::{RING_ROTATE_BYTES, RING_ROTATE_KEEP};

pub fn app_log_dir(home: &Path) -> PathBuf {
    home.join("logs")
}

pub fn app_dir(home: &Path, app: &str) -> PathBuf {
    app_log_dir(home).join(app)
}

fn current_path(dir: &Path) -> PathBuf {
    dir.join("current.jsonl")
}

fn rotated_path(dir: &Path, n: usize) -> PathBuf {
    dir.join(format!("{n}.jsonl"))
}

/// Wall-clock epoch-ms for the one-line jsonl `ts`. Fine for the buffer —
/// nothing folds from it.
fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Escape a string for safe inclusion in a JSON line — newlines, quotes,
/// backslashes, and control chars all encoded so each entry is a single line.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Write one entry. Optional `source` extends the line with `,"source":"…"`.
/// Rotation is checked after the write so the cap survives one cap-sized write.
pub fn append(home: &Path, app: &str, level: &str, msg: &str, data: &str) -> Result<()> {
    let dir = app_dir(home, app);
    fs::create_dir_all(&dir).map_err(|e| Error::Storage(e.to_string()))?;
    let path = current_path(&dir);
    let ts = now_epoch_ms();
    let source_field = match level {
        "error" => ",\"source\":\"explicit\"".to_string(),
        _ => String::new(),
    };
    // `data` arrives from the cap already as `"{}"` (canonical) or a truncated
    // arbitrary string; we wrap only its raw form as a JSON string so the
    // buffer is always single-line jsonl. Indexers wanting structured data
    // parse the `data` field separately.
    let line = format!(
        "{{\"ts\":{ts},\"level\":\"{}\",\"msg\":\"{}\",\"data\":\"{}\"{}}}\n",
        json_escape(level),
        json_escape(msg),
        json_escape(data),
        source_field,
    );

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| Error::Storage(e.to_string()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| Error::Storage(e.to_string()))?;
    drop(file);

    rotate_if_needed(&dir)?;
    Ok(())
}

/// Write one entry with an explicit `source` (auto-capture path). The line
/// never records an event here; the caller (the sandbox auto-capture path)
/// separately pushes a `telemetry.error` event if the app grants telemetry.
pub fn append_with_source(
    home: &Path,
    app: &str,
    level: &str,
    msg: &str,
    data: &str,
    source: &str,
) -> Result<()> {
    let dir = app_dir(home, app);
    fs::create_dir_all(&dir).map_err(|e| Error::Storage(e.to_string()))?;
    let path = current_path(&dir);
    let ts = now_epoch_ms();
    let line = format!(
        "{{\"ts\":{ts},\"level\":\"{}\",\"msg\":\"{}\",\"data\":\"{}\",\"source\":\"{}\"}}\n",
        json_escape(level),
        json_escape(msg),
        json_escape(data),
        json_escape(source),
    );
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| Error::Storage(e.to_string()))?;
    file.write_all(line.as_bytes())
        .map_err(|e| Error::Storage(e.to_string()))?;
    drop(file);
    rotate_if_needed(&dir)?;
    Ok(())
}

fn rotate_if_needed(dir: &Path) -> Result<()> {
    let path = current_path(dir);
    let size = match fs::metadata(&path) {
        Ok(meta) => meta.len(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::Storage(e.to_string())),
    };
    if size < RING_ROTATE_BYTES {
        return Ok(());
    }
    // Drop the oldest, shift the rest up: 3 -> gone, 2 -> 3, 1 -> 2,
    // current -> 1, then current is truncated on next append.
    let oldest = rotated_path(dir, RING_ROTATE_KEEP);
    if oldest.exists() {
        let _ = fs::remove_file(&oldest);
    }
    for n in (1..RING_ROTATE_KEEP).rev() {
        let from = rotated_path(dir, n);
        let to = rotated_path(dir, n + 1);
        if from.exists() {
            fs::rename(&from, &to).map_err(|e| Error::Storage(e.to_string()))?;
        }
    }
    let first = rotated_path(dir, 1);
    fs::rename(&path, &first).map_err(|e| Error::Storage(e.to_string()))?;
    Ok(())
}

/// Tail-read this app's log buffer, newest last, optionally filtered by level.
/// `tail` caps the number of entries returned. Returns a JSON document
/// `{"lines":[{"ts":...,"level":"info","msg":"…","data":"…","source":"…"}]}`.
pub fn read_tail(
    home: &Path,
    app: &str,
    level: &str,
    tail: usize,
) -> Result<String> {
    let dir = app_dir(home, app);

    // Read oldest-first: rotated files KEEP..1 (oldest first), then current.
    let mut entries: Vec<String> = Vec::new();
    let files: Vec<PathBuf> = (1..=RING_ROTATE_KEEP)
        .rev()
        .map(|n| rotated_path(&dir, n))
        .filter(|p| p.exists())
        .chain(std::iter::once(current_path(&dir)))
        .collect();

    for file in files {
        let Ok(content) = fs::read_to_string(&file) else {
            continue;
        };
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            if !level.is_empty() && !line_contains_level(line, level) {
                continue;
            }
            entries.push(line.to_string());
        }
    }

    let start = entries.len().saturating_sub(tail.max(1));
    let slice = &entries[start..];
    let joined = slice
        .iter()
        .map(|l| l.to_string())
        .collect::<Vec<_>>()
        .join(",");
    Ok(format!("{{\"lines\":[{joined}]}}"))
}

fn line_contains_level(line: &str, level: &str) -> bool {
    let needle = format!("\"level\":\"{level}\"");
    line.contains(&needle)
}

/// Delete the per-app log directory when an app is removed. Idempotent.
pub fn delete_app_logs(home: &Path, app: &str) -> Result<()> {
    let dir = app_dir(home, app);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| Error::Storage(e.to_string()))?;
    }
    Ok(())
}
