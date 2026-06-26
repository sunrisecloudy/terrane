//! Per-capability end-to-end tests, mirroring `src/cap/`. Each module drives the
//! real `terrane` binary against a throwaway `$TERRANE_HOME`. Shared fixtures
//! live in `helpers`.
//!
//! `app`/`kv` are pure and run by default; `net`/`model` do real effects and are
//! `#[ignore]`d — run with `cargo test -p terrane-cli -- --ignored`.

mod helpers;

mod app;
mod kv;
mod model;
mod net;
