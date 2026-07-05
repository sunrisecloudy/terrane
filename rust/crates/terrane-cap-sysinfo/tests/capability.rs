//! Integration tests over the public surface of `terrane-cap-sysinfo`.
//!
//! The capability is a validated forwarder to the edge `LiveHost`, so the proofs
//! cover: forwarding the read to the host, the pure-core error when no host is
//! present, rejecting unknown domains, and that it records nothing (no commands,
//! no events, an inert `fold`).

use std::any::Any;
use std::cell::RefCell;

use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Error, EventRecord, LiveHost, QueryValue, ReadValue,
    ResourceReadCtx, Result, StateStore,
};
use terrane_cap_sysinfo::{SysinfoCapability, DOMAINS};

struct EmptyStore;

impl StateStore for EmptyStore {
    fn get(&self, _namespace: &str) -> Option<&dyn Any> {
        None
    }

    fn get_mut(&mut self, _namespace: &str) -> Option<&mut dyn Any> {
        None
    }
}

struct NoBus;

impl CapBus for NoBus {
    fn query(&self, cap: &str, name: &str, _args: &[String]) -> Result<QueryValue> {
        Err(Error::InvalidInput(format!("unknown query: {cap}.{name}")))
    }
}

/// Records the last `(domain, args)` it was asked to sample and echoes a canned
/// JSON document, so a test can assert the capability forwarded verbatim.
#[derive(Default)]
struct StubHost {
    last: RefCell<Option<(String, Vec<String>)>>,
}

impl LiveHost for StubHost {
    fn sample(&self, domain: &str, args: &[String]) -> Result<String> {
        *self.last.borrow_mut() = Some((domain.to_string(), args.to_vec()));
        Ok(format!("{{\"domain\":\"{domain}\",\"args\":{}}}", args.len()))
    }
}

fn read(host: Option<&dyn LiveHost>, method: &str, args: &[String]) -> Result<ReadValue> {
    let store = EmptyStore;
    let bus = NoBus;
    SysinfoCapability.read_resource(
        ResourceReadCtx {
            state: &store,
            bus: &bus,
            app: "os-monitor",
            host,
        },
        method,
        args,
    )
}

#[test]
fn read_forwards_the_domain_and_args_to_the_live_host() {
    let host = StubHost::default();
    let value = read(Some(&host), "processes", &["cpu".into(), "5".into()]).unwrap();
    assert_eq!(
        value,
        ReadValue::OptString(Some("{\"domain\":\"processes\",\"args\":2}".to_string()))
    );
    assert_eq!(
        *host.last.borrow(),
        Some(("processes".to_string(), vec!["cpu".to_string(), "5".to_string()]))
    );
}

#[test]
fn every_declared_domain_is_forwarded() {
    let host = StubHost::default();
    for domain in DOMAINS {
        let value = read(Some(&host), domain, &[]).unwrap();
        let ReadValue::OptString(Some(json)) = value else {
            panic!("{domain} should return a JSON string");
        };
        assert!(json.contains(domain), "{domain} not echoed: {json}");
    }
}

#[test]
fn read_without_a_live_host_is_a_clear_error() {
    let err = read(None, "cpu", &[]).unwrap_err().to_string();
    assert!(err.contains("live host"), "unexpected error: {err}");
}

#[test]
fn unknown_domain_is_rejected_before_reaching_the_host() {
    let host = StubHost::default();
    let err = read(Some(&host), "gpu", &[]).unwrap_err().to_string();
    assert!(err.contains("unknown resource read"), "unexpected error: {err}");
    assert!(host.last.borrow().is_none(), "unknown domain must not sample");
}

#[test]
fn capability_declares_only_reads_and_no_recorded_surface() {
    let manifest = SysinfoCapability.manifest();
    assert!(manifest.commands.is_empty(), "sysinfo must have no commands");
    assert!(manifest.events.is_empty(), "sysinfo must have no events");
    assert_eq!(manifest.resources.len(), DOMAINS.len());
    for method in &manifest.resources {
        assert_eq!(method.kind(), "read", "{} must be a read", method.name());
    }
}

#[test]
fn decide_rejects_all_commands_and_fold_is_inert() {
    let store = EmptyStore;
    let bus = NoBus;
    let err = SysinfoCapability
        .decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "sysinfo.anything",
            &[],
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown command"), "unexpected error: {err}");

    // Folding any record is a no-op — nothing to persist, so replay is trivial.
    let mut store = EmptyStore;
    let record = EventRecord {
        kind: "sysinfo.whatever".to_string(),
        payload: Vec::new(),
        actor: String::new(),
    };
    SysinfoCapability.fold(&mut store, &record).unwrap();
}
