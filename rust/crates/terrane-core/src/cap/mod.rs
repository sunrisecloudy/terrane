//! Capabilities — the pluggable units of the engine.
//!
//! `terrane-core::cap` keeps the historical module path for built-in caps and
//! re-exports the shared capability API from `terrane-cap-api`.

pub use terrane_cap_api::*;

pub mod host;

pub use terrane_cap_app as app;
pub use terrane_cap_build as build;
pub use terrane_cap_builder as builder;
pub use terrane_cap_crdt as crdt;
pub use terrane_cap_harness as harness;
pub use terrane_cap_kv as kv;
pub use terrane_cap_model as model;
pub use terrane_cap_net as net;
pub use terrane_cap_replica as replica;
