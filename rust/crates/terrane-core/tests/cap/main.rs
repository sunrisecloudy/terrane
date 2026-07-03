//! Per-capability engine tests. Each module drives the
//! engine through its public surface (`Core::dispatch`) for one capability.
//! Shared fixtures live in `helpers`.

mod helpers;

mod agent;
mod app;
mod auth;
mod builder;
mod crdt;
mod crypto;
mod grant_resources;
mod grant_spec_inventory;
mod grant_verbs_match_specs;
mod harness;
mod host;
mod interface;
mod kv;
mod local_model;
mod log;
mod model;
mod native;
mod net;
mod relational_db;
mod replica;
mod stt;
mod wasm_runtime;
