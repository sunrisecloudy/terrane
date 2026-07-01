//! Cross-process single-writer guard for a Terrane home.
//!
//! Any host that opens a [`Core`](crate::Core) — a long-lived one like the MCP
//! server or a short-lived CLI command — takes an exclusive advisory lock on
//! `<log>.lock`, held for the `Core`'s lifetime. A second *process* opening the
//! same home fails fast instead of corrupting the shared event log (two
//! independent in-memory States diverging, or torn appends). This is what makes
//! the live Core the single source of truth.
//!
//! Opens *within one process* share a single OS lock through a process-global
//! registry, so tests and short reopens (seed-then-verify, replay checks) do not
//! self-conflict — only true cross-process contention is rejected.

use std::collections::HashMap;
use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::{Error, Result};

static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Weak<LockHandle>>>> = OnceLock::new();

fn registry() -> &'static Mutex<HashMap<PathBuf, Weak<LockHandle>>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A held home lock. The last `Arc` to drop releases the OS lock and clears the
/// registry slot.
pub struct LockHandle {
    key: PathBuf,
    file: File,
}

impl Drop for LockHandle {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        if let Ok(mut map) = registry().lock() {
            // Remove only if the slot is still ours (no live handle replaced it).
            if map.get(&self.key).is_some_and(|weak| weak.strong_count() == 0) {
                map.remove(&self.key);
            }
        }
    }
}

/// Acquire (or, within this process, share) the exclusive lock guarding the home
/// that owns `log_path`. Returns a [`Error::Storage`] if another process holds it.
pub fn acquire(log_path: &Path) -> Result<Arc<LockHandle>> {
    let lock_path = lock_path_for(log_path);
    let key = canonical_key(&lock_path)?;
    // Hold the registry mutex across the OS-lock attempt so two threads racing to
    // open the same home cannot each create a separate lock file description.
    let mut map = registry()
        .lock()
        .map_err(|_| Error::Storage("home-lock registry poisoned".into()))?;
    if let Some(existing) = map.get(&key).and_then(Weak::upgrade) {
        return Ok(existing);
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|e| Error::Storage(format!("open home lock {}: {e}", lock_path.display())))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            let home = log_path.parent().unwrap_or(log_path);
            return Err(Error::Storage(format!(
                "another terrane process holds {} — stop it or approve in the running session",
                home.display()
            )));
        }
        Err(TryLockError::Error(e)) => {
            return Err(Error::Storage(format!(
                "lock home {}: {e}",
                lock_path.display()
            )));
        }
    }
    let handle = Arc::new(LockHandle {
        key: key.clone(),
        file,
    });
    map.insert(key, Arc::downgrade(&handle));
    Ok(handle)
}

fn lock_path_for(log_path: &Path) -> PathBuf {
    let mut raw = log_path.as_os_str().to_owned();
    raw.push(".lock");
    PathBuf::from(raw)
}

/// Canonicalize the parent directory (created if needed) and rejoin the lock file
/// name, so one home maps to one stable key regardless of how its path was
/// spelled. The lock file itself need not exist yet.
fn canonical_key(lock_path: &Path) -> Result<PathBuf> {
    let parent = lock_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| Error::Storage(e.to_string()))?;
    let canon = parent
        .canonicalize()
        .map_err(|e| Error::Storage(format!("canonicalize {}: {e}", parent.display())))?;
    let name = lock_path
        .file_name()
        .ok_or_else(|| Error::Storage("home lock path has no file name".into()))?;
    Ok(canon.join(name))
}
