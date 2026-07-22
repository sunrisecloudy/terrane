//! Discovery, verification, activation, and caching for native capabilities.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use terrane_cap_interface::EventRecord;
use terrane_cap_protocol::{
    validate_dependency_graph, write_frame, BundleManifest, CapabilityIndex,
    CapabilityIndexArtifact, CapabilityLockfile, HostConnectorRequest, HostConnectorResponse,
    LockedCapability, ProtocolError, WorkerRequest, WorkerResponse, LOCK_FORMAT_VERSION,
    PROTOCOL_VERSION,
};

pub const DEFAULT_MAX_WARM_WORKERS: usize = 8;
pub const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MANIFEST_FILE: &str = "manifest.json";
const LOCK_FILE: &str = "capabilities.lock.json";
const INDEX_FILE: &str = "index.json";
const MIGRATION_MARKER_FILE: &str = "capabilities/migration-v1.json";

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
    Timeout(Duration),
}

impl std::fmt::Display for ManagerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "capability manager I/O error: {error}"),
            Self::Json(error) => write!(f, "capability manager JSON error: {error}"),
            Self::Protocol(error) => write!(f, "capability protocol error: {error}"),
            Self::Invalid(message) => write!(f, "invalid capability bundle: {message}"),
            Self::Worker(message) => write!(f, "capability worker error: {message}"),
            Self::Timeout(timeout) => write!(
                f,
                "capability worker did not respond within {} seconds",
                timeout.as_secs()
            ),
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
    artifacts: BTreeMap<String, CapabilityIndexArtifact>,
    download_base_url: Option<String>,
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
    output: ChildStdout,
    last_used: Instant,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDocument {
    namespace: String,
    last_applied_seq: u64,
    state_sha256: String,
    #[serde(default)]
    payload_sha256: String,
    payload: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MigrationMarker {
    format_version: u32,
    lock_sha256: String,
    migrated_log_records: u64,
    migrated_log_sha256: String,
}

impl WorkerProcess {
    fn call(&mut self, request: &WorkerRequest) -> Result<WorkerResponse, ManagerError> {
        self.call_with_connector(request, &mut |_request| HostConnectorResponse::Error {
            message: "parent host connector is unavailable for this worker request".into(),
        })
    }

    fn call_with_connector(
        &mut self,
        request: &WorkerRequest,
        connector: &mut dyn FnMut(HostConnectorRequest) -> HostConnectorResponse,
    ) -> Result<WorkerResponse, ManagerError> {
        write_frame(&mut self.input, request)?;
        loop {
            let response = read_worker_response(&mut self.output, request_timeout(request))?;
            self.last_used = Instant::now();
            match response {
                WorkerResponse::ConnectorRequest {
                    request_id,
                    request,
                } => {
                    let response = connector(request);
                    write_frame(
                        &mut self.input,
                        &WorkerRequest::ConnectorResponse {
                            request_id,
                            response,
                        },
                    )?;
                }
                WorkerResponse::Error {
                    code,
                    message,
                    retryable,
                } => {
                    return Err(ManagerError::Worker(format!(
                        "{code}: {message} (retryable={retryable})"
                    )))
                }
                response => return Ok(response),
            }
        }
    }

    fn stop(mut self) {
        let _ = self.call(&WorkerRequest::Shutdown);
        let _ = self.child.wait();
    }
}

fn request_timeout(request: &WorkerRequest) -> Duration {
    match request {
        WorkerRequest::RunRuntime { .. } | WorkerRequest::ExecuteEffect { .. } => {
            Duration::from_secs(5 * 60)
        }
        _ => Duration::from_secs(30),
    }
}

#[cfg(unix)]
fn configure_worker_output(output: &ChildStdout) -> Result<(), ManagerError> {
    use std::os::fd::AsRawFd;

    let descriptor = output.as_raw_fd();
    // SAFETY: the descriptor is owned by `output` for the duration of both
    // calls, and `F_GETFL`/`F_SETFL` do not retain the pointer or descriptor.
    let flags = unsafe { libc::fcntl(descriptor, libc::F_GETFL) };
    if flags < 0 {
        return Err(ManagerError::Io(std::io::Error::last_os_error()));
    }
    if unsafe { libc::fcntl(descriptor, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(ManagerError::Io(std::io::Error::last_os_error()));
    }
    Ok(())
}

#[cfg(not(unix))]
fn configure_worker_output(_output: &ChildStdout) -> Result<(), ManagerError> {
    Ok(())
}

#[cfg(unix)]
fn read_worker_response(
    output: &mut ChildStdout,
    timeout: Duration,
) -> Result<WorkerResponse, ManagerError> {
    let deadline = Instant::now() + timeout;
    let mut length = [0u8; 4];
    read_exact_until(output, &mut length, deadline, timeout)?;
    let size = u32::from_le_bytes(length) as usize;
    if size > terrane_cap_protocol::DEFAULT_MAX_FRAME_BYTES {
        return Err(ManagerError::Protocol(ProtocolError::FrameTooLarge {
            size,
            max: terrane_cap_protocol::DEFAULT_MAX_FRAME_BYTES,
        }));
    }
    let mut payload = vec![0u8; size];
    read_exact_until(output, &mut payload, deadline, timeout)?;
    serde_json::from_slice(&payload).map_err(ManagerError::Json)
}

#[cfg(unix)]
fn read_exact_until(
    output: &mut ChildStdout,
    mut buffer: &mut [u8],
    deadline: Instant,
    timeout: Duration,
) -> Result<(), ManagerError> {
    use std::os::fd::AsRawFd;

    while !buffer.is_empty() {
        match output.read(buffer) {
            Ok(0) => return Err(ManagerError::Protocol(ProtocolError::Eof)),
            Ok(read) => buffer = &mut buffer[read..],
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                let remaining = deadline
                    .checked_duration_since(Instant::now())
                    .ok_or(ManagerError::Timeout(timeout))?;
                let milliseconds = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
                let mut descriptor = libc::pollfd {
                    fd: output.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                // SAFETY: `descriptor` points to one initialized pollfd for the
                // duration of the call and the timeout is bounded to `i32`.
                let result = unsafe { libc::poll(&mut descriptor, 1, milliseconds) };
                if result == 0 {
                    return Err(ManagerError::Timeout(timeout));
                }
                if result < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(ManagerError::Io(error));
                }
            }
            Err(error) => return Err(ManagerError::Io(error)),
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn read_worker_response(
    output: &mut ChildStdout,
    _timeout: Duration,
) -> Result<WorkerResponse, ManagerError> {
    terrane_cap_protocol::read_frame(output).map_err(ManagerError::Protocol)
}

fn full_replay(process: &mut WorkerProcess, records: &[EventRecord]) -> Result<u64, ManagerError> {
    match process.call(&WorkerRequest::Replay {
        records: records.to_vec(),
    })? {
        WorkerResponse::Ack { last_applied_seq } if last_applied_seq == records.len() as u64 => {
            Ok(last_applied_seq)
        }
        response => Err(ManagerError::Worker(format!(
            "worker replay returned {response:?}"
        ))),
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
        let packaged = read_catalog(&packaged_root, &verifying_key)?;
        validate_dependency_graph(&packaged.manifests)?;
        fs::create_dir_all(home.join("capabilities/cache"))?;
        fs::create_dir_all(home.join("capabilities/state"))?;
        ensure_lockfile(
            &home,
            &packaged_root,
            &packaged.manifests,
            &packaged.artifacts,
        )?;
        Ok(Arc::new(Self {
            home,
            packaged_root,
            verifying_key,
            catalog: packaged.manifests,
            artifacts: packaged.artifacts,
            download_base_url: packaged.download_base_url,
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

    pub fn migration_complete(&self) -> Result<bool, ManagerError> {
        let marker_path = self.home.join(MIGRATION_MARKER_FILE);
        let marker: MigrationMarker = match fs::read(&marker_path) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(marker) => marker,
                Err(_) => return Ok(false),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if marker.format_version != 1 {
            return Ok(false);
        }
        let lock = fs::read(self.home.join(LOCK_FILE))?;
        Ok(marker.lock_sha256 == sha256_bytes(&lock))
    }

    pub fn mark_migration_complete(&self, records: &[EventRecord]) -> Result<(), ManagerError> {
        let path = self.home.join(MIGRATION_MARKER_FILE);
        let parent = path
            .parent()
            .ok_or_else(|| ManagerError::Invalid("migration marker has no parent".into()))?;
        fs::create_dir_all(parent)?;
        let lock = fs::read(self.home.join(LOCK_FILE))?;
        let marker = MigrationMarker {
            format_version: 1,
            lock_sha256: sha256_bytes(&lock),
            migrated_log_records: records.len() as u64,
            migrated_log_sha256: sha256_bytes(&serde_json::to_vec(records)?),
        };
        let temporary = parent.join(format!(".migration-{}.tmp", random_nonce()?));
        fs::write(&temporary, serde_json::to_vec_pretty(&marker)?)?;
        fs::rename(temporary, path)?;
        Ok(())
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

    pub fn prepare_background(
        self: &Arc<Self>,
        records: &[EventRecord],
    ) -> Result<(), ManagerError> {
        let namespaces = self
            .catalog
            .iter()
            .filter(|(_, manifest)| {
                manifest.activation == terrane_cap_protocol::ActivationMode::Background
                    && records.iter().any(|record| {
                        manifest
                            .declaration
                            .events
                            .iter()
                            .any(|kind| kind == &record.kind)
                    })
            })
            .map(|(namespace, _)| namespace.clone())
            .collect::<Vec<_>>();
        self.prepare(&namespaces, records)
    }

    pub fn call(
        &self,
        namespace: &str,
        records: &[EventRecord],
        request: WorkerRequest,
    ) -> Result<WorkerResponse, ManagerError> {
        self.ensure_loaded(namespace, records)?;
        let slot = self.slot(namespace)?;
        let first = {
            let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let process = slot
                .process
                .as_mut()
                .ok_or_else(|| ManagerError::Worker(format!("{namespace} is not loaded")))?;
            process.call(&request)
        };
        match first {
            Ok(response) => Ok(response),
            Err(
                error
                @ (ManagerError::Io(_) | ManagerError::Protocol(_) | ManagerError::Timeout(_)),
            ) => {
                {
                    let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    if let Some(mut process) = slot.process.take() {
                        let _ = process.child.kill();
                        let _ = process.child.wait();
                    }
                    slot.last_error = Some(format!("worker exited; restarting once: {error}"));
                }
                self.ensure_loaded(namespace, records)?;
                let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                let process = slot
                    .process
                    .as_mut()
                    .ok_or_else(|| ManagerError::Worker(format!("{namespace} did not restart")))?;
                match process.call(&request) {
                    Ok(response) => Ok(response),
                    Err(error) => {
                        if let Some(mut process) = slot.process.take() {
                            let _ = process.child.kill();
                            let _ = process.child.wait();
                        }
                        slot.last_error = Some(format!("worker restart failed: {error}"));
                        Err(error)
                    }
                }
            }
            Err(error) => Err(error),
        }
    }

    pub fn call_with_connector(
        &self,
        namespace: &str,
        records: &[EventRecord],
        request: WorkerRequest,
        mut connector: impl FnMut(HostConnectorRequest) -> HostConnectorResponse,
    ) -> Result<WorkerResponse, ManagerError> {
        self.ensure_loaded(namespace, records)?;
        let slot = self.slot(namespace)?;
        let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let process = slot
            .process
            .as_mut()
            .ok_or_else(|| ManagerError::Worker(format!("{namespace} is not loaded")))?;
        process.call_with_connector(&request, &mut connector)
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

    pub fn full_replay_hashes(
        &self,
        records: &[EventRecord],
    ) -> Result<BTreeMap<String, String>, ManagerError> {
        let namespaces: Vec<_> = self.catalog.keys().cloned().collect();
        let mut hashes = BTreeMap::new();
        for namespace in namespaces {
            let response = self.call(&namespace, records, WorkerRequest::Snapshot)?;
            let WorkerResponse::Snapshot {
                last_applied_seq,
                state_sha256,
                ..
            } = response
            else {
                return Err(ManagerError::Worker(format!(
                    "capability {namespace} returned {response:?} during replay verification"
                )));
            };
            if last_applied_seq != records.len() as u64 {
                return Err(ManagerError::Worker(format!(
                    "capability {namespace} replay cursor {last_applied_seq} does not match log length {}",
                    records.len()
                )));
            }
            hashes.insert(namespace.clone(), state_sha256);
            self.set_keep_alive(&namespace, false)?;
            let _ = self.evict(&namespace)?;
        }
        Ok(hashes)
    }

    /// Force a loaded worker to exit so trusted diagnostics can verify the
    /// one-restart recovery path. The next request performs restore + replay.
    pub fn terminate_for_diagnostics(&self, namespace: &str) -> Result<bool, ManagerError> {
        let slot = self.slot(namespace)?;
        let mut slot = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(process) = slot.process.as_mut() else {
            return Ok(false);
        };
        process.child.kill()?;
        let _ = process.child.wait();
        Ok(true)
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
            Ok(mut process) => {
                slot.keep_alive =
                    if manifest.activation == terrane_cap_protocol::ActivationMode::Background {
                        matches!(
                            process.call(&WorkerRequest::BackgroundStatus),
                            Ok(WorkerResponse::BackgroundStatus {
                                keep_alive: true,
                                ..
                            })
                        )
                    } else {
                        false
                    };
                slot.process = Some(process);
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
        let declared_events = self
            .catalog
            .values()
            .flat_map(|manifest| manifest.declaration.events.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(",");
        let mut child = Command::new(executable)
            .arg("--namespace")
            .arg(&manifest.namespace)
            .arg("--version")
            .arg(&manifest.version)
            .arg("--manifest-sha256")
            .arg(&manifest_sha256)
            .env("TERRANE_CAP_DECLARED_EVENTS", declared_events)
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
        configure_worker_output(&output)?;
        let mut process = WorkerProcess {
            child,
            input: BufWriter::new(input),
            output,
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
        let restored_seq = self.restore_worker_snapshot(manifest, records, &mut process)?;
        for (offset, record) in records[restored_seq as usize..].iter().enumerate() {
            let seq = restored_seq + offset as u64 + 1;
            match process.call(&WorkerRequest::Fold {
                seq,
                record: record.clone(),
                dependencies: BTreeMap::new(),
            })? {
                WorkerResponse::Ack { last_applied_seq } if last_applied_seq == seq => {}
                response => {
                    return Err(ManagerError::Worker(format!(
                        "worker tail replay at sequence {seq} returned {response:?}"
                    )))
                }
            }
        }
        Ok(process)
    }

    fn restore_worker_snapshot(
        &self,
        manifest: &BundleManifest,
        records: &[EventRecord],
        process: &mut WorkerProcess,
    ) -> Result<u64, ManagerError> {
        let path = self
            .home
            .join("capabilities/state")
            .join(format!("{}.json", manifest.namespace));
        let document = File::open(path)
            .ok()
            .and_then(|file| serde_json::from_reader::<_, SnapshotDocument>(file).ok())
            .filter(|document| {
                document.namespace == manifest.namespace
                    && document.last_applied_seq <= records.len() as u64
                    && !document.payload_sha256.is_empty()
                    && sha256_bytes(&document.payload) == document.payload_sha256
            });
        let Some(document) = document else {
            return full_replay(process, records);
        };
        match process.call(&WorkerRequest::Restore {
            snapshot: document.payload,
            last_applied_seq: document.last_applied_seq,
            dependencies: BTreeMap::new(),
        }) {
            Ok(WorkerResponse::Ack { last_applied_seq })
                if last_applied_seq == document.last_applied_seq =>
            {
                Ok(last_applied_seq)
            }
            Ok(_) | Err(_) => full_replay(process, records),
        }
    }

    fn ensure_cached(&self, manifest: &BundleManifest) -> Result<PathBuf, ManagerError> {
        let cache = self.cache_dir(manifest);
        let executable = cache.join(&manifest.executable);
        if cached_bundle_is_valid(&cache, manifest, &self.verifying_key) {
            return Ok(executable);
        }
        if let Some(artifact) = self.artifacts.get(&manifest.namespace) {
            self.ensure_cached_archive(manifest, artifact, &cache)?;
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

    fn ensure_cached_archive(
        &self,
        manifest: &BundleManifest,
        artifact: &CapabilityIndexArtifact,
        cache: &Path,
    ) -> Result<(), ManagerError> {
        let packaged = self.packaged_root.join(&artifact.archive);
        let archive = if archive_matches(&packaged, &artifact.archive_sha256) {
            packaged
        } else {
            self.download_exact_artifact(artifact)?
        };
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
        if let Err(error) = extract_tcap(&archive, &temporary, manifest) {
            let _ = fs::remove_dir_all(&temporary);
            return Err(error);
        }
        manifest.verify_bundle(&temporary, &self.verifying_key)?;
        let extracted: BundleManifest =
            serde_json::from_reader(File::open(temporary.join(MANIFEST_FILE))?)?;
        if extracted != *manifest {
            let _ = fs::remove_dir_all(&temporary);
            return Err(ManagerError::Invalid(format!(
                "archive {} manifest does not match signed index",
                artifact.archive
            )));
        }
        if cache.exists() {
            fs::remove_dir_all(cache)?;
        }
        fs::rename(temporary, cache)?;
        Ok(())
    }

    fn download_exact_artifact(
        &self,
        artifact: &CapabilityIndexArtifact,
    ) -> Result<PathBuf, ManagerError> {
        let base = self
            .download_base_url
            .as_deref()
            .ok_or_else(|| {
                ManagerError::Invalid(format!(
                    "packaged artifact {} is unavailable and signed index has no download URL",
                    artifact.archive
                ))
            })?
            .trim_end_matches('/');
        let url = format!("{base}/{}", artifact.archive);
        let downloads = self.home.join("capabilities/downloads");
        fs::create_dir_all(&downloads)?;
        let target = downloads.join(format!(
            "{}-{}",
            artifact.manifest.namespace, artifact.archive_sha256
        ));
        if archive_matches(&target, &artifact.archive_sha256) {
            return Ok(target);
        }
        let temporary = downloads.join(format!(".download-{}.tmp", random_nonce()?));
        let response = ureq::get(&url).call().map_err(|error| {
            ManagerError::Io(std::io::Error::other(format!(
                "download exact capability artifact {url}: {error}"
            )))
        })?;
        let mut reader = response.into_reader();
        let mut file = File::create(&temporary)?;
        std::io::copy(&mut reader, &mut file)?;
        drop(file);
        if !archive_matches(&temporary, &artifact.archive_sha256) {
            let _ = fs::remove_file(&temporary);
            return Err(ManagerError::Invalid(format!(
                "downloaded artifact {} does not match pinned hash {}",
                artifact.archive, artifact.archive_sha256
            )));
        }
        if target.exists() {
            fs::remove_file(&target)?;
        }
        fs::rename(temporary, &target)?;
        Ok(target)
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
        let document = SnapshotDocument {
            namespace: namespace.to_string(),
            last_applied_seq,
            payload_sha256: sha256_bytes(&payload),
            state_sha256,
            payload,
        };
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

struct PackagedCatalog {
    manifests: BTreeMap<String, BundleManifest>,
    artifacts: BTreeMap<String, CapabilityIndexArtifact>,
    download_base_url: Option<String>,
}

fn read_catalog(
    root: &Path,
    verifying_key: &VerifyingKey,
) -> Result<PackagedCatalog, ManagerError> {
    let index_path = root.join(INDEX_FILE);
    if index_path.is_file() {
        let index: CapabilityIndex = serde_json::from_reader(File::open(index_path)?)?;
        index.validate(verifying_key)?;
        let manifests = index
            .artifacts
            .iter()
            .map(|(namespace, artifact)| (namespace.clone(), artifact.manifest.clone()))
            .collect::<BTreeMap<_, _>>();
        validate_platforms(&manifests)?;
        return Ok(PackagedCatalog {
            manifests,
            artifacts: index.artifacts,
            download_base_url: index.download_base_url,
        });
    }
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
    Ok(PackagedCatalog {
        manifests: catalog,
        artifacts: BTreeMap::new(),
        download_base_url: None,
    })
}

fn validate_platforms(catalog: &BTreeMap<String, BundleManifest>) -> Result<(), ManagerError> {
    for manifest in catalog.values() {
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
    }
    Ok(())
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
    artifacts: &BTreeMap<String, CapabilityIndexArtifact>,
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
                || locked.bundle_sha256
                    != expected_bundle_sha256(packaged_root, namespace, manifest, artifacts)?
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
        let bundle_sha256 = expected_bundle_sha256(packaged_root, namespace, manifest, artifacts)?;
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

fn expected_bundle_sha256(
    packaged_root: &Path,
    namespace: &str,
    manifest: &BundleManifest,
    artifacts: &BTreeMap<String, CapabilityIndexArtifact>,
) -> Result<String, ManagerError> {
    if let Some(artifact) = artifacts.get(namespace) {
        return Ok(artifact.archive_sha256.clone());
    }
    sha256_bundle(&packaged_root.join(namespace), manifest)
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

fn archive_matches(path: &Path, expected: &str) -> bool {
    path.is_file() && terrane_cap_protocol::sha256_file(path).is_ok_and(|actual| actual == expected)
}

fn extract_tcap(
    archive_path: &Path,
    target: &Path,
    manifest: &BundleManifest,
) -> Result<(), ManagerError> {
    let file = File::open(archive_path)?;
    let decoder = zstd::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    let allowed = BTreeSet::from([MANIFEST_FILE.to_string(), manifest.executable.clone()]);
    let mut seen = BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            return Err(ManagerError::Invalid(
                "capability archive contains a non-file entry".into(),
            ));
        }
        let path = entry.path()?.to_string_lossy().into_owned();
        if !allowed.contains(&path) || !seen.insert(path.clone()) {
            return Err(ManagerError::Invalid(format!(
                "capability archive contains unexpected entry {path}"
            )));
        }
        entry.unpack(target.join(path))?;
    }
    if seen != allowed {
        return Err(ManagerError::Invalid(
            "capability archive is missing its manifest or executable".into(),
        ));
    }
    Ok(())
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
    use std::io::{Read, Write};

    use ed25519_dalek::{Signer, SigningKey};
    use terrane_cap_protocol::{
        sha256_file, ActivationMode, CapabilityDeclaration, CapabilityIndex,
        CapabilityIndexArtifact, BUNDLE_FORMAT_VERSION, INDEX_FORMAT_VERSION,
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

    fn write_indexed_archive(
        root: &Path,
        signing: &SigningKey,
        namespace: &str,
        download_base_url: Option<String>,
    ) -> PathBuf {
        write_bundle(root, signing, namespace, Vec::new());
        let bundle = root.join(namespace);
        let manifest: BundleManifest =
            serde_json::from_reader(File::open(bundle.join(MANIFEST_FILE)).unwrap()).unwrap();
        let archive_name = format!("{namespace}-0.1.0.tcap");
        let archive_path = root.join(&archive_name);
        let file = File::create(&archive_path).unwrap();
        let encoder = zstd::Encoder::new(file, 1).unwrap();
        let mut archive = tar::Builder::new(encoder);
        archive
            .append_path_with_name(bundle.join(MANIFEST_FILE), MANIFEST_FILE)
            .unwrap();
        archive
            .append_path_with_name(bundle.join("worker"), "worker")
            .unwrap();
        archive.into_inner().unwrap().finish().unwrap();
        fs::remove_dir_all(bundle).unwrap();
        let artifact = CapabilityIndexArtifact {
            archive: archive_name,
            archive_sha256: sha256_file(&archive_path).unwrap(),
            manifest,
        };
        let mut index = CapabilityIndex {
            format_version: INDEX_FORMAT_VERSION,
            download_base_url,
            artifacts: BTreeMap::from([(namespace.to_string(), artifact)]),
            signature: String::new(),
        };
        index.signature = hex(&signing.sign(&index.signing_payload().unwrap()).to_bytes());
        fs::write(
            root.join(INDEX_FILE),
            serde_json::to_vec_pretty(&index).unwrap(),
        )
        .unwrap();
        archive_path
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
    fn migration_marker_is_atomic_and_bound_to_the_exact_lock() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[31u8; 32]);
        write_bundle(packaged.path(), &signing, "alpha", Vec::new());
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        assert!(!manager.migration_complete().unwrap());

        manager.mark_migration_complete(&[]).unwrap();
        assert!(manager.migration_complete().unwrap());
        assert!(home.path().join(MIGRATION_MARKER_FILE).is_file());
        assert!(fs::read_dir(home.path().join("capabilities"))
            .unwrap()
            .all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".migration-")));

        fs::write(home.path().join(LOCK_FILE), b"{}").unwrap();
        assert!(!manager.migration_complete().unwrap());
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

    #[test]
    fn signed_tcap_is_extracted_only_when_demanded() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[21u8; 32]);
        write_indexed_archive(packaged.path(), &signing, "alpha", None);
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        assert!(!home.path().join("capabilities/cache/alpha").exists());
        let manifest = manager.catalog["alpha"].clone();
        let executable = manager.ensure_cached(&manifest).unwrap();
        assert_eq!(fs::read(executable).unwrap(), b"worker");
    }

    #[test]
    fn offline_repair_fails_closed_without_replacing_the_exact_lock() {
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[22u8; 32]);
        let archive = write_indexed_archive(packaged.path(), &signing, "alpha", None);
        fs::remove_file(archive).unwrap();
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        let error = manager
            .ensure_cached(&manager.catalog["alpha"])
            .unwrap_err();
        assert!(error.to_string().contains("no download URL"));
        let lock: CapabilityLockfile =
            serde_json::from_reader(File::open(home.path().join(LOCK_FILE)).unwrap()).unwrap();
        assert_eq!(lock.capabilities["alpha"].version, "0.1.0");
    }

    #[test]
    fn missing_packaged_archive_repairs_from_signed_index_exact_hash() {
        use std::net::TcpListener;
        use std::thread;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let packaged = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let signing = SigningKey::from_bytes(&[23u8; 32]);
        let archive = write_indexed_archive(packaged.path(), &signing, "alpha", Some(base_url));
        let bytes = fs::read(&archive).unwrap();
        fs::remove_file(archive).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let manager =
            CapabilityManager::open(home.path(), packaged.path(), signing.verifying_key()).unwrap();
        let executable = manager.ensure_cached(&manager.catalog["alpha"]).unwrap();
        assert_eq!(fs::read(executable).unwrap(), b"worker");
        server.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn silent_worker_pipe_times_out() {
        let mut child = Command::new("sh")
            .arg("-c")
            .arg("sleep 1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut output = child.stdout.take().unwrap();
        configure_worker_output(&output).unwrap();
        let error = read_worker_response(&mut output, Duration::from_millis(10)).unwrap_err();
        assert!(matches!(error, ManagerError::Timeout(_)));
        child.kill().unwrap();
        child.wait().unwrap();
    }
}
