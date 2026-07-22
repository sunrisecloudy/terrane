//! Discovery, verification, activation, and caching for native capabilities.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use terrane_cap_interface::EventRecord;
use terrane_cap_protocol::{
    read_frame, validate_dependency_graph, write_frame, BundleManifest, CapabilityLockfile,
    LockedCapability, ProtocolError, WorkerRequest, WorkerResponse, LOCK_FORMAT_VERSION,
    PROTOCOL_VERSION,
};

pub const DEFAULT_MAX_WARM_WORKERS: usize = 8;
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MANIFEST_FILE: &str = "manifest.json";
const LOCK_FILE: &str = "capabilities.lock.json";

pub fn verifying_key_from_hex(value: &str) -> Result<VerifyingKey, ManagerError> {
    let bytes = decode_hex(value)?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| ManagerError::Invalid("verifying key must encode exactly 32 bytes".into()))?;
    VerifyingKey::from_bytes(&bytes)
        .map_err(|error| ManagerError::Invalid(format!("invalid verifying key: {error}")))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityStatus {
    Available,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityStatusView {
    pub namespace: String,
    pub version: String,
    pub status: CapabilityStatus,
    pub keep_alive: bool,
}

#[derive(Debug)]
pub enum ManagerError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Protocol(ProtocolError),
    Invalid(String),
    Worker(String),
}

impl std::fmt::Display for ManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "capability manager I/O error: {error}"),
            Self::Json(error) => write!(f, "capability manager JSON error: {error}"),
            Self::Protocol(error) => write!(f, "capability protocol error: {error}"),
            Self::Invalid(message) => write!(f, "invalid capability bundle: {message}"),
            Self::Worker(message) => write!(f, "capability worker error: {message}"),
        }
    }
}

impl std::error::Error for ManagerError {}

impl From<std::io::Error> for ManagerError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<serde_json::Error> for ManagerError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

impl From<ProtocolError> for ManagerError {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

pub struct CapabilityManager {
    home: PathBuf,
    packaged_root: PathBuf,
    verifying_key: VerifyingKey,
    catalog: BTreeMap<String, BundleManifest>,
    slots: Mutex<BTreeMap<String, Arc<Mutex<WorkerSlot>>>>,
    max_warm_workers: usize,
    idle_timeout: Duration,
}

#[derive(Default)]
struct WorkerSlot {
    process: Option<WorkerProcess>,
    loading: bool,
    keep_alive: bool,
    last_error: Option<String>,
}

struct WorkerProcess {
    child: Child,
    input: BufWriter<ChildStdin>,
    output: BufReader<ChildStdout>,
    last_used: Instant,
}

impl WorkerProcess {
    fn call(&mut self, request: &WorkerRequest) -> Result<WorkerResponse, ManagerError> {
        write_frame(&mut self.input, request)?;
        let response = read_frame(&mut self.output)?;
        self.last_used = Instant::now();
        match response {
            WorkerResponse::Error {
                code,
                message,
                retryable,
            } => Err(ManagerError::Worker(format!(
                "{code}: {message} (retryable={retryable})"
            ))),
            response => Ok(response),
        }
    }

    fn stop(mut self) {
        let _ = self.call(&WorkerRequest::Shutdown);
        let _ = self.child.wait();
    }
}

impl CapabilityManager {
    pub fn open(
        home: impl Into<PathBuf>,
        packaged_root: impl Into<PathBuf>,
        verifying_key: VerifyingKey,
    ) -> Result<Arc<Self>, ManagerError> {
        Self::open_with_limits(
            home,
            packaged_root,
            verifying_key,
            DEFAULT_MAX_WARM_WORKERS,
            DEFAULT_IDLE_TIMEOUT,
        )
    }

    pub fn open_with_limits(
        home: impl Into<PathBuf>,
        packaged_root: impl Into<PathBuf>,
        verifying_key: VerifyingKey,
        max_warm_workers: usize,
        idle_timeout: Duration,
    ) -> Result<Arc<Self>, ManagerError> {
        let home = home.into();
        let packaged_root = packaged_root.into();
        let catalog = read_catalog(&packaged_root, &verifying_key)?;
        validate_dependency_graph(&catalog)?;
        fs::create_dir_all(home.join("capabilities/cache"))?;
        fs::create_dir_all(home.join("capabilities/state"))?;
        ensure_lockfile(&home, &packaged_root, &catalog)?;
        Ok(Arc::new(Self {
            home,
            packaged_root,
            verifying_key,
            catalog,
            slots: Mutex::new(BTreeMap::new()),
            max_warm_workers,
            idle_timeout,
        }))
    }

    pub fn namespaces(&self) -> impl Iterator<Item = &str> {
        self.catalog.keys().map(String::as_str)
    }

    pub fn contains(&self, namespace: &str) -> bool {
        self.catalog.contains_key(namespace)
    }

    pub fn manifest(&self, namespace: &str) -> Option<&BundleManifest> {
        self.catalog.get(namespace)
    }

    pub fn status(&self) -> Vec<CapabilityStatusView> {
        let slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.catalog
            .iter()
            .map(|(namespace, manifest)| {
                let slot = slots.get(namespace).and_then(|slot| slot.lock().ok());
                let status = match slot.as_deref() {
                    Some(WorkerSlot {
                        process: Some(_), ..
                    }) => CapabilityStatus::Ready,
                    Some(WorkerSlot { loading: true, .. }) => CapabilityStatus::Loading,
                    Some(WorkerSlot {
                        last_error: Some(error),
                        ..
                    }) => CapabilityStatus::Failed(error.clone()),
                    _ => CapabilityStatus::Available,
                };
                CapabilityStatusView {
                    namespace: namespace.clone(),
                    version: manifest.version.clone(),
                    status,
                    keep_alive: slot.as_deref().is_some_and(|slot| slot.keep_alive),
                }
            })
            .collect()
    }

    pub fn prepare(
        self: &Arc<Self>,
        namespaces: &[String],
        records: &[EventRecord],
    ) -> Result<(), ManagerError> {
        let closure = self.dependency_closure(namespaces)?;
        let failures = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for namespace in closure {
                let failures = &failures;
                scope.spawn(move || {
                    if let Err(error) = self.ensure_loaded(&namespace, records) {
                        failures
                            .lock()
                            .unwrap_or_else(|poisoned| poisoned.into_inner())
                            .push((namespace, error));
                    }
                });
            }
        });
        let mut failures = failures
            .into_inner()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((namespace, error)) = failures.pop() {
            return Err(ManagerError::Worker(format!(
                "failed to prepare {namespace}: {error}"
            )));
        }
        self.evict_idle()?;
        Ok(())
    }

    pub fn call(
        &self,
        namespace: &str,
        records: &[EventRecord],
        request: WorkerRequest,
    ) -> Result<WorkerResponse, ManagerError> {
        self.ensure_loaded(namespace, records)?;
        let slot = self.slot(namespace)?;
        let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let process = slot
            .process
            .as_mut()
            .ok_or_else(|| ManagerError::Worker(format!("{namespace} is not loaded")))?;
        process.call(&request)
    }

    pub fn fold_loaded(&self, first_seq: u64, records: &[EventRecord]) {
        if records.is_empty() {
            return;
        }
        let slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let loaded: Vec<_> = slots
            .iter()
            .map(|(namespace, slot)| (namespace.clone(), slot.clone()))
            .collect();
        drop(slots);
        for (namespace, slot) in loaded {
            let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(process) = slot.process.as_mut() else {
                continue;
            };
            let mut failed = None;
            for (offset, record) in records.iter().enumerate() {
                let seq = first_seq + offset as u64;
                match process.call(&WorkerRequest::Fold {
                    seq,
                    record: record.clone(),
                    dependencies: BTreeMap::new(),
                }) {
                    Ok(WorkerResponse::Ack { last_applied_seq }) if last_applied_seq == seq => {}
                    Ok(response) => {
                        failed = Some(format!("fold sequence {seq} returned {response:?}"));
                        break;
                    }
                    Err(error) => {
                        failed = Some(error.to_string());
                        break;
                    }
                }
            }
            if let Some(error) = failed {
                if let Some(mut process) = slot.process.take() {
                    let _ = process.child.kill();
                    let _ = process.child.wait();
                }
                slot.last_error = Some(format!("{namespace} fell behind the log: {error}"));
            }
        }
    }

    pub fn validate_worker_records(
        &self,
        namespace: &str,
        records: &[EventRecord],
    ) -> Result<(), ManagerError> {
        let manifest = self
            .catalog
            .get(namespace)
            .ok_or_else(|| ManagerError::Invalid(format!("unknown capability {namespace}")))?;
        let declared: BTreeSet<_> = manifest
            .declaration
            .events
            .iter()
            .map(String::as_str)
            .collect();
        for record in records {
            let owner = record
                .kind
                .split_once('.')
                .map(|(owner, _)| owner)
                .ok_or_else(|| {
                    ManagerError::Invalid(format!(
                        "worker returned unnamespaced event {}",
                        record.kind
                    ))
                })?;
            if owner == namespace && !declared.contains(record.kind.as_str()) {
                return Err(ManagerError::Invalid(format!(
                    "worker {namespace} returned undeclared event {}",
                    record.kind
                )));
            }
            if owner != namespace {
                if !manifest
                    .declaration
                    .delegated_events
                    .iter()
                    .any(|event| event == &record.kind)
                {
                    return Err(ManagerError::Invalid(format!(
                        "worker {namespace} returned non-delegated foreign event {}",
                        record.kind
                    )));
                }
                if let Some(owner_manifest) = self.catalog.get(owner) {
                    if !owner_manifest
                        .declaration
                        .events
                        .iter()
                        .any(|event| event == &record.kind)
                    {
                        return Err(ManagerError::Invalid(format!(
                            "worker {namespace} returned undeclared foreign event {}",
                            record.kind
                        )));
                    }
                } else if !terrane_cap_protocol::is_fundamental(owner) {
                    return Err(ManagerError::Invalid(format!(
                        "worker {namespace} returned event for unknown namespace {owner}"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn set_keep_alive(&self, namespace: &str, keep_alive: bool) -> Result<(), ManagerError> {
        let slot = self.slot(namespace)?;
        slot.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .keep_alive = keep_alive;
        Ok(())
    }

    pub fn evict(&self, namespace: &str) -> Result<bool, ManagerError> {
        let slot = self.slot(namespace)?;
        let process = {
            let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if slot.keep_alive {
                return Ok(false);
            }
            slot.process.take()
        };
        if let Some(mut process) = process {
            self.persist_snapshot(namespace, &mut process)?;
            process.stop();
            return Ok(true);
        }
        Ok(false)
    }

    pub fn evict_all(&self) -> Result<(), ManagerError> {
        let namespaces: Vec<_> = self.catalog.keys().cloned().collect();
        for namespace in namespaces {
            let slot = self.slot(&namespace)?;
            slot.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .keep_alive = false;
            let _ = self.evict(&namespace)?;
        }
        Ok(())
    }

    pub fn repair(&self, namespace: &str) -> Result<(), ManagerError> {
        let manifest = self
            .catalog
            .get(namespace)
            .ok_or_else(|| ManagerError::Invalid(format!("unknown capability {namespace}")))?;
        let slot = self.slot(namespace)?;
        let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut process) = slot.process.take() {
            self.persist_snapshot(namespace, &mut process)?;
            process.stop();
        }
        let cache = self.cache_dir(manifest);
        if cache.exists() {
            fs::remove_dir_all(&cache)?;
        }
        self.ensure_cached(manifest)?;
        slot.last_error = None;
        Ok(())
    }

    pub fn evict_idle(&self) -> Result<(), ManagerError> {
        let slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut candidates = Vec::new();
        let mut loaded = 0usize;
        for (namespace, slot) in slots.iter() {
            let slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(process) = &slot.process {
                loaded += 1;
                if !slot.keep_alive {
                    candidates.push((namespace.clone(), process.last_used));
                }
            }
        }
        drop(slots);
        candidates.sort_by_key(|(_, last_used)| *last_used);
        let now = Instant::now();
        let excess = loaded.saturating_sub(self.max_warm_workers);
        let mut evicted = BTreeSet::new();
        for (namespace, last_used) in &candidates {
            if now.duration_since(*last_used) >= self.idle_timeout && self.evict(namespace)? {
                evicted.insert(namespace.clone());
            }
        }
        let remaining_excess = excess.saturating_sub(evicted.len());
        for (namespace, _) in candidates
            .into_iter()
            .filter(|(namespace, _)| !evicted.contains(namespace))
            .take(remaining_excess)
        {
            let _ = self.evict(&namespace)?;
        }
        Ok(())
    }

    fn ensure_loaded(&self, namespace: &str, records: &[EventRecord]) -> Result<(), ManagerError> {
        let manifest = self
            .catalog
            .get(namespace)
            .ok_or_else(|| ManagerError::Invalid(format!("unknown capability {namespace}")))?;
        let slot = self.slot(namespace)?;
        let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.process.is_some() {
            return Ok(());
        }
        slot.loading = true;
        let result = self.spawn_worker(manifest, records);
        slot.loading = false;
        match result {
            Ok(process) => {
                slot.process = Some(process);
                slot.keep_alive =
                    manifest.activation == terrane_cap_protocol::ActivationMode::Background;
                slot.last_error = None;
                Ok(())
            }
            Err(error) => {
                slot.last_error = Some(error.to_string());
                Err(error)
            }
        }
    }

    fn spawn_worker(
        &self,
        manifest: &BundleManifest,
        records: &[EventRecord],
    ) -> Result<WorkerProcess, ManagerError> {
        let executable = self.ensure_cached(manifest)?;
        let manifest_sha256 = sha256_bytes(&manifest.signing_payload()?);
        let mut child = Command::new(executable)
            .arg("--namespace")
            .arg(&manifest.namespace)
            .arg("--version")
            .arg(&manifest.version)
            .arg("--manifest-sha256")
            .arg(&manifest_sha256)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| ManagerError::Worker("worker stdin was not piped".into()))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| ManagerError::Worker("worker stdout was not piped".into()))?;
        let mut process = WorkerProcess {
            child,
            input: BufWriter::new(input),
            output: BufReader::new(output),
            last_used: Instant::now(),
        };
        let nonce = random_nonce()?;
        match process.call(&WorkerRequest::Hello {
            protocol_version: PROTOCOL_VERSION,
            namespace: manifest.namespace.clone(),
            session_nonce: nonce.clone(),
        })? {
            WorkerResponse::Hello {
                protocol_version,
                namespace,
                version,
                session_nonce,
                manifest_sha256: worker_manifest_sha256,
            } if protocol_version == PROTOCOL_VERSION
                && namespace == manifest.namespace
                && version == manifest.version
                && session_nonce == nonce
                && worker_manifest_sha256 == manifest_sha256 => {}
            response => {
                return Err(ManagerError::Worker(format!(
                    "invalid worker handshake: {response:?}"
                )))
            }
        }
        match process.call(&WorkerRequest::Replay {
            records: records.to_vec(),
        })? {
            WorkerResponse::Ack { .. } => Ok(process),
            response => Err(ManagerError::Worker(format!(
                "worker replay returned {response:?}"
            ))),
        }
    }

    fn ensure_cached(&self, manifest: &BundleManifest) -> Result<PathBuf, ManagerError> {
        let cache = self.cache_dir(manifest);
        let executable = cache.join(&manifest.executable);
        if cached_bundle_is_valid(&cache, manifest, &self.verifying_key) {
            return Ok(executable);
        }
        let packaged = self.packaged_root.join(&manifest.namespace);
        let source = manifest.verify_bundle(&packaged, &self.verifying_key)?;
        let parent = cache
            .parent()
            .ok_or_else(|| ManagerError::Invalid("capability cache has no parent".into()))?;
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!(
            ".{}.tmp-{}",
            manifest.executable_sha256,
            random_nonce()?
        ));
        fs::create_dir_all(&temporary)?;
        fs::copy(source, temporary.join(&manifest.executable))?;
        fs::copy(packaged.join(MANIFEST_FILE), temporary.join(MANIFEST_FILE))?;
        if cache.exists() {
            fs::remove_dir_all(&cache)?;
        }
        fs::rename(&temporary, &cache)?;
        Ok(executable)
    }

    fn cache_dir(&self, manifest: &BundleManifest) -> PathBuf {
        self.home
            .join("capabilities/cache")
            .join(&manifest.namespace)
            .join(&manifest.version)
            .join(&manifest.executable_sha256)
    }

    fn persist_snapshot(
        &self,
        namespace: &str,
        process: &mut WorkerProcess,
    ) -> Result<(), ManagerError> {
        let response = process.call(&WorkerRequest::Snapshot)?;
        let WorkerResponse::Snapshot {
            payload,
            last_applied_seq,
            state_sha256,
        } = response
        else {
            return Err(ManagerError::Worker(format!(
                "snapshot returned {response:?}"
            )));
        };
        let state_dir = self.home.join("capabilities/state");
        fs::create_dir_all(&state_dir)?;
        let target = state_dir.join(format!("{namespace}.json"));
        let temporary = state_dir.join(format!(".{namespace}.tmp"));
        let document = serde_json::json!({
            "namespace": namespace,
            "lastAppliedSeq": last_applied_seq,
            "stateSha256": state_sha256,
            "payload": payload,
        });
        fs::write(&temporary, serde_json::to_vec_pretty(&document)?)?;
        fs::rename(temporary, target)?;
        Ok(())
    }

    fn slot(&self, namespace: &str) -> Result<Arc<Mutex<WorkerSlot>>, ManagerError> {
        if !self.catalog.contains_key(namespace) {
            return Err(ManagerError::Invalid(format!(
                "unknown capability {namespace}"
            )));
        }
        let mut slots = self
            .slots
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        Ok(slots
            .entry(namespace.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(WorkerSlot::default())))
            .clone())
    }

    fn dependency_closure(&self, roots: &[String]) -> Result<Vec<String>, ManagerError> {
        let mut pending = roots.to_vec();
        let mut found = BTreeSet::new();
        while let Some(namespace) = pending.pop() {
            let manifest = self.catalog.get(&namespace).ok_or_else(|| {
                ManagerError::Invalid(format!("unknown required capability {namespace}"))
            })?;
            if found.insert(namespace) {
                pending.extend(manifest.dependencies.iter().cloned());
            }
        }
        Ok(found.into_iter().collect())
    }
}

impl Drop for CapabilityManager {
    fn drop(&mut self) {
        let slots = self
            .slots
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for slot in slots.values() {
            let process = slot
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .process
                .take();
            if let Some(process) = process {
                process.stop();
            }
        }
    }
}

fn read_catalog(
    root: &Path,
    verifying_key: &VerifyingKey,
) -> Result<BTreeMap<String, BundleManifest>, ManagerError> {
    let mut catalog = BTreeMap::new();
    if !root.is_dir() {
        return Err(ManagerError::Invalid(format!(
            "capability bundle root does not exist: {}",
            root.display()
        )));
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join(MANIFEST_FILE);
        if !path.is_file() {
            continue;
        }
        let manifest: BundleManifest = serde_json::from_reader(File::open(&path)?)?;
        manifest.validate()?;
        if manifest.platform != std::env::consts::OS
            || manifest.architecture != std::env::consts::ARCH
        {
            return Err(ManagerError::Invalid(format!(
                "capability {} targets {}/{}, host is {}/{}",
                manifest.namespace,
                manifest.platform,
                manifest.architecture,
                std::env::consts::OS,
                std::env::consts::ARCH
            )));
        }
        verify_manifest_signature(&manifest, verifying_key)?;
        if catalog
            .insert(manifest.namespace.clone(), manifest)
            .is_some()
        {
            return Err(ManagerError::Invalid(format!(
                "duplicate capability bundle namespace in {}",
                root.display()
            )));
        }
    }
    Ok(catalog)
}

fn verify_manifest_signature(
    manifest: &BundleManifest,
    verifying_key: &VerifyingKey,
) -> Result<(), ManagerError> {
    let bytes = decode_hex(&manifest.signature)?;
    let signature = Signature::from_slice(&bytes)
        .map_err(|error| ManagerError::Invalid(format!("invalid signature: {error}")))?;
    verifying_key
        .verify(&manifest.signing_payload()?, &signature)
        .map_err(|error| ManagerError::Invalid(format!("signature verification failed: {error}")))
}

fn ensure_lockfile(
    home: &Path,
    packaged_root: &Path,
    catalog: &BTreeMap<String, BundleManifest>,
) -> Result<(), ManagerError> {
    let target = home.join(LOCK_FILE);
    if target.is_file() {
        let lock: CapabilityLockfile = serde_json::from_reader(File::open(&target)?)?;
        if lock.format_version != LOCK_FORMAT_VERSION {
            return Err(ManagerError::Invalid(format!(
                "unsupported capability lock version {}",
                lock.format_version
            )));
        }
        let locked_namespaces: BTreeSet<_> = lock.capabilities.keys().collect();
        let packaged_namespaces: BTreeSet<_> = catalog.keys().collect();
        if locked_namespaces != packaged_namespaces {
            return Err(ManagerError::Invalid(
                "capability lock must pin every packaged namespace exactly".into(),
            ));
        }
        for (namespace, locked) in &lock.capabilities {
            let manifest = catalog.get(namespace).ok_or_else(|| {
                ManagerError::Invalid(format!("locked capability {namespace} is not packaged"))
            })?;
            if locked.namespace != *namespace
                || locked.version != manifest.version
                || locked.executable_sha256 != manifest.executable_sha256
                || locked.platform != manifest.platform
                || locked.architecture != manifest.architecture
                || locked.bundle_sha256 != sha256_bundle(&packaged_root.join(namespace), manifest)?
            {
                return Err(ManagerError::Invalid(format!(
                    "packaged capability {namespace} does not match its exact lock"
                )));
            }
        }
        return Ok(());
    }
    let mut lock = CapabilityLockfile::default();
    for (namespace, manifest) in catalog {
        let bundle_sha256 = sha256_bundle(&packaged_root.join(namespace), manifest)?;
        lock.capabilities.insert(
            namespace.clone(),
            LockedCapability {
                namespace: namespace.clone(),
                version: manifest.version.clone(),
                bundle_sha256,
                executable_sha256: manifest.executable_sha256.clone(),
                platform: manifest.platform.clone(),
                architecture: manifest.architecture.clone(),
            },
        );
    }
    fs::create_dir_all(home)?;
    let temporary = home.join(format!(".{LOCK_FILE}.tmp"));
    fs::write(&temporary, serde_json::to_vec_pretty(&lock)?)?;
    fs::rename(temporary, target)?;
    Ok(())
}

fn cached_bundle_is_valid(
    cache: &Path,
    expected: &BundleManifest,
    verifying_key: &VerifyingKey,
) -> bool {
    let manifest = File::open(cache.join(MANIFEST_FILE))
        .ok()
        .and_then(|file| serde_json::from_reader::<_, BundleManifest>(file).ok());
    manifest.as_ref() == Some(expected) && expected.verify_bundle(cache, verifying_key).is_ok()
}

fn sha256_bundle(path: &Path, manifest: &BundleManifest) -> Result<String, ManagerError> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path.join(MANIFEST_FILE))?);
    hasher.update(manifest.executable.as_bytes());
    hasher.update(manifest.executable_sha256.as_bytes());
    Ok(hex(&hasher.finalize()))
}

fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn random_nonce() -> Result<String, ManagerError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| ManagerError::Invalid(format!("generate worker nonce: {error}")))?;
    Ok(hex(&bytes))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ManagerError> {
    if !value.len().is_multiple_of(2) {
        return Err(ManagerError::Invalid("hex value has odd length".into()));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = hex_nibble(pair[0])?;
            let low = hex_nibble(pair[1])?;
            Ok((high << 4) | low)
        })
        .collect()
}

fn hex_nibble(value: u8) -> Result<u8, ManagerError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(ManagerError::Invalid("invalid hex digit".into())),
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use terrane_cap_protocol::{
        sha256_file, ActivationMode, CapabilityDeclaration, BUNDLE_FORMAT_VERSION,
    };

    fn write_bundle(root: &Path, signing: &SigningKey, namespace: &str, dependencies: Vec<String>) {
        let dir = root.join(namespace);
        fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("worker");
        fs::write(&executable, b"worker").unwrap();
        let mut manifest = BundleManifest {
            format_version: BUNDLE_FORMAT_VERSION,
            namespace: namespace.into(),
            version: "0.1.0".into(),
            protocol_version: PROTOCOL_VERSION,
            state_schema_version: 1,
            platform: std::env::consts::OS.into(),
            architecture: std::env::consts::ARCH.into(),
            executable: "worker".into(),
            executable_sha256: sha256_file(&executable).unwrap(),
            signature: String::new(),
            dependencies,
            activation: ActivationMode::Demand,
            declaration: CapabilityDeclaration {
                commands: vec![format!("{namespace}.run")],
                ..CapabilityDeclaration::default()
            },
        };
        manifest.signature = hex(&signing
            .sign(&manifest.signing_payload().unwrap())
            .to_bytes());
        fs::write(
            dir.join(MANIFEST_FILE),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn open_builds_exact_lock_without_loading_workers() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        write_bundle(packaged.path(), &signing, "alpha", Vec::new());
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        assert_eq!(manager.status()[0].status, CapabilityStatus::Available);
        let lock: CapabilityLockfile =
            serde_json::from_reader(File::open(home.path().join(LOCK_FILE)).unwrap()).unwrap();
        assert_eq!(lock.capabilities["alpha"].version, "0.1.0");
    }

    #[test]
    fn dependency_closure_is_transitive_and_deduplicated() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[10u8; 32]);
        write_bundle(packaged.path(), &signing, "alpha", vec!["beta".into()]);
        write_bundle(packaged.path(), &signing, "beta", vec!["gamma".into()]);
        write_bundle(packaged.path(), &signing, "gamma", Vec::new());
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        assert_eq!(
            manager
                .dependency_closure(&["alpha".into(), "beta".into()])
                .unwrap(),
            vec!["alpha", "beta", "gamma"]
        );
    }

    #[test]
    fn corrupt_cached_executable_is_repaired_from_the_exact_packaged_bundle() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[11u8; 32]);
        write_bundle(packaged.path(), &signing, "alpha", Vec::new());
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        let manifest = manager.catalog["alpha"].clone();
        let cached = manager.ensure_cached(&manifest).unwrap();
        fs::write(&cached, b"corrupt").unwrap();

        manager.repair("alpha").unwrap();
        let repaired = manager.cache_dir(&manifest).join(&manifest.executable);
        assert_eq!(sha256_file(&repaired).unwrap(), manifest.executable_sha256);
    }

    #[test]
    fn an_existing_lock_must_pin_the_exact_bundle_digest() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[12u8; 32]);
        write_bundle(packaged.path(), &signing, "alpha", Vec::new());
        drop(
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap(),
        );
        let lock_path = home.path().join(LOCK_FILE);
        let mut lock: CapabilityLockfile =
            serde_json::from_reader(File::open(&lock_path).unwrap()).unwrap();
        lock.capabilities.get_mut("alpha").unwrap().bundle_sha256 = "0".repeat(64);
        fs::write(&lock_path, serde_json::to_vec_pretty(&lock).unwrap()).unwrap();

        let error = CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key())
            .err()
            .unwrap();
        assert!(error.to_string().contains("exact lock"));
    }

    #[test]
    fn corrupt_signature_is_rejected_before_any_worker_starts() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[11u8; 32]);
        write_bundle(packaged.path(), &signing, "alpha", Vec::new());
        let other = SigningKey::from_bytes(&[12u8; 32]);
        let error =
            match CapabilityManager::open(home.path(), packaged.path(), other.verifying_key()) {
                Ok(_) => panic!("corrupt signature was accepted"),
                Err(error) => error,
            };
        assert!(error.to_string().contains("signature verification failed"));
    }
}
