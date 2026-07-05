use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::abi::{EventRecord, Result, RuntimeRequest};
use crate::manifest::GrantResourceSpec;

/// A read-only value returned by a capability query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryValue {
    Bool(bool),
    U64(Option<u64>),
    Json(String),
}

/// Read-only access from one capability into another.
pub trait CapBus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue>;

    fn grant_resource_spec(
        &self,
        namespace: &str,
        selector_schema_id: &str,
    ) -> Result<Option<GrantResourceSpec>> {
        let _ = namespace;
        let _ = selector_schema_id;
        Ok(None)
    }
}

/// Context handed to command decisions.
#[derive(Clone, Copy)]
pub struct CommandCtx<'a> {
    pub state: &'a dyn crate::StateStore,
    pub bus: &'a dyn CapBus,
}

/// Context handed to read-only capability queries.
#[derive(Clone, Copy)]
pub struct QueryCtx<'a> {
    pub state: &'a dyn crate::StateStore,
    pub bus: &'a dyn CapBus,
}

/// Live host-environment access for reads that observe the outside world (system
/// metrics, sensors) rather than folded state. Distinct from
/// [`Effect`](crate::abi::Effect): a live read is performed at the edge but
/// records **nothing**, so it never enters the log and replay-identity is
/// preserved (the app sees a fresh sample each call and only what it *persists*
/// via ordinary writes is ever replayed). Implemented by the host's edge runner;
/// absent (`None`) in pure cores, where such reads simply fail.
pub trait LiveHost {
    /// Sample a named `domain` (e.g. `"cpu"`, `"memory"`, `"snapshot"`) with the
    /// read's positional `args`, returning a JSON document for the app backend.
    fn sample(&self, domain: &str, args: &[String]) -> Result<String>;
}

/// Context handed to backend resource reads.
#[derive(Clone, Copy)]
pub struct ResourceReadCtx<'a> {
    pub state: &'a dyn crate::StateStore,
    pub bus: &'a dyn CapBus,
    pub app: &'a str,
    /// Edge access for live (non-recorded) reads, when the host provides one.
    pub host: Option<&'a dyn LiveHost>,
}

/// A value a resource read hands back to backend JS/WASM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadValue {
    OptString(Option<String>),
    StringMap(BTreeMap<String, String>),
    StringList(Vec<String>),
}

/// A runtime engine's controlled access to Terrane resources.
pub trait RuntimeHost {
    fn resource_methods(&self, namespace: &str) -> Result<Vec<crate::ResourceMethod>>;

    fn read_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue>;

    fn write_resource(&mut self, namespace: &str, method: &str, args: &[String]) -> Result<()>;

    /// Run an effectful resource call: records events like a write and
    /// returns a value like a read. Default: unsupported.
    fn call_resource(
        &mut self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let _ = args;
        Err(crate::abi::Error::Runtime(format!(
            "{namespace}.{method}: resource calls are not supported by this runtime host"
        )))
    }

    /// Runtime-only app telemetry hook. The JS sandbox uses this for `console.*`
    /// and exception mirroring so logging can still reach the local edge buffer
    /// even when `ctx.resource.telemetry` is not installed. Implementations may
    /// record an event for error-class facts when policy allows it.
    fn app_log(
        &mut self,
        level: &str,
        msg: &str,
        data: &str,
        source: &str,
        stack: &str,
        record_error: bool,
    ) -> Result<()> {
        let _ = (level, msg, data, source, stack, record_error);
        Ok(())
    }

    fn take_records(&mut self) -> Vec<EventRecord>;
}

/// Shareable runtime host handle. Runtime engines capture this inside guest-code
/// callbacks while core keeps ownership of commit/replay.
#[derive(Clone)]
pub struct RuntimeHostHandle {
    inner: Rc<RefCell<Box<dyn RuntimeHost>>>,
}

impl RuntimeHostHandle {
    pub fn new(host: Box<dyn RuntimeHost>) -> Self {
        Self {
            inner: Rc::new(RefCell::new(host)),
        }
    }

    pub fn resource_methods(&self, namespace: &str) -> Result<Vec<crate::ResourceMethod>> {
        self.inner.borrow().resource_methods(namespace)
    }

    pub fn read_resource(
        &self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        self.inner
            .borrow_mut()
            .read_resource(namespace, method, args)
    }

    pub fn write_resource(&self, namespace: &str, method: &str, args: &[String]) -> Result<()> {
        self.inner
            .borrow_mut()
            .write_resource(namespace, method, args)
    }

    pub fn call_resource(
        &self,
        namespace: &str,
        method: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        self.inner
            .borrow_mut()
            .call_resource(namespace, method, args)
    }

    pub fn app_log(
        &self,
        level: &str,
        msg: &str,
        data: &str,
        source: &str,
        stack: &str,
        record_error: bool,
    ) -> Result<()> {
        self.inner
            .borrow_mut()
            .app_log(level, msg, data, source, stack, record_error)
    }

    pub fn take_records(&self) -> Vec<EventRecord> {
        self.inner.borrow_mut().take_records()
    }
}

/// Context handed to runtime capabilities.
#[derive(Clone)]
pub struct RuntimeCtx {
    pub source: String,
    pub source_files: Option<BTreeMap<String, String>>,
    pub app_name: String,
    pub host: RuntimeHostHandle,
}

impl RuntimeRequest {
    pub fn input_tail(&self) -> &[String] {
        &self.input
    }
}
