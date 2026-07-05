//! The `net` capability — recorded network fetches. The fetch itself is an
//! [`Effect`](crate::Effect) run at the edge; its result is recorded as an event,
//! so replay reproduces it without the network. Reacts to `app.removed`.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, AppId,
    CapManifest, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, Result, StateStore,
};

mod doc;
pub mod request;

/// A recorded network response, rebuilt by folding a `net.fetched` event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchResponse {
    pub status: u16,
    pub body: String,
}

/// A recorded HTTP response for `net.request`, keyed by canonical request hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedResponse {
    pub request_json_redacted: String,
    pub status: u16,
    pub response_headers: BTreeMap<String, String>,
    pub body_kind: String,
    pub body: String,
    pub body_is_base64: bool,
    pub body_hash: String,
    pub body_size: u64,
    pub body_mime: String,
}

/// This capability's slice of State: per-app recorded responses, keyed by URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetState {
    pub fetches: BTreeMap<AppId, BTreeMap<String, FetchResponse>>,
    pub requests: BTreeMap<AppId, BTreeMap<String, RecordedResponse>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Fetched {
    app: String,
    url: String,
    status: u16,
    body: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Responded {
    app: String,
    request_key: String,
    request_json_redacted: String,
    status: u16,
    response_headers: BTreeMap<String, String>,
    body_kind: String,
    body: String,
    body_is_base64: bool,
    body_hash: String,
    body_size: u64,
    body_mime: String,
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

pub fn responded_event(
    app: impl Into<String>,
    request_key: impl Into<String>,
    request_json_redacted: impl Into<String>,
    status: u16,
    response_headers: BTreeMap<String, String>,
    body: RecordedBody,
) -> Result<EventRecord> {
    encode_event(
        "net.responded",
        &Responded {
            app: app.into(),
            request_key: request_key.into(),
            request_json_redacted: request_json_redacted.into(),
            status,
            response_headers,
            body_kind: body.kind,
            body: body.body,
            body_is_base64: body.is_base64,
            body_hash: body.hash,
            body_size: body.size,
            body_mime: body.mime,
        },
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedBody {
    pub kind: String,
    pub body: String,
    pub is_base64: bool,
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

pub fn decode_recorded_response(record: &EventRecord) -> Result<(String, String, RecordedResponse)> {
    let e: Responded = decode_event(record)?;
    Ok((
        e.app,
        e.request_key,
        RecordedResponse {
            request_json_redacted: e.request_json_redacted,
            status: e.status,
            response_headers: e.response_headers,
            body_kind: e.body_kind,
            body: e.body,
            body_is_base64: e.body_is_base64,
            body_hash: e.body_hash,
            body_size: e.body_size,
            body_mime: e.body_mime,
        },
    ))
}

pub struct NetCapability;

impl Capability for NetCapability {
    fn namespace(&self) -> &'static str {
        "net"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec { name: "net.fetch" },
                CommandSpec {
                    name: "net.request",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "net.fetched",
                },
                EventSpec {
                    kind: "net.responded",
                },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "get",
                    params: &["url"],
                },
                ResourceMethod::Call {
                    name: "call",
                    params: &["request_json"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "net",
                &["call"],
                "HTTP requests for recorded full HTTP effects and transient live calls.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::net_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "net.fetch" => {
                let app = arg(args, 0, "app")?;
                let url = arg(args, 1, "url")?;
                // Validate purely; the result is produced by the runner at the edge.
                ensure_app_exists(ctx.bus, &app)?;
                if url.trim().is_empty() {
                    return Err(Error::InvalidInput("url must not be empty".into()));
                }
                Ok(Decision::Effect(Effect::HttpGet { app, url }))
            }
            "net.request" => {
                let app = arg(args, 0, "app")?;
                let request_json = arg(args, 1, "request_json")?;
                ensure_app_exists(ctx.bus, &app)?;
                let prepared = request::prepare_request(&request_json)?;
                Ok(Decision::Effect(Effect::HttpRequest {
                    app,
                    request: prepared.canonical_json,
                }))
            }
            // The app-callable resource: a live GET whose result is returned to
            // the backend but never recorded. Same edge effect, but wrapped in
            // TransientEffect so the core does not persist net.fetched.
            "net.get" => {
                let app = arg(args, 0, "app")?;
                let url = arg(args, 1, "url")?;
                ensure_app_exists(ctx.bus, &app)?;
                if url.trim().is_empty() {
                    return Err(Error::InvalidInput("url must not be empty".into()));
                }
                Ok(Decision::TransientEffect(Effect::HttpGet { app, url }))
            }
            "net.call" => {
                let app = arg(args, 0, "app")?;
                let request_json = arg(args, 1, "request_json")?;
                ensure_app_exists(ctx.bus, &app)?;
                let prepared = request::prepare_request(&request_json)?;
                Ok(Decision::TransientEffect(Effect::HttpRequest {
                    app,
                    request: prepared.canonical_json,
                }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "get" => {
                let record = records
                    .first()
                    .ok_or_else(|| Error::Runtime("net.get produced no response".into()))?;
                let fetched: Fetched = decode_event(record)?;
                Ok(ReadValue::OptString(Some(fetched.body)))
            }
            "call" => {
                let record = records
                    .iter()
                    .find(|record| record.kind == "net.responded")
                    .ok_or_else(|| Error::Runtime("net.call produced no response".into()))?;
                let (_, request_key, response) = decode_recorded_response(record)?;
                if response.body_kind == "inline" {
                    return Ok(ReadValue::OptString(Some(response.body)));
                }
                Err(Error::Runtime(format!(
                    "net.call response body is in blob __net__/{request_key}; grant blob and use ctx.resource.blob.get to read it"
                )))
            }
            other => Err(Error::InvalidInput(format!(
                "net.{other} is not a callable resource"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "net.fetched" => {
                let e: Fetched = decode_event(record)?;
                state_mut::<NetState>(state, "net")?
                    .fetches
                    .entry(e.app)
                    .or_default()
                    .insert(
                        e.url,
                        FetchResponse {
                            status: e.status,
                            body: e.body,
                        },
                    );
            }
            "net.responded" => {
                let (app, request_key, response) = decode_recorded_response(record)?;
                state_mut::<NetState>(state, "net")?
                    .requests
                    .entry(app)
                    .or_default()
                    .insert(request_key, response);
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                let state = state_mut::<NetState>(state, "net")?;
                state.fetches.remove(&e.id);
                state.requests.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "net.fetched" => {
                let e: Fetched = decode_event(record).ok()?;
                Some(format!(
                    "net.fetched {} {} → {} ({} bytes)",
                    e.app,
                    e.url,
                    e.status,
                    e.body.len()
                ))
            }
            "net.responded" => {
                let e: Responded = decode_event(record).ok()?;
                let request: serde_json::Value = serde_json::from_str(&e.request_json_redacted).ok()?;
                let method = request.get("method").and_then(|v| v.as_str()).unwrap_or("GET");
                let host_path = request
                    .get("url")
                    .and_then(|v| v.as_str())
                    .map(host_and_path_without_query)
                    .unwrap_or_else(|| "<url>".to_string());
                Some(format!(
                    "net.responded {} {} {} → {} ({} bytes)",
                    e.app, method, host_path, e.status, e.body_size
                ))
            }
            _ => None,
        }
    }
}

fn host_and_path_without_query(url: &str) -> String {
    let after_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let without_fragment = after_scheme
        .split_once('#')
        .map(|(left, _)| left)
        .unwrap_or(after_scheme);
    let without_query = without_fragment
        .split_once('?')
        .map(|(left, _)| left)
        .unwrap_or(without_fragment);
    if without_query.is_empty() {
        "<url>".to_string()
    } else {
        without_query.to_string()
    }
}
