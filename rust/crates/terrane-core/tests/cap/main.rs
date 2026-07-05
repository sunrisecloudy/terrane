//! Per-capability engine tests. Each module drives the
//! engine through its public surface (`Core::dispatch`) for one capability.
//! Shared fixtures live in `helpers`.

mod helpers;

mod actor;
mod agent;
mod applescript;
mod app;
mod auth;
mod automation;
mod blob;
mod browser;
mod builder;
mod common;
mod connection;
mod crdt;
mod crypto;
mod document;
mod geo;
mod grant_resources;
mod grant_spec_inventory;
mod grant_verbs_match_specs;
mod harness;
mod history;
mod host;
mod interop;
mod job_queue;
mod interface;
mod kv;
mod local_model;
mod log;
mod media;
mod migration;
mod mcp;
mod model;
mod native;
mod net;
mod person;
mod query;
mod relational_db;
mod replica;
mod scheduler;
mod search;
mod stt;
mod sync;
mod stream;
mod telemetry;
mod time;
mod tts;
mod wasm_runtime;
mod webhook;
