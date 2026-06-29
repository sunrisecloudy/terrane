//! The `kv` capability — a per-app key/value store. Reacts to `app.removed` by
//! dropping that app's data.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_event, encode_event, ensure_app_exists, state_mut, state_ref, AppId, CapManifest,
    CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord, EventSpec, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod storage;

pub use storage::{sync_full_storage, sync_storage_after_commit};

/// Prefix reserved for platform-owned data inside an app's KV namespace.
pub const RESERVED_PREFIX: &str = "__terrane/";
const DEFAULT_SCAN_LIMIT: usize = 100;
const MAX_SCAN_LIMIT: usize = 500;

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

#[derive(BorshSerialize, BorshDeserialize)]
struct Set {
    app: String,
    key: String,
    value: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Deleted {
    app: String,
    key: String,
}

/// Internal helper for platform capabilities that intentionally write KV keys,
/// including reserved prefixes. Public `kv.*` commands still reject them.
pub fn set_event(
    app: impl Into<String>,
    key: impl Into<String>,
    value: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "kv.set",
        &Set {
            app: app.into(),
            key: key.into(),
            value: value.into(),
        },
    )
}

/// Internal helper for platform capabilities that intentionally delete KV keys,
/// including reserved prefixes.
pub fn delete_event(app: impl Into<String>, key: impl Into<String>) -> Result<EventRecord> {
    encode_event(
        "kv.deleted",
        &Deleted {
            app: app.into(),
            key: key.into(),
        },
    )
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

#[derive(BorshSerialize, BorshDeserialize)]
struct StorageConfigured {
    app: Option<String>,
    backend: KvStorageBackend,
    path: Option<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct StorageCleared {
    app: Option<String>,
}

pub struct KvCapability;

impl Capability for KvCapability {
    fn namespace(&self) -> &'static str {
        "kv"
    }

    /// The app-scoped key/value surface backends get on `ctx.resource.kv`.
    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "kv.set" },
                CommandSpec { name: "kv.rm" },
                CommandSpec { name: "kv.delete" },
                CommandSpec {
                    name: "kv.storage.set",
                },
                CommandSpec {
                    name: "kv.storage.clear",
                },
            ],
            events: vec![
                EventSpec { kind: "kv.set" },
                EventSpec { kind: "kv.deleted" },
                EventSpec {
                    kind: "kv.storage.configured",
                },
                EventSpec {
                    kind: "kv.storage.cleared",
                },
            ],
            queries: Vec::new(),
            resources: resource_methods(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "kv.set" => {
                let app = arg(args, 0, "app")?;
                let key = arg(args, 1, "key")?;
                reject_public_reserved(&key)?;
                let value = args.get(2..).unwrap_or_default().join(" ");
                ensure_app_exists(ctx.bus, &app)?;
                if key.trim().is_empty() {
                    return Err(Error::InvalidInput("key must not be empty".into()));
                }
                Ok(Decision::Commit(vec![set_event(app, key, value)?]))
            }
            "kv.rm" | "kv.delete" => {
                let app = arg(args, 0, "app")?;
                let key = arg(args, 1, "key")?;
                reject_public_reserved(&key)?;
                let missing = state_ref::<KvState>(ctx.state, "kv")?
                    .data
                    .get(&app)
                    .map(|kv| !kv.contains_key(&key))
                    .unwrap_or(true);
                if missing {
                    return Err(Error::KeyNotFound(app, key));
                }
                Ok(Decision::Commit(vec![delete_event(app, key)?]))
            }
            "kv.storage.set" => decide_storage_set(ctx, args),
            "kv.storage.clear" => decide_storage_clear(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "kv.set" => {
                let e: Set = decode_event(record)?;
                state_mut::<KvState>(state, "kv")?
                    .data
                    .entry(e.app)
                    .or_default()
                    .insert(e.key, e.value);
            }
            "kv.deleted" => {
                let e: Deleted = decode_event(record)?;
                let state = state_mut::<KvState>(state, "kv")?;
                if let Some(kv) = state.data.get_mut(&e.app) {
                    kv.remove(&e.key);
                    if kv.is_empty() {
                        state.data.remove(&e.app);
                    }
                }
            }
            // React to another capability's event: drop a removed app's data.
            "app.removed" => {
                #[derive(BorshDeserialize)]
                struct Removed {
                    id: String,
                }
                let e: Removed = decode_event(record)?;
                let state = state_mut::<KvState>(state, "kv")?;
                state.data.remove(&e.id);
                state.storage.apps.remove(&e.id);
            }
            "kv.storage.configured" => {
                let e: StorageConfigured = decode_event(record)?;
                let binding = KvStorageBinding::new(e.backend, e.path)?;
                let state = state_mut::<KvState>(state, "kv")?;
                match e.app {
                    Some(app) => {
                        state.storage.apps.insert(app, binding);
                    }
                    None => {
                        state.storage.default = binding;
                    }
                }
            }
            "kv.storage.cleared" => {
                let e: StorageCleared = decode_event(record)?;
                let state = state_mut::<KvState>(state, "kv")?;
                match e.app {
                    Some(app) => {
                        state.storage.apps.remove(&app);
                    }
                    None => {
                        state.storage.default = KvStorageBinding::default();
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "kv.set" => {
                let e: Set = decode_event(record).ok()?;
                Some(format!("kv.set {}/{} = {}", e.app, e.key, e.value))
            }
            "kv.deleted" => {
                let e: Deleted = decode_event(record).ok()?;
                Some(format!("kv.deleted {}/{}", e.app, e.key))
            }
            "kv.storage.configured" => {
                let e: StorageConfigured = decode_event(record).ok()?;
                let binding = KvStorageBinding::new(e.backend, e.path).ok()?;
                Some(match e.app {
                    Some(app) => format!("kv.storage {app} -> {}", binding.describe()),
                    None => format!("kv.storage default -> {}", binding.describe()),
                })
            }
            "kv.storage.cleared" => {
                let e: StorageCleared = decode_event(record).ok()?;
                Some(match e.app {
                    Some(app) => format!("kv.storage {app} cleared"),
                    None => "kv.storage default cleared".to_string(),
                })
            }
            _ => None,
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "get" => read_get(ctx.state, ctx.app, args),
            "all" => read_all(ctx.state, ctx.app, args),
            "scan" => read_scan(ctx.state, ctx.app, args),
            "range" => read_range(ctx.state, ctx.app, args),
            "keys" => read_keys(ctx.state, ctx.app, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: kv.{other}"
            ))),
        }
    }
}

fn decide_storage_set(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, backend, path) = parse_storage_binding_args(ctx, args)?;
    Ok(Decision::Commit(vec![encode_event(
        "kv.storage.configured",
        &StorageConfigured { app, backend, path },
    )?]))
}

fn decide_storage_clear(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = parse_storage_scope(ctx, args)?;
    Ok(Decision::Commit(vec![encode_event(
        "kv.storage.cleared",
        &StorageCleared { app },
    )?]))
}

fn parse_storage_binding_args(
    ctx: CommandCtx<'_>,
    args: &[String],
) -> Result<(Option<String>, KvStorageBackend, Option<String>)> {
    let scope = arg(args, 0, "scope")?;
    match scope.as_str() {
        "default" => {
            ensure_arg_count(args, 3)?;
            let backend = parse_storage_backend(args, 1)?;
            let path = parse_storage_path(args, 2)?;
            Ok((None, backend, path))
        }
        "app" => {
            ensure_arg_count(args, 4)?;
            let app = arg(args, 1, "app")?;
            ensure_app_exists(ctx.bus, &app)?;
            let backend = parse_storage_backend(args, 2)?;
            let path = parse_storage_path(args, 3)?;
            Ok((Some(app), backend, path))
        }
        other => Err(Error::InvalidInput(format!(
            "storage scope must be default or app, got {other}"
        ))),
    }
}

fn parse_storage_scope(ctx: CommandCtx<'_>, args: &[String]) -> Result<Option<String>> {
    let scope = arg(args, 0, "scope")?;
    match scope.as_str() {
        "default" => {
            ensure_arg_count(args, 1)?;
            Ok(None)
        }
        "app" => {
            ensure_arg_count(args, 2)?;
            let app = arg(args, 1, "app")?;
            ensure_app_exists(ctx.bus, &app)?;
            Ok(Some(app))
        }
        other => Err(Error::InvalidInput(format!(
            "storage scope must be default or app, got {other}"
        ))),
    }
}

fn ensure_arg_count(args: &[String], max: usize) -> Result<()> {
    if args.len() > max {
        return Err(Error::InvalidInput(format!(
            "too many kv storage arguments: expected at most {max}, got {}",
            args.len()
        )));
    }
    Ok(())
}

fn parse_storage_backend(args: &[String], index: usize) -> Result<KvStorageBackend> {
    arg(args, index, "backend")?.parse()
}

fn parse_storage_path(args: &[String], index: usize) -> Result<Option<String>> {
    let Some(path) = args.get(index) else {
        return Ok(None);
    };
    let path = path.trim();
    if path.is_empty() {
        return Err(Error::InvalidInput(
            "kv storage path must not be empty".into(),
        ));
    }
    Ok(Some(path.to_string()))
}

fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "set",
            params: &["key", "value"],
        },
        ResourceMethod::Read {
            name: "get",
            params: &["key"],
        },
        ResourceMethod::Read {
            name: "all",
            params: &[],
        },
        ResourceMethod::Write {
            name: "rm",
            params: &["key"],
        },
        ResourceMethod::Read {
            name: "scan",
            params: &["prefix", "limit"],
        },
        ResourceMethod::Read {
            name: "range",
            params: &["start", "endExclusive", "limit"],
        },
        ResourceMethod::Read {
            name: "keys",
            params: &["prefix", "limit"],
        },
    ]
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

/// `ctx.resource.kv.get(key)` - the value for `key` in `app`'s store, or none.
fn read_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let key = args.first().map(String::as_str).unwrap_or_default();
    if is_reserved_key(key) {
        return Ok(ReadValue::OptString(None));
    }
    Ok(ReadValue::OptString(get_value(state, app, key)?))
}

/// `ctx.resource.kv.all()` - every non-reserved key/value pair in `app`'s store.
fn read_all(state: &dyn StateStore, app: &str, _args: &[String]) -> Result<ReadValue> {
    Ok(ReadValue::StringMap(public_pairs(
        state_ref::<KvState>(state, "kv")?
            .data
            .get(app)
            .cloned()
            .unwrap_or_default(),
    )))
}

fn read_scan(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    reject_public_reserved(prefix)?;
    let limit = parse_limit(args.get(1))?;
    Ok(ReadValue::StringMap(
        scan_prefix(state, app, prefix, limit)?
            .into_iter()
            .filter(|(key, _)| !is_reserved_key(key))
            .collect(),
    ))
}

fn read_range(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let start = args.first().map(String::as_str).unwrap_or_default();
    let end = args.get(1).map(String::as_str).unwrap_or_default();
    reject_public_reserved(start)?;
    reject_public_reserved(end)?;
    if end <= start {
        return Err(Error::InvalidInput(
            "range endExclusive must sort after start".into(),
        ));
    }
    let limit = parse_limit(args.get(2))?;
    Ok(ReadValue::StringMap(
        scan_range(state, app, start, end, limit)?
            .into_iter()
            .filter(|(key, _)| !is_reserved_key(key))
            .collect(),
    ))
}

fn read_keys(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    reject_public_reserved(prefix)?;
    let limit = parse_limit(args.get(1))?;
    Ok(ReadValue::StringList(
        scan_prefix(state, app, prefix, limit)?
            .into_iter()
            .map(|(key, _)| key)
            .filter(|key| !is_reserved_key(key))
            .collect(),
    ))
}

fn public_pairs(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    map.into_iter()
        .filter(|(key, _)| !is_reserved_key(key))
        .collect()
}

fn reject_public_reserved(key: &str) -> Result<()> {
    if is_reserved_key(key) {
        Err(Error::InvalidInput(format!(
            "kv key prefix {RESERVED_PREFIX:?} is reserved for platform data"
        )))
    } else {
        Ok(())
    }
}

fn parse_limit(raw: Option<&String>) -> Result<usize> {
    match raw.map(String::as_str).filter(|s| !s.is_empty()) {
        Some(s) => s.parse::<usize>().map(bounded_limit).map_err(|_| {
            Error::InvalidInput(format!("limit must be a positive integer, got {s:?}"))
        }),
        None => Ok(DEFAULT_SCAN_LIMIT),
    }
}

fn bounded_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_SCAN_LIMIT)
}

#[cfg(test)]
mod tests;
