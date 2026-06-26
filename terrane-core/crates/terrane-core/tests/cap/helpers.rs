//! Shared fixtures for the per-capability engine tests.

use terrane_core::cap::model::responded_event;
use terrane_core::cap::net::fetched_event;
use terrane_core::{Effect, EffectRunner};
use terrane_domain::{EventRecord, Request, Result};

/// Build a `Request` from a dotted name and string args.
pub(crate) fn req(name: &str, args: &[&str]) -> Request {
    Request::new(name, args.iter().map(|s| s.to_string()).collect())
}

/// A deterministic stand-in for the edge: canned responses for every effect, so
/// tests never touch the network or spawn a real agent.
pub(crate) struct FakeEdge;

impl EffectRunner for FakeEdge {
    fn run(&self, effect: &Effect) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => {
                Ok(vec![fetched_event(app, url, 200, format!("body for {url}"))?])
            }
            Effect::ModelCall { app, agent, prompt } => Ok(vec![responded_event(
                app,
                agent,
                prompt,
                format!("{agent} says: {prompt}"),
                0,
            )?]),
        }
    }
}
