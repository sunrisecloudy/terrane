use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{state_ref, AppId, Error, EventRecord, Result, StateStore};

use crate::{delete_event, MAX_SCAN_LIMIT, RESERVED_PREFIX};

/// The physical storage engine selected for a logical `kv` store.
///
/// App bundles never see this. Apps ask for the logical `kv` resource; users
/// and hosts bind that resource to a storage backend and location.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum KvStorageBackend {
    Memory,
    Sqlite,
    RocksDb,
}

impl KvStorageBackend {
    pub fn as_str(&self) -> &'static str {
        match self {
            KvStorageBackend::Memory => "memory",
            KvStorageBackend::Sqlite => "sqlite",
            KvStorageBackend::RocksDb => "rocksdb",
        }
    }

    pub fn required_feature(&self) -> Option<&'static str> {
        match self {
            KvStorageBackend::Memory => None,
            KvStorageBackend::Sqlite => Some("sqlite-storage"),
            KvStorageBackend::RocksDb => Some("rocksdb-storage"),
        }
    }

    pub fn is_available(&self) -> bool {
        match self {
            KvStorageBackend::Memory => true,
            KvStorageBackend::Sqlite => cfg!(feature = "sqlite-storage"),
            KvStorageBackend::RocksDb => cfg!(feature = "rocksdb-storage"),
        }
    }

    pub fn ensure_available(&self) -> Result<()> {
        if self.is_available() {
            return Ok(());
        }
        let feature = self.required_feature().unwrap_or("unknown");
        Err(Error::InvalidInput(format!(
            "kv storage backend {} requires feature {feature}",
            self.as_str()
        )))
    }
}

impl fmt::Display for KvStorageBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for KvStorageBackend {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "memory" => Ok(KvStorageBackend::Memory),
            "sqlite" => Ok(KvStorageBackend::Sqlite),
            "rocksdb" | "rockdb" => Ok(KvStorageBackend::RocksDb),
            other => Err(Error::InvalidInput(format!(
                "unknown kv storage backend: {other}"
            ))),
        }
    }
}

/// User-owned binding from logical `kv` to a physical backend/location.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct KvStorageBinding {
    pub backend: KvStorageBackend,
    pub path: Option<String>,
}

impl Default for KvStorageBinding {
    fn default() -> Self {
        KvStorageBinding {
            backend: KvStorageBackend::Memory,
            path: None,
        }
    }
}

impl KvStorageBinding {
    pub fn new(backend: KvStorageBackend, path: Option<String>) -> Result<Self> {
        if matches!(&path, Some(path) if path.trim().is_empty()) {
            return Err(Error::InvalidInput(
                "kv storage path must not be empty".into(),
            ));
        }
        Ok(KvStorageBinding { backend, path })
    }

    pub fn describe(&self) -> String {
        match &self.path {
            Some(path) => format!("{} at {}", self.backend, path),
            None => self.backend.to_string(),
        }
    }

    pub fn resolved_path(&self, home: &Path) -> Option<PathBuf> {
        let default_name = match self.backend {
            KvStorageBackend::Memory => return None,
            KvStorageBackend::Sqlite => "kv.sqlite3",
            KvStorageBackend::RocksDb => "kv.rocksdb",
        };
        let configured = self.path.as_deref().unwrap_or(default_name);
        let path = PathBuf::from(configured);
        if path.is_absolute() {
            Some(path)
        } else {
            Some(home.join(path))
        }
    }
}

/// This capability's storage selection state.
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct KvStorageState {
    pub default: KvStorageBinding,
    pub apps: BTreeMap<AppId, KvStorageBinding>,
}

impl KvStorageState {
    pub fn binding_for(&self, app: Option<&str>) -> KvStorageBinding {
        app.and_then(|id| self.apps.get(id))
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }
}

/// Core-facing projection plan owned by the kv capability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvStoragePlan {
    pub default: KvStorageBinding,
    pub apps: BTreeMap<AppId, KvStorageBinding>,
}

/// This capability's slice of State: per-app key/value maps plus user-owned
/// storage bindings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KvState {
    pub data: BTreeMap<AppId, BTreeMap<String, String>>,
    pub storage: KvStorageState,
}

pub fn is_reserved_key(key: &str) -> bool {
    key.starts_with(RESERVED_PREFIX)
}

pub fn get_value(state: &dyn StateStore, app: &str, key: &str) -> Result<Option<String>> {
    Ok(state_ref::<KvState>(state, "kv")?
        .data
        .get(app)
        .and_then(|m| m.get(key).cloned()))
}

pub fn scan_prefix(
    state: &dyn StateStore,
    app: &str,
    prefix: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    let limit = bounded_limit(limit);
    let mut out = Vec::new();
    let Some(map) = state_ref::<KvState>(state, "kv")?.data.get(app) else {
        return Ok(out);
    };
    for (key, value) in map.range(prefix.to_string()..) {
        if !key.starts_with(prefix) || out.len() >= limit {
            break;
        }
        out.push((key.clone(), value.clone()));
    }
    Ok(out)
}

pub fn scan_range(
    state: &dyn StateStore,
    app: &str,
    start: &str,
    end_exclusive: &str,
    limit: usize,
) -> Result<Vec<(String, String)>> {
    let limit = bounded_limit(limit);
    let mut out = Vec::new();
    let Some(map) = state_ref::<KvState>(state, "kv")?.data.get(app) else {
        return Ok(out);
    };
    for (key, value) in map.range(start.to_string()..end_exclusive.to_string()) {
        if out.len() >= limit {
            break;
        }
        out.push((key.clone(), value.clone()));
    }
    Ok(out)
}

pub fn delete_prefix_events(
    state: &dyn StateStore,
    app: &str,
    prefix: &str,
    limit: usize,
) -> Result<Vec<EventRecord>> {
    scan_prefix(state, app, prefix, limit)?
        .into_iter()
        .map(|(key, _)| delete_event(app.to_string(), key))
        .collect()
}

pub(crate) fn bounded_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_SCAN_LIMIT)
}

/// Storage plan for core/host projection setup.
pub fn storage_plan(state: &dyn StateStore) -> Result<KvStoragePlan> {
    let kv = state_ref::<KvState>(state, "kv")?;
    Ok(KvStoragePlan {
        default: kv.storage.default.clone(),
        apps: kv.storage.apps.clone(),
    })
}

/// Effective storage binding for one app, falling back to the workspace default.
pub fn storage_binding(state: &dyn StateStore, app: Option<&str>) -> Result<KvStorageBinding> {
    Ok(state_ref::<KvState>(state, "kv")?.storage.binding_for(app))
}
