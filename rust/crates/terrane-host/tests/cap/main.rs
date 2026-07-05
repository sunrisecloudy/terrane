//! Per-capability end-to-end tests. Each module drives the
//! real `terrane` binary against a throwaway `$TERRANE_HOME`. Shared fixtures
//! live in `helpers`.
//!
//! `app`/`kv`/`host` are deterministic + local and run by default;
//! `net`/`model`/`local_model` do real effects and are `#[ignore]`d — run with
//! `cargo test -p terrane-host -- --ignored`.

mod helpers;

mod app;
mod blob;
mod browser;
mod common;
mod connection;
mod deep_links;
mod document;
mod geo;
mod history;
mod host;
mod i18n;
mod interop;
mod kv;
mod local_model;
mod media;
mod mcp;
mod model;
mod native;
mod net;
mod password_manager;
mod query;
mod search;
mod sysinfo;
mod telemetry;
mod time;
mod tts;
