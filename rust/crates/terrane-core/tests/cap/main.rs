//! Per-capability engine tests. Each module drives the
//! engine through its public surface (`Core::dispatch`) for one capability.
//! Shared fixtures live in `helpers`.

mod helpers;

mod actor;
mod agent;
mod app;
mod auth;
mod blob;
mod builder;
mod crdt;
mod crypto;
mod grant_resources;
mod grant_spec_inventory;
mod grant_verbs_match_specs;
mod harness;
mod history;
mod host;
mod interface;
mod kv;
mod local_model;
mod log;
mod model;
mod native;
mod net;
mod query;
mod relational_db;
mod replica;
mod scheduler;
mod search;
mod stt;
mod time;
mod wasm_runtime;
