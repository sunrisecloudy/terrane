//! Native process adapter for Terrane's existing Rust capabilities.
//!
//! This compatibility worker deliberately reuses each capability's current
//! Rust implementation. The process boundary is the stable protocol crate;
//! no Rust trait object crosses into the host.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Error, EventRecord, GrantResourceSpec, QueryCtx, QueryValue,
    ResourceReadCtx, Result as CapResult, StateStore,
};
use terrane_cap_protocol::{
    read_frame, write_frame, WorkerRequest, WorkerResponse, PROTOCOL_VERSION,
};

#[cfg(feature = "packager")]
pub mod package;
mod selected;

type SelectedFactory = fn() -> selected::SelectedCapability;

#[derive(Default)]
struct WorkerState(BTreeMap<String, Box<dyn Any>>);

impl WorkerState {
    fn insert<T: Any>(&mut self, namespace: &str, value: T) {
        self.0.insert(namespace.to_string(), Box::new(value));
    }

    fn insert_boxed(&mut self, namespace: &str, value: Box<dyn Any>) {
        self.0.insert(namespace.to_string(), value);
    }
}

impl StateStore for WorkerState {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        self.0.get(namespace).map(Box::as_ref)
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        self.0.get_mut(namespace).map(Box::as_mut)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DurableSnapshot {
    records: Vec<EventRecord>,
}

pub struct Worker {
    namespace: String,
    version: String,
    manifest_sha256: String,
    capability: Box<dyn Capability>,
    support: Vec<Box<dyn Capability>>,
    state: WorkerState,
    factory: SelectedFactory,
    background_work: fn(&dyn StateStore) -> bool,
    declared_events: BTreeSet<String>,
    applied_records: Vec<EventRecord>,
    last_applied_seq: u64,
}

impl Worker {
    pub fn new(
        namespace: impl Into<String>,
        version: impl Into<String>,
        manifest_sha256: impl Into<String>,
    ) -> CapResult<Self> {
        let namespace = namespace.into();
        let factory: SelectedFactory = selected::build;
        let selected = factory();
        if selected.capability.namespace() != namespace {
            return Err(Error::InvalidInput(format!(
                "worker binary owns {}, host requested {namespace}",
                selected.capability.namespace()
            )));
        }
        let (state, support) = worker_state(selected.state);
        let declared_events = std::env::var("TERRANE_CAP_DECLARED_EVENTS")
            .unwrap_or_default()
            .split(',')
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .collect();
        Ok(Self {
            namespace,
            version: version.into(),
            manifest_sha256: manifest_sha256.into(),
            capability: selected.capability,
            support,
            state,
            factory,
            background_work: selected.background_work,
            declared_events,
            applied_records: Vec::new(),
            last_applied_seq: 0,
        })
    }

    pub fn handle(&mut self, request: WorkerRequest) -> WorkerResponse {
        match self.try_handle(request) {
            Ok(response) => response,
            Err(error) => WorkerResponse::Error {
                code: error_code(&error).into(),
                message: error.to_string(),
                retryable: matches!(error, Error::Runtime(_) | Error::Storage(_)),
            },
        }
    }

    fn try_handle(&mut self, request: WorkerRequest) -> CapResult<WorkerResponse> {
        match request {
            WorkerRequest::Hello {
                protocol_version,
                namespace,
                session_nonce,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    return Err(Error::InvalidInput(format!(
                        "worker protocol version {protocol_version} is unsupported"
                    )));
                }
                if namespace != self.namespace {
                    return Err(Error::InvalidInput(format!(
                        "worker owns {}, host requested {namespace}",
                        self.namespace
                    )));
                }
                Ok(WorkerResponse::Hello {
                    protocol_version: PROTOCOL_VERSION,
                    namespace: self.namespace.clone(),
                    version: self.version.clone(),
                    session_nonce,
                    manifest_sha256: self.manifest_sha256.clone(),
                })
            }
            WorkerRequest::Replay { records } => {
                self.reset();
                self.last_applied_seq = 0;
                for (index, record) in records.iter().enumerate() {
                    self.fold(record)?;
                    self.applied_records.push(record.clone());
                    self.last_applied_seq = index as u64 + 1;
                }
                Ok(WorkerResponse::Ack {
                    last_applied_seq: self.last_applied_seq,
                })
            }
            WorkerRequest::Restore {
                snapshot,
                last_applied_seq,
                dependencies: _,
            } => {
                self.reset();
                if !snapshot.is_empty() {
                    self.restore_snapshot(&snapshot)?;
                }
                self.last_applied_seq = last_applied_seq;
                Ok(WorkerResponse::Ack { last_applied_seq })
            }
            WorkerRequest::Fold {
                seq,
                record,
                dependencies: _,
            } => {
                if seq != self.last_applied_seq + 1 {
                    return Err(Error::Storage(format!(
                        "worker {} expected log sequence {}, got {seq}",
                        self.namespace,
                        self.last_applied_seq + 1
                    )));
                }
                self.fold(&record)?;
                self.applied_records.push(record);
                self.last_applied_seq = seq;
                Ok(WorkerResponse::Ack {
                    last_applied_seq: seq,
                })
            }
            WorkerRequest::Decide {
                request,
                dependencies: _,
            } => {
                let bus = WorkerBus::new(
                    self.capability.as_ref(),
                    &self.support,
                    &self.state,
                    &self.declared_events,
                );
                let decision = self.capability.decide(
                    CommandCtx {
                        state: &self.state,
                        bus: &bus,
                    },
                    &request.name,
                    &request.args,
                )?;
                Ok(WorkerResponse::Decision { decision })
            }
            WorkerRequest::Query {
                name,
                args,
                dependencies: _,
            } => {
                let bus = WorkerBus::new(
                    self.capability.as_ref(),
                    &self.support,
                    &self.state,
                    &self.declared_events,
                );
                let value = self.capability.query(
                    QueryCtx {
                        state: &self.state,
                        bus: &bus,
                    },
                    &name,
                    &args,
                )?;
                Ok(WorkerResponse::QueryValue { value })
            }
            WorkerRequest::ReadResource {
                app,
                name,
                args,
                dependencies: _,
            } => {
                let bus = WorkerBus::new(
                    self.capability.as_ref(),
                    &self.support,
                    &self.state,
                    &self.declared_events,
                );
                let value = self.capability.read_resource(
                    ResourceReadCtx {
                        state: &self.state,
                        bus: &bus,
                        app: &app,
                        host: None,
                    },
                    &name,
                    &args,
                )?;
                Ok(WorkerResponse::ReadValue { value })
            }
            WorkerRequest::ResourceCallOutput {
                app,
                method,
                records,
            } => {
                let value =
                    self.capability
                        .resource_call_output(&self.state, &app, &method, &records)?;
                Ok(WorkerResponse::ReadValue { value })
            }
            WorkerRequest::Snapshot => {
                let payload = self.snapshot()?;
                Ok(WorkerResponse::Snapshot {
                    state_sha256: self.canonical_state_hash()?,
                    payload,
                    last_applied_seq: self.last_applied_seq,
                })
            }
            WorkerRequest::Health => Ok(WorkerResponse::Health {
                ready: true,
                detail: format!(
                    "{} ready at log sequence {}",
                    self.namespace, self.last_applied_seq
                ),
            }),
            WorkerRequest::BackgroundStatus => {
                let keep_alive = (self.background_work)(&self.state);
                Ok(WorkerResponse::BackgroundStatus {
                    keep_alive,
                    reason: if keep_alive {
                        "private state contains persisted background activation hints".into()
                    } else {
                        "private state has no persisted background activation hints".into()
                    },
                })
            }
            WorkerRequest::RunRuntime { .. } => Err(Error::Runtime(
                "runtime host callbacks are not attached to this worker session".into(),
            )),
            WorkerRequest::ExecuteEffect { .. } => Err(Error::Runtime(
                "effect execution remains owned by the parent host connector".into(),
            )),
            WorkerRequest::Shutdown => Ok(WorkerResponse::Ack {
                last_applied_seq: self.last_applied_seq,
            }),
        }
    }

    fn reset(&mut self) {
        let selected = (self.factory)();
        let (state, support) = worker_state(selected.state);
        self.capability = selected.capability;
        self.background_work = selected.background_work;
        self.support = support;
        self.state = state;
        self.applied_records.clear();
    }

    fn fold(&mut self, record: &EventRecord) -> CapResult<()> {
        for capability in &self.support {
            capability.fold(&mut self.state, record)?;
        }
        self.capability.fold(&mut self.state, record)
    }

    fn snapshot(&self) -> CapResult<Vec<u8>> {
        serde_json::to_vec(&DurableSnapshot {
            records: self
                .applied_records
                .iter()
                .filter(|record| self.record_affects_worker(record))
                .cloned()
                .collect(),
        })
        .map_err(|error| Error::Storage(format!("encode worker snapshot: {error}")))
    }

    fn restore_snapshot(&mut self, payload: &[u8]) -> CapResult<()> {
        let snapshot: DurableSnapshot = serde_json::from_slice(payload)
            .map_err(|error| Error::Storage(format!("decode worker snapshot: {error}")))?;
        for record in &snapshot.records {
            self.fold(record)?;
        }
        self.applied_records = snapshot.records;
        Ok(())
    }

    fn canonical_state_hash(&self) -> CapResult<String> {
        let relevant = self
            .applied_records
            .iter()
            .filter(|record| capability_accepts(self.capability.as_ref(), record))
            .collect::<Vec<_>>();
        let encoded = serde_json::to_vec(&relevant)
            .map_err(|error| Error::Storage(format!("encode canonical state input: {error}")))?;
        Ok(sha256(&encoded))
    }

    fn record_affects_worker(&self, record: &EventRecord) -> bool {
        capability_accepts(self.capability.as_ref(), record)
            || self
                .support
                .iter()
                .any(|capability| capability_accepts(capability.as_ref(), record))
    }
}

fn capability_accepts(capability: &dyn Capability, record: &EventRecord) -> bool {
    let manifest = capability.manifest();
    manifest
        .events
        .iter()
        .any(|event| event.kind == record.kind)
        || manifest
            .subscriptions
            .iter()
            .any(|subscription| subscription.kind == record.kind)
}

fn worker_state(
    selected_state: Option<(&'static str, Box<dyn Any>)>,
) -> (WorkerState, Vec<Box<dyn Capability>>) {
    let mut state = WorkerState::default();
    state.insert("app", terrane_cap_app::AppState::default());
    state.insert("blob", terrane_cap_blob::BlobState::default());
    state.insert("person", terrane_cap_person::PersonState::default());
    state.insert("replica", terrane_cap_replica::ReplicaState::default());
    if let Some((namespace, value)) = selected_state {
        state.insert_boxed(namespace, value);
    }
    let support: Vec<Box<dyn Capability>> = vec![
        Box::new(terrane_cap_app::AppCapability),
        Box::new(terrane_cap_blob::BlobCapability),
        Box::new(terrane_cap_person::PersonCapability),
        Box::new(terrane_cap_replica::ReplicaCapability),
    ];
    (state, support)
}

struct WorkerBus<'a> {
    owner: &'a dyn Capability,
    support: &'a [Box<dyn Capability>],
    state: &'a dyn StateStore,
    declared_events: &'a BTreeSet<String>,
}

impl<'a> WorkerBus<'a> {
    fn new(
        owner: &'a dyn Capability,
        support: &'a [Box<dyn Capability>],
        state: &'a dyn StateStore,
        declared_events: &'a BTreeSet<String>,
    ) -> Self {
        Self {
            owner,
            support,
            state,
            declared_events,
        }
    }

    fn capability(&self, namespace: &str) -> Option<&dyn Capability> {
        if self.owner.namespace() == namespace {
            return Some(self.owner);
        }
        self.support
            .iter()
            .map(Box::as_ref)
            .find(|capability| capability.namespace() == namespace)
    }
}

impl CapBus for WorkerBus<'_> {
    fn query(&self, capability: &str, name: &str, args: &[String]) -> CapResult<QueryValue> {
        let capability = self.capability(capability).ok_or_else(|| {
            Error::InvalidInput(format!(
                "worker has no query adapter for {capability}.{name}"
            ))
        })?;
        capability.query(
            QueryCtx {
                state: self.state,
                bus: self,
            },
            name,
            args,
        )
    }

    fn event_kind_matches(&self, pattern: &str) -> bool {
        self.declared_events.iter().any(|kind| {
            pattern.strip_suffix(".*").is_some_and(|prefix| {
                kind.strip_prefix(prefix)
                    .is_some_and(|rest| rest.starts_with('.'))
            }) || kind == pattern
        })
    }

    fn grant_resource_spec(
        &self,
        namespace: &str,
        selector_schema_id: &str,
    ) -> CapResult<Option<GrantResourceSpec>> {
        let Some(capability) = self.capability(namespace) else {
            return Ok(None);
        };
        Ok(capability.grant_resource_specs().into_iter().find(|spec| {
            spec.namespace == namespace && spec.selector_schema_id == selector_schema_id
        }))
    }
}

pub fn serve(
    worker: &mut Worker,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<(), terrane_cap_protocol::ProtocolError> {
    loop {
        let request: WorkerRequest = match read_frame(reader) {
            Ok(request) => request,
            Err(terrane_cap_protocol::ProtocolError::Eof) => return Ok(()),
            Err(error) => return Err(error),
        };
        let shutdown = matches!(request, WorkerRequest::Shutdown);
        write_frame(writer, &worker.handle(request))?;
        if shutdown {
            return Ok(());
        }
    }
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn error_code(error: &Error) -> &'static str {
    match error {
        Error::AppExists(_) => "app_exists",
        Error::AppNotFound(_) => "app_not_found",
        Error::KeyNotFound(_, _) => "key_not_found",
        Error::InvalidInput(_) => "invalid_input",
        Error::Storage(_) => "storage",
        Error::Runtime(_) => "runtime",
    }
}

pub struct EmptyBus;

impl CapBus for EmptyBus {
    fn query(
        &self,
        capability: &str,
        name: &str,
        _args: &[String],
    ) -> CapResult<terrane_cap_interface::QueryValue> {
        Err(Error::InvalidInput(format!(
            "worker bus has no query adapter for {capability}.{name}"
        )))
    }
}

#[derive(Default)]
pub struct EmptyState(BTreeMap<String, Box<dyn std::any::Any>>);

impl StateStore for EmptyState {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        self.0.get(namespace).map(|value| value.as_ref())
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        self.0.get_mut(namespace).map(|value| value.as_mut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terrane_cap_protocol::PROTOCOL_VERSION;

    #[test]
    fn handshake_echoes_session_and_identity() {
        let mut worker = Worker::new("time", "0.1.0", "abc").unwrap();
        assert_eq!(
            worker.handle(WorkerRequest::Hello {
                protocol_version: PROTOCOL_VERSION,
                namespace: "time".into(),
                session_nonce: "nonce".into(),
            }),
            WorkerResponse::Hello {
                protocol_version: PROTOCOL_VERSION,
                namespace: "time".into(),
                version: "0.1.0".into(),
                session_nonce: "nonce".into(),
                manifest_sha256: "abc".into(),
            }
        );
    }

    #[test]
    fn replay_then_snapshot_tracks_sequence() {
        let mut worker = Worker::new("time", "0.1.0", "abc").unwrap();
        assert_eq!(
            worker.handle(WorkerRequest::Replay {
                records: Vec::new()
            }),
            WorkerResponse::Ack {
                last_applied_seq: 0
            }
        );
        match worker.handle(WorkerRequest::Snapshot) {
            WorkerResponse::Snapshot {
                last_applied_seq, ..
            } => assert_eq!(last_applied_seq, 0),
            other => panic!("unexpected response: {other:?}"),
        }
    }
}
