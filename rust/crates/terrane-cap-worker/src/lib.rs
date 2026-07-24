//! Native process adapter for Terrane's existing Rust capabilities.
//!
//! This compatibility worker deliberately reuses each capability's current
//! Rust implementation. The process boundary is the stable protocol crate;
//! no Rust trait object crosses into the host.

use std::any::Any;
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Error, EventRecord, GrantResourceSpec, QueryCtx, QueryValue,
    ResourceMethod, ResourceReadCtx, Result as CapResult, RuntimeCtx, RuntimeHost,
    RuntimeHostHandle, RuntimeRequest, StateStore,
};
use terrane_cap_protocol::{
    read_frame, write_frame, HostConnectorRequest, HostConnectorResponse, OwnedResourceMethod,
    WorkerRequest, WorkerResponse, PROTOCOL_VERSION,
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
    headless_effect: selected::HeadlessEffect,
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
            headless_effect: selected.headless_effect,
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
                overlay_records,
                dependencies: _,
            } => self.with_overlay(&overlay_records, |worker| {
                let bus = WorkerBus::new(
                    worker.capability.as_ref(),
                    &worker.support,
                    &worker.state,
                    &worker.declared_events,
                );
                let decision = worker.capability.decide(
                    CommandCtx {
                        state: &worker.state,
                        bus: &bus,
                    },
                    &request.name,
                    &request.args,
                )?;
                Ok(WorkerResponse::Decision { decision })
            }),
            WorkerRequest::Query {
                name,
                args,
                overlay_records,
                dependencies: _,
            } => self.with_overlay(&overlay_records, |worker| {
                let bus = WorkerBus::new(
                    worker.capability.as_ref(),
                    &worker.support,
                    &worker.state,
                    &worker.declared_events,
                );
                let value = worker.capability.query(
                    QueryCtx {
                        state: &worker.state,
                        bus: &bus,
                    },
                    &name,
                    &args,
                )?;
                Ok(WorkerResponse::QueryValue { value })
            }),
            WorkerRequest::ReadResource {
                app,
                name,
                args,
                overlay_records,
                dependencies: _,
            } => self.with_overlay(&overlay_records, |worker| {
                let bus = WorkerBus::new(
                    worker.capability.as_ref(),
                    &worker.support,
                    &worker.state,
                    &worker.declared_events,
                );
                let value = worker.capability.read_resource(
                    ResourceReadCtx {
                        state: &worker.state,
                        bus: &bus,
                        app: &app,
                        host: None,
                    },
                    &name,
                    &args,
                )?;
                Ok(WorkerResponse::ReadValue { value })
            }),
            WorkerRequest::ResourceCallOutput {
                app,
                method,
                records,
                overlay_records,
            } => self.with_overlay(&overlay_records, |worker| {
                let value = worker.capability.resource_call_output(
                    &worker.state,
                    &app,
                    &method,
                    &records,
                )?;
                Ok(WorkerResponse::ReadValue { value })
            }),
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
            WorkerRequest::ConnectorResponse { .. } => Err(Error::InvalidInput(
                "connector responses are only valid during runtime execution".into(),
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
        self.headless_effect = selected.headless_effect;
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

    fn with_overlay<T>(
        &mut self,
        records: &[EventRecord],
        action: impl FnOnce(&mut Self) -> CapResult<T>,
    ) -> CapResult<T> {
        if records.is_empty() {
            return action(self);
        }
        let baseline = self.snapshot()?;
        let baseline_seq = self.last_applied_seq;
        let fold_result = records.iter().try_for_each(|record| self.fold(record));
        let result = match fold_result {
            Ok(()) => action(self),
            Err(error) => Err(error),
        };
        self.reset();
        self.restore_snapshot(&baseline)?;
        self.last_applied_seq = baseline_seq;
        result
    }

    fn record_affects_worker(&self, record: &EventRecord) -> bool {
        capability_accepts(self.capability.as_ref(), record)
            || self
                .support
                .iter()
                .any(|capability| capability_accepts(capability.as_ref(), record))
    }

    fn run_runtime_with_connector(
        &self,
        app: String,
        source: String,
        source_files: Option<BTreeMap<String, String>>,
        app_name: String,
        input: Vec<String>,
        io: ConnectorIo,
    ) -> CapResult<WorkerResponse> {
        let host = RuntimeHostHandle::new(Box::new(ConnectorHost::new(io)));
        let output = self.capability.run_runtime(
            RuntimeCtx {
                source,
                source_files,
                app_name,
                host,
            },
            RuntimeRequest { app, input },
        )?;
        Ok(WorkerResponse::RuntimeOutput {
            output,
            records: Vec::new(),
        })
    }

    fn run_effect_with_connector(
        &self,
        effect: terrane_cap_interface::Effect,
        io: ConnectorIo,
    ) -> CapResult<WorkerResponse> {
        if let Some(records) = (self.headless_effect)(&self.state, &effect)? {
            return Ok(WorkerResponse::EffectRecords { records });
        }
        let connector = ConnectorHost::new(io);
        match connector.call(HostConnectorRequest::ExecuteEffect { effect })? {
            HostConnectorResponse::EffectRecords { records } => {
                Ok(WorkerResponse::EffectRecords { records })
            }
            response => Err(unexpected_connector_response("execute effect", response)),
        }
    }

    fn read_resource_with_connector(
        &mut self,
        app: String,
        name: String,
        args: Vec<String>,
        overlay_records: Vec<EventRecord>,
        io: ConnectorIo,
    ) -> CapResult<WorkerResponse> {
        let connector = ConnectorHost::new(io);
        self.with_overlay(&overlay_records, |worker| {
            let bus = WorkerBus::new(
                worker.capability.as_ref(),
                &worker.support,
                &worker.state,
                &worker.declared_events,
            );
            let value = worker.capability.read_resource(
                ResourceReadCtx {
                    state: &worker.state,
                    bus: &bus,
                    app: &app,
                    host: Some(&connector),
                },
                &name,
                &args,
            )?;
            Ok(WorkerResponse::ReadValue { value })
        })
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

#[derive(Clone)]
struct ConnectorIo {
    reader: Arc<Mutex<Box<dyn Read + Send>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

struct ConnectorHost {
    io: ConnectorIo,
    next_request_id: Cell<u64>,
}

impl ConnectorHost {
    fn new(io: ConnectorIo) -> Self {
        Self {
            io,
            next_request_id: Cell::new(1),
        }
    }

    fn call(&self, request: HostConnectorRequest) -> CapResult<HostConnectorResponse> {
        let request_id = self.next_request_id.get();
        self.next_request_id.set(request_id + 1);
        write_frame(
            &mut *self
                .io
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            &WorkerResponse::ConnectorRequest {
                request_id,
                request,
            },
        )
        .map_err(|error| Error::Runtime(format!("write host connector request: {error}")))?;
        let response: WorkerRequest = read_frame(
            &mut *self
                .io
                .reader
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .map_err(|error| Error::Runtime(format!("read host connector response: {error}")))?;
        match response {
            WorkerRequest::ConnectorResponse {
                request_id: response_id,
                response,
            } if response_id == request_id => match response {
                HostConnectorResponse::Error { message } => Err(Error::Runtime(message)),
                response => Ok(response),
            },
            other => Err(Error::Runtime(format!(
                "expected connector response {request_id}, got {other:?}"
            ))),
        }
    }
}

impl RuntimeHost for ConnectorHost {
    fn resource_methods(&self, namespace: &str) -> CapResult<Vec<ResourceMethod>> {
        match self.call(HostConnectorRequest::ResourceMethods {
            namespace: namespace.to_string(),
        })? {
            HostConnectorResponse::ResourceMethods { methods } => {
                methods.into_iter().map(owned_resource_method).collect()
            }
            response => Err(unexpected_connector_response("resource methods", response)),
        }
    }

    fn read_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> CapResult<terrane_cap_interface::ReadValue> {
        match self.call(HostConnectorRequest::ReadResource {
            namespace: namespace.to_string(),
            method: method.to_string(),
            args: args.to_vec(),
        })? {
            HostConnectorResponse::ReadValue { value } => Ok(value),
            response => Err(unexpected_connector_response("read resource", response)),
        }
    }

    fn write_resource(&mut self, namespace: &str, method: &str, args: &[String]) -> CapResult<()> {
        match self.call(HostConnectorRequest::WriteResource {
            namespace: namespace.to_string(),
            method: method.to_string(),
            args: args.to_vec(),
        })? {
            HostConnectorResponse::Ack => Ok(()),
            response => Err(unexpected_connector_response("write resource", response)),
        }
    }

    fn call_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> CapResult<terrane_cap_interface::ReadValue> {
        match self.call(HostConnectorRequest::CallResource {
            namespace: namespace.to_string(),
            method: method.to_string(),
            args: args.to_vec(),
        })? {
            HostConnectorResponse::ReadValue { value } => Ok(value),
            response => Err(unexpected_connector_response("call resource", response)),
        }
    }

    fn app_log(
        &mut self,
        level: &str,
        msg: &str,
        data: &str,
        source: &str,
        stack: &str,
        record_error: bool,
    ) -> CapResult<()> {
        match self.call(HostConnectorRequest::AppLog {
            level: level.to_string(),
            message: msg.to_string(),
            data: data.to_string(),
            source: source.to_string(),
            stack: stack.to_string(),
            record_error,
        })? {
            HostConnectorResponse::Ack => Ok(()),
            response => Err(unexpected_connector_response("app log", response)),
        }
    }

    fn take_records(&mut self) -> Vec<EventRecord> {
        Vec::new()
    }
}

impl terrane_cap_interface::LiveHost for ConnectorHost {
    fn sample(&self, domain: &str, args: &[String]) -> CapResult<String> {
        match self.call(HostConnectorRequest::LiveSample {
            domain: domain.to_string(),
            args: args.to_vec(),
        })? {
            HostConnectorResponse::LiveSample { value } => Ok(value),
            response => Err(unexpected_connector_response("live sample", response)),
        }
    }
}

fn unexpected_connector_response(action: &str, response: HostConnectorResponse) -> Error {
    Error::Runtime(format!("host connector returned {response:?} for {action}"))
}

fn owned_resource_method(method: OwnedResourceMethod) -> CapResult<ResourceMethod> {
    let name: &'static str = Box::leak(method.name.into_boxed_str());
    let params = method
        .params
        .into_iter()
        .map(|value| Box::leak(value.into_boxed_str()) as &'static str)
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let params: &'static [&'static str] = Box::leak(params);
    match method.kind.as_str() {
        "read" => Ok(ResourceMethod::Read { name, params }),
        "write" => Ok(ResourceMethod::Write { name, params }),
        "call" => Ok(ResourceMethod::Call { name, params }),
        kind => Err(Error::Runtime(format!(
            "host connector returned unknown resource method kind {kind}"
        ))),
    }
}

pub fn serve<R, W>(
    worker: &mut Worker,
    reader: R,
    writer: W,
) -> Result<(), terrane_cap_protocol::ProtocolError>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
{
    let io = ConnectorIo {
        reader: Arc::new(Mutex::new(Box::new(reader))),
        writer: Arc::new(Mutex::new(Box::new(writer))),
    };
    loop {
        let request: WorkerRequest = match read_frame(
            &mut *io
                .reader
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        ) {
            Ok(request) => request,
            Err(terrane_cap_protocol::ProtocolError::Eof) => return Ok(()),
            Err(error) => return Err(error),
        };
        let shutdown = matches!(request, WorkerRequest::Shutdown);
        let response = match request {
            WorkerRequest::RunRuntime {
                app,
                source,
                source_files,
                app_name,
                input,
                principal: _,
            } => match worker.run_runtime_with_connector(
                app,
                source,
                source_files,
                app_name,
                input,
                io.clone(),
            ) {
                Ok(response) => response,
                Err(error) => WorkerResponse::Error {
                    code: error_code(&error).into(),
                    message: error.to_string(),
                    retryable: matches!(error, Error::Runtime(_) | Error::Storage(_)),
                },
            },
            WorkerRequest::ExecuteEffect { effect } => {
                match worker.run_effect_with_connector(effect, io.clone()) {
                    Ok(response) => response,
                    Err(error) => WorkerResponse::Error {
                        code: error_code(&error).into(),
                        message: error.to_string(),
                        retryable: matches!(error, Error::Runtime(_) | Error::Storage(_)),
                    },
                }
            }
            WorkerRequest::ReadResource {
                app,
                name,
                args,
                overlay_records,
                dependencies: _,
            } => match worker.read_resource_with_connector(
                app,
                name,
                args,
                overlay_records,
                io.clone(),
            ) {
                Ok(response) => response,
                Err(error) => WorkerResponse::Error {
                    code: error_code(&error).into(),
                    message: error.to_string(),
                    retryable: matches!(error, Error::Runtime(_) | Error::Storage(_)),
                },
            },
            request => worker.handle(request),
        };
        write_frame(
            &mut *io
                .writer
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
            &response,
        )?;
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
