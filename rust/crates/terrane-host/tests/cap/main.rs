//! Per-capability end-to-end tests. Each module drives the
//! real `terrane` binary against a throwaway `$TERRANE_HOME`. Shared fixtures
//! live in `helpers`.
//!
//! `app`/`kv`/`host` are deterministic + local and run by default;
//! `net`/`model`/`local_model` do real effects and are `#[ignore]`d — run with
//! `cargo test -p terrane-host -- --ignored`.

mod helpers;

mod app;
mod host;
mod kv;
mod local_model;
mod model;
mod native;
mod net;
mod search;
mod sysinfo;
