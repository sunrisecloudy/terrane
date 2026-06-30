//! Shared fixtures for the per-capability engine tests.

use terrane_core::{Core, EffectRunner, Request, LOCAL_OWNER_SUBJECT};

/// Build a `Request` from a dotted name and string args.
pub(crate) fn req(name: &str, args: &[&str]) -> Request {
    Request::new(name, args.iter().map(|s| s.to_string()).collect())
}

pub(crate) fn grant_resource<R: EffectRunner>(core: &mut Core<R>, app: &str, namespace: &str) {
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, app, namespace]))
        .unwrap();
}
