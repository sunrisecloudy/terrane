//! Shared fixtures for the per-capability engine tests.

use terrane_core::Request;

/// Build a `Request` from a dotted name and string args.
pub(crate) fn req(name: &str, args: &[&str]) -> Request {
    Request::new(name, args.iter().map(|s| s.to_string()).collect())
}
