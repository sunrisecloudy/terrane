//! The `browser` capability — recorded headless page rendering.
//!
//! The render itself is an [`Effect`](terrane_cap_interface::Effect) run at the
//! edge. Replay folds `browser.rendered` and never launches a browser.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, AppId,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, GrantResourceSpec, ReadValue, RecordedCallCap, ResourceMethod, Result,
    StateStore,
};

mod doc;
pub mod request;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRender {
    pub request_json_redacted: String,
    pub url: String,
    pub output: String,
    pub status: u16,
    pub body_kind: String,
    pub body: String,
    pub body_hash: String,
    pub size: u64,
    pub mime: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserState {
    pub renders: BTreeMap<AppId, BTreeMap<String, RecordedRender>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Rendered {
    app: String,
    request_key: String,
    request_json_redacted: String,
    url: String,
    output: String,
    status: u16,
    body_kind: String,
    body: String,
    body_hash: String,
    size: u64,
    mime: String,
    title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedBody {
    pub kind: String,
    pub body: String,
    pub hash: String,
    pub size: u64,
    pub mime: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedEvent {
    pub app: String,
    pub request_key: String,
    pub request_json_redacted: String,
    pub url: String,
    pub output: String,
    pub status: u16,
    pub body: RecordedBody,
    pub title: String,
}

pub fn rendered_event(input: RenderedEvent) -> Result<EventRecord> {
    encode_event(
        "browser.rendered",
        &Rendered {
            app: input.app,
            request_key: input.request_key,
            request_json_redacted: input.request_json_redacted,
            url: input.url,
            output: input.output,
            status: input.status,
            body_kind: input.body.kind,
            body: input.body.body,
            body_hash: input.body.hash,
            size: input.body.size,
            mime: input.body.mime,
            title: input.title,
        },
    )
}

pub fn decode_recorded_render(record: &EventRecord) -> Result<(String, String, RecordedRender)> {
    let e: Rendered = decode_event(record)?;
    Ok((
        e.app,
        e.request_key,
        RecordedRender {
            request_json_redacted: e.request_json_redacted,
            url: e.url,
            output: e.output,
            status: e.status,
            body_kind: e.body_kind,
            body: e.body,
            body_hash: e.body_hash,
            size: e.size,
            mime: e.mime,
            title: e.title,
        },
    ))
}

pub struct BrowserCapability;

impl Capability for BrowserCapability {
    fn namespace(&self) -> &'static str {
        "browser"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "browser.render",
            }],
            events: vec![EventSpec {
                kind: "browser.rendered",
            }],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "render",
                    params: &["request_json"],
                },
                ResourceMethod::Call {
                    name: "peek",
                    params: &["request_json"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "browser",
                &["call"],
                "Load and execute web pages in a hidden browser.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::browser_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "browser.render" => {
                let app = arg(args, 0, "app")?;
                let request_json = arg(args, 1, "request_json")?;
                ensure_app_exists(ctx.bus, &app)?;
                let prepared = request::prepare_render(&request_json)?;
                Ok(Decision::Effect(Effect::BrowserRender {
                    app,
                    request: prepared.canonical_json,
                }))
            }
            "browser.peek" => {
                let app = arg(args, 0, "app")?;
                let request_json = arg(args, 1, "request_json")?;
                ensure_app_exists(ctx.bus, &app)?;
                let prepared = request::prepare_render(&request_json)?;
                Ok(Decision::TransientEffect(Effect::BrowserRender {
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
            "render" | "peek" => {
                let record = records
                    .iter()
                    .find(|record| record.kind == "browser.rendered")
                    .ok_or_else(|| Error::Runtime(format!("browser.{method} produced no render")))?;
                let (_, request_key, render) = decode_recorded_render(record)?;
                if render.body_kind == "inline" {
                    return Ok(ReadValue::OptString(Some(render.body)));
                }
                Err(Error::Runtime(format!(
                    "browser.{method} output is in blob __browser__/{request_key}; grant blob and use ctx.resource.blob.get to read it"
                )))
            }
            other => Err(Error::InvalidInput(format!(
                "browser.{other} is not a callable resource"
            ))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "browser.rendered" => {
                let (app, request_key, render) = decode_recorded_render(record)?;
                state_mut::<BrowserState>(state, "browser")?
                    .renders
                    .entry(app)
                    .or_default()
                    .insert(request_key, render);
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<BrowserState>(state, "browser")?
                    .renders
                    .remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "browser.rendered" => {
                let e: Rendered = decode_event(record).ok()?;
                Some(format!(
                    "browser.rendered {} {} {} → {} {} ({} bytes)",
                    e.app,
                    e.output,
                    request::host_and_path_without_query(&e.url),
                    e.status,
                    e.body_kind,
                    e.size
                ))
            }
            _ => None,
        }
    }

    fn recorded_call_per_run_limit(&self, method: &str) -> Option<RecordedCallCap> {
        if method == "render" {
            Some(RecordedCallCap {
                limit: 30,
                escape_hint: "use browser.peek for an unrecorded transient render",
            })
        } else {
            None
        }
    }
}
