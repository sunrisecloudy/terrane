//! The `push` capability — local push subscription facts and delivery outcomes.

use serde_json::Value;
use terrane_cap_interface::{
    restore_state, snapshot_state, CapManifest, Capability, CommandCtx, CommandSpec, Decision,
    Error, EventPattern, EventRecord, EventSpec, GrantResourceSpec, ReadValue, ResourceReadCtx,
    Result, StateStore,
};

mod commands;
mod doc;
mod events;
mod resources;
mod types;

pub use events::{delivered_event, failed_event, subscribed_event, unsubscribed_event};
pub use resources::list_json;
pub use types::{
    PushDelivery, PushDeliveryStatus, PushState, PushSubscription, DELIVERY_HISTORY_LIMIT,
    MAX_SUBSCRIPTIONS_PER_APP,
};

pub struct PushCapability;

impl Capability for PushCapability {
    fn namespace(&self) -> &'static str {
        "push"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "push.subscribe",
                },
                CommandSpec {
                    name: "push.unsubscribe",
                },
                CommandSpec {
                    name: "push.record-delivery",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "push.subscribed",
                },
                EventSpec {
                    kind: "push.unsubscribed",
                },
                EventSpec {
                    kind: "push.delivered",
                },
                EventSpec {
                    kind: "push.failed",
                },
            ],
            queries: Vec::new(),
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "push",
                &["call", "read", "subscribe"],
                "Show system notifications when this app's data changes.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::push_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        commands::decide(ctx, name, args)
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<PushState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<PushState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }

    fn app_of(&self, record: &EventRecord) -> Option<String> {
        events::app_of(record)
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        resources::read(ctx, name, args)
    }

    fn resource_call_output(
        &self,
        state: &dyn StateStore,
        app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "subscribe" => {
                let record = records
                    .iter()
                    .find(|record| record.kind == "push.subscribed")
                    .ok_or_else(|| Error::Runtime("push.subscribe produced no event".into()))?;
                let sub = decode_subscribed(record)?;
                Ok(ReadValue::OptString(Some(sub.sub_id)))
            }
            "unsubscribe" => Ok(ReadValue::OptString(Some(list_json(state, app)?))),
            other => Err(Error::InvalidInput(format!(
                "unknown push resource call output: {other}"
            ))),
        }
    }
}

pub fn matches_pattern(pattern: &str, kind: &str) -> bool {
    if let Some(ns) = pattern.strip_suffix(".*") {
        kind.strip_prefix(ns).is_some_and(|tail| tail.starts_with('.'))
    } else {
        pattern == kind
    }
}

pub fn render_template(template: &str, record: &EventRecord, describe: Option<&str>) -> Result<(String, String)> {
    let payload = payload_json(record);
    let rendered = render_string(template, record, describe, payload.as_ref())?;
    let (title, body) = rendered
        .split_once('|')
        .map(|(title, body)| (title.trim().to_string(), body.trim().to_string()))
        .unwrap_or_else(|| (rendered.trim().to_string(), String::new()));
    if title.is_empty() {
        return Err(Error::InvalidInput("rendered push title is empty".into()));
    }
    Ok((title, body))
}

fn render_string(
    template: &str,
    record: &EventRecord,
    describe: Option<&str>,
    payload: Option<&Value>,
) -> Result<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let Some(end) = after.find('}') else {
            return Err(Error::InvalidInput("template has unmatched {".into()));
        };
        let field = &after[..end];
        out.push_str(&placeholder(field, record, describe, payload)?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

fn placeholder(
    field: &str,
    record: &EventRecord,
    describe: Option<&str>,
    payload: Option<&Value>,
) -> Result<String> {
    match field {
        "kind" => Ok(record.kind.clone()),
        "describe" => Ok(describe.unwrap_or(&record.kind).to_string()),
        value if value.trim().is_empty() => Err(Error::InvalidInput("empty placeholder".into())),
        value => Ok(payload
            .and_then(|payload| payload.get(value))
            .map(json_value_string)
            .unwrap_or_default()),
    }
}

fn payload_json(record: &EventRecord) -> Option<Value> {
    match record.kind.as_str() {
        "kv.set" => {
            #[derive(borsh::BorshDeserialize)]
            struct KvSet {
                app: String,
                key: String,
                value: String,
            }
            let decoded: KvSet = terrane_cap_interface::decode_event(record).ok()?;
            Some(serde_json::json!({"app": decoded.app, "key": decoded.key, "value": decoded.value}))
        }
        "kv.deleted" => {
            #[derive(borsh::BorshDeserialize)]
            struct KvDeleted {
                app: String,
                key: String,
            }
            let decoded: KvDeleted = terrane_cap_interface::decode_event(record).ok()?;
            Some(serde_json::json!({"app": decoded.app, "key": decoded.key}))
        }
        _ => None,
    }
}

fn json_value_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(_) | Value::Object(_) => value.to_string(),
    }
}

fn decode_subscribed(record: &EventRecord) -> Result<types::Subscribed> {
    terrane_cap_interface::decode_event(record)
}

fn json_escape(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}
