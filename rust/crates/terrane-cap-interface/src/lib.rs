//! Shared capability ABI for Terrane built-in and external capability crates.

mod abi;
mod capability;
mod doc;
mod helpers;
mod manifest;
mod runtime;
mod state;

pub use abi::{
    decode_event, encode_event, format_item_uri, namespace_of, parse_item_uri, AppId,
    CommandAuthority, Decision, Effect, Error, EventRecord, ExecutionPrincipal, ItemUri, Request,
    Result, RuntimeOutput, RuntimeRequest, LOCAL_ORG, LOCAL_OWNER_SUBJECT, LOCAL_SOURCE,
};
pub use capability::{Capability, RecordedCallCap};
pub use doc::{
    command_doc, event_doc, limit, param, query_doc, resource_method, schema, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, InternalNote, LimitDoc, ParamDoc,
    QueryDoc, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};
pub use helpers::{
    app_exists, arg, decode_app_removed, ensure_app_exists, extract_json_object, join_tail,
    non_empty, non_empty_or, parse_usize_arg, replica_peer, required_tail, truncate, AppRemoved,
};
pub use manifest::{
    CapManifest, CommandSpec, EventPattern, EventSpec, GrantResourceCompatibility,
    GrantResourceSpec, QuerySpec, ResourceMethod, UnknownSelectorSchemaPolicy,
    NAMESPACE_SELECTOR_SCHEMA_ID, NAMESPACE_SELECTOR_SCHEMA_JSON,
};
pub use runtime::{
    CapBus, CommandCtx, LiveHost, QueryCtx, QueryValue, ReadValue, ResourceReadCtx, RuntimeCtx,
    RuntimeHost, RuntimeHostHandle,
};
pub use state::{state_mut, state_ref, StateStore};

#[cfg(test)]
mod tests;
