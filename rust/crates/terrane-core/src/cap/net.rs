//! The `net` capability — recorded network fetches. The fetch itself is an
//! [`Effect`](crate::Effect) run at the edge; its result is recorded as an event,
//! so replay reproduces it without the network. Reacts to `app.removed`.

use std::collections::BTreeMap;

use crate::{AppId, Error, EventRecord, Result};
use borsh::{BorshDeserialize, BorshSerialize};

use super::{arg, Capability};
use crate::{decode_event, encode_event, Decision, Effect, State};

/// A recorded network response, rebuilt by folding a `net.fetched` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    pub status: u16,
    pub body: String,
}

/// This capability's slice of State: per-app recorded responses, keyed by URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetState {
    pub fetches: BTreeMap<AppId, BTreeMap<String, FetchResponse>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Fetched {
    app: String,
    url: String,
    status: u16,
    body: String,
}

/// Build the recorded event for a completed fetch. Called by an
/// [`EffectRunner`](crate::EffectRunner) once it has performed the GET, so the
/// `"net.fetched"` kind and payload shape stay owned by this capability.
pub fn fetched_event(app: &str, url: &str, status: u16, body: String) -> Result<EventRecord> {
    encode_event(
        "net.fetched",
        &Fetched {
            app: app.to_string(),
            url: url.to_string(),
            status,
            body,
        },
    )
}

pub struct NetCapability;

impl Capability for NetCapability {
    fn namespace(&self) -> &'static str {
        "net"
    }

    fn decide(&self, state: &State, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "net.fetch" => {
                let app = arg(args, 0, "app")?;
                let url = arg(args, 1, "url")?;
                // Validate purely; the result is produced by the runner at the edge.
                if !state.app.apps.contains_key(&app) {
                    return Err(Error::AppNotFound(app));
                }
                if url.trim().is_empty() {
                    return Err(Error::InvalidInput("url must not be empty".into()));
                }
                Ok(Decision::Effect(Effect::HttpGet { app, url }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut State, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "net.fetched" => {
                let e: Fetched = decode_event(record)?;
                state.net.fetches.entry(e.app).or_default().insert(
                    e.url,
                    FetchResponse {
                        status: e.status,
                        body: e.body,
                    },
                );
            }
            "app.removed" => {
                #[derive(BorshDeserialize)]
                struct Removed {
                    id: String,
                }
                let e: Removed = decode_event(record)?;
                state.net.fetches.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "net.fetched" {
            let e: Fetched = decode_event(record).ok()?;
            return Some(format!(
                "net.fetched {} {} → {} ({} bytes)",
                e.app,
                e.url,
                e.status,
                e.body.len()
            ));
        }
        None
    }
}
