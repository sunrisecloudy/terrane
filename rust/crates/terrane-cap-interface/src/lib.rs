//! Shared capability ABI for Terrane built-in and external capability crates.

mod abi;
mod capability;
mod doc;
mod helpers;
mod manifest;
mod runtime;
mod state;

pub use abi::{
    decode_event, encode_event, namespace_of, AppId, Decision, Effect, Error, EventRecord, Request,
    Result, RuntimeOutput, RuntimeRequest,
};
pub use capability::Capability;
pub use doc::{
    limit, param, resource_method, schema, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, LimitDoc, ParamDoc, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};
pub use helpers::{
    app_exists, arg, decode_app_removed, ensure_app_exists, extract_json_object, join_tail,
    non_empty, non_empty_or, parse_usize_arg, replica_peer, required_tail, truncate, AppRemoved,
};
pub use manifest::{CapManifest, CommandSpec, EventPattern, EventSpec, QuerySpec, ResourceMethod};
pub use runtime::{
    CapBus, CommandCtx, QueryCtx, QueryValue, ReadValue, ResourceReadCtx, RuntimeCtx, RuntimeHost,
    RuntimeHostHandle,
};
pub use state::{state_mut, state_ref, StateStore};

#[cfg(test)]
mod tests;
