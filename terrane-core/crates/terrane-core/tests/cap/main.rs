//! Per-capability engine tests, mirroring `src/cap/`. Each module drives the
//! engine through its public surface (`Core::dispatch`) for one capability.
//! Shared fixtures live in `helpers`.

mod helpers;

mod app;
mod builder;
mod crdt;
mod host;
mod kv;
mod log;
mod model;
mod net;
mod replica;
