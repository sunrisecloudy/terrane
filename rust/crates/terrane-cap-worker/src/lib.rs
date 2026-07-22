//! Native process adapter for Terrane's existing Rust capabilities.
//!
//! This compatibility worker deliberately reuses each capability's current
//! Rust implementation. The process boundary is the stable protocol crate;
//! no Rust trait object crosses into the host.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    CapBus, CommandCtx, Error, QueryCtx, ResourceReadCtx, Result as CapResult, StateStore,
};
use terrane_cap_protocol::{
    read_frame, write_frame, WorkerRequest, WorkerResponse, PROTOCOL_VERSION,
};
use terrane_core::{apply, default_registry, Registry, RegistryBus, State};

pub mod package;

pub struct Worker {
    namespace: String,
    version: String,
    manifest_sha256: String,
    registry: Registry,
    state: State,
    last_applied_seq: u64,
}

impl Worker {
    pub fn new(
        namespace: impl Into<String>,
        version: impl Into<String>,
        manifest_sha256: impl Into<String>,
    ) -> CapResult<Self> {
        let namespace = namespace.into();
        let registry = default_registry();
        registry.get(&namespace)?;
        Ok(Self {
            namespace,
            version: version.into(),
            manifest_sha256: manifest_sha256.into(),
            registry,
            state: State::default(),
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
                self.state = State::default();
                self.last_applied_seq = 0;
                for (index, record) in records.iter().enumerate() {
                    apply(&self.registry, &mut self.state, record)?;
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
                if !snapshot.is_empty() {
                    self.registry
                        .get(&self.namespace)?
                        .restore(&mut self.state as &mut dyn StateStore, &snapshot)?;
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
                apply(&self.registry, &mut self.state, &record)?;
                self.last_applied_seq = seq;
                Ok(WorkerResponse::Ack {
                    last_applied_seq: seq,
                })
            }
            WorkerRequest::Decide {
                request,
                dependencies: _,
            } => {
                let bus = RegistryBus::new(&self.registry, &self.state);
                let decision = self.registry.get(&self.namespace)?.decide(
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
                let bus = RegistryBus::new(&self.registry, &self.state);
                let value = self.registry.get(&self.namespace)?.query(
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
                let bus = RegistryBus::new(&self.registry, &self.state);
                let value = self.registry.get(&self.namespace)?.read_resource(
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
                let value = self.registry.get(&self.namespace)?.resource_call_output(
                    &self.state,
                    &app,
                    &method,
                    &records,
                )?;
                Ok(WorkerResponse::ReadValue { value })
            }
            WorkerRequest::Snapshot => {
                let payload = self
                    .registry
                    .get(&self.namespace)?
                    .snapshot(&self.state)?
                    .unwrap_or_default();
                Ok(WorkerResponse::Snapshot {
                    state_sha256: sha256(&payload),
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
            WorkerRequest::BackgroundStatus => Ok(WorkerResponse::BackgroundStatus {
                keep_alive: false,
                reason: "no background activation adapter registered".into(),
            }),
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
