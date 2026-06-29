use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::abi::{EventRecord, Result, RuntimeRequest};

/// A read-only value returned by a capability query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryValue {
    Bool(bool),
    U64(Option<u64>),
}

/// Read-only access from one capability into another.
pub trait CapBus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> Result<QueryValue>;
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

/// Context handed to backend resource reads.
#[derive(Clone, Copy)]
pub struct ResourceReadCtx<'a> {
    pub state: &'a dyn crate::StateStore,
    pub bus: &'a dyn CapBus,
    pub app: &'a str,
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

    pub fn take_records(&self) -> Vec<EventRecord> {
        self.inner.borrow_mut().take_records()
    }
}

/// Context handed to runtime capabilities.
#[derive(Clone)]
pub struct RuntimeCtx {
    pub source: String,
    pub app_name: String,
    pub host: RuntimeHostHandle,
}

impl RuntimeRequest {
    pub fn input_tail(&self) -> &[String] {
        &self.input
    }
}
