//! The `presence` capability — transient realtime signals plus durable channel
//! definitions.
//!
//! Presence payloads are deliberately never recorded. Durable state contains
//! only channel definitions and limits; publishing is a `TransientEffect` run at
//! the host edge.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, restore_state,
    snapshot_state, state_mut, state_ref, AppId, CapManifest, Capability, CommandCtx, CommandSpec,
    Decision, Effect, Error, EventPattern, EventRecord, EventSpec, GrantResourceSpec, ReadValue,
    ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod doc;

pub const DEFAULT_MAX_PAYLOAD_BYTES: usize = 16 * 1024;
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;
pub const DEFAULT_MAX_RATE_PER_SEC: u32 = 20;
pub const MAX_CHANNELS_PER_APP: usize = 64;
pub const MAX_CHANNEL_NAME_CHARS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ChannelLimits {
    pub max_payload: u32,
    pub max_rate: u32,
}

impl Default for ChannelLimits {
    fn default() -> Self {
        ChannelLimits {
            max_payload: DEFAULT_MAX_PAYLOAD_BYTES as u32,
            max_rate: DEFAULT_MAX_RATE_PER_SEC,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PresenceState {
    pub channels: BTreeMap<AppId, BTreeMap<String, ChannelLimits>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct ChannelDefined {
    app: String,
    channel: String,
    max_payload: u32,
    max_rate: u32,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct ChannelDropped {
    app: String,
    channel: String,
}

pub fn channel_defined_event(
    app: impl Into<String>,
    channel: impl Into<String>,
    limits: ChannelLimits,
) -> Result<EventRecord> {
    encode_event(
        "presence.channel.defined",
        &ChannelDefined {
            app: app.into(),
            channel: channel.into(),
            max_payload: limits.max_payload,
            max_rate: limits.max_rate,
        },
    )
}

pub fn channel_dropped_event(
    app: impl Into<String>,
    channel: impl Into<String>,
) -> Result<EventRecord> {
    encode_event(
        "presence.channel.dropped",
        &ChannelDropped {
            app: app.into(),
            channel: channel.into(),
        },
    )
}

pub struct PresenceCapability;

impl Capability for PresenceCapability {
    fn namespace(&self) -> &'static str {
        "presence"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "presence.channel.define",
                },
                CommandSpec {
                    name: "presence.channel.drop",
                },
                CommandSpec {
                    name: "presence.publish",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "presence.channel.defined",
                },
                EventSpec {
                    kind: "presence.channel.dropped",
                },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "publish",
                    params: &["channel", "json"],
                },
                ResourceMethod::Read {
                    name: "peers",
                    params: &["channel"],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "presence",
                &["call", "read", "publish", "subscribe"],
                "Ephemeral realtime signals to replicas sharing this app.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::presence_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "presence.channel.define" => decide_define(ctx, args),
            "presence.channel.drop" => decide_drop(ctx, args),
            "presence.publish" => decide_publish(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "presence.channel.defined" => {
                let event: ChannelDefined = decode_event(record)?;
                state_mut::<PresenceState>(state, "presence")?
                    .channels
                    .entry(event.app)
                    .or_default()
                    .insert(
                        event.channel,
                        ChannelLimits {
                            max_payload: event.max_payload,
                            max_rate: event.max_rate,
                        },
                    );
            }
            "presence.channel.dropped" => {
                let event: ChannelDropped = decode_event(record)?;
                let state = state_mut::<PresenceState>(state, "presence")?;
                if let Some(channels) = state.channels.get_mut(&event.app) {
                    channels.remove(&event.channel);
                    if channels.is_empty() {
                        state.channels.remove(&event.app);
                    }
                }
            }
            "app.removed" => {
                let event = decode_app_removed(record)?;
                state_mut::<PresenceState>(state, "presence")?
                    .channels
                    .remove(&event.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        snapshot_state::<PresenceState>(state, self.namespace())
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        restore_state::<PresenceState>(state, self.namespace(), payload)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "presence.channel.defined" => {
                let event: ChannelDefined = decode_event(record).ok()?;
                Some(format!(
                    "presence.channel.defined {}/{} payload<={} rate<={}/s",
                    event.app, event.channel, event.max_payload, event.max_rate
                ))
            }
            "presence.channel.dropped" => {
                let event: ChannelDropped = decode_event(record).ok()?;
                Some(format!("presence.channel.dropped {}/{}", event.app, event.channel))
            }
            _ => None,
        }
    }

    fn app_of(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "presence.channel.defined" => decode_event::<ChannelDefined>(record).ok().map(|e| e.app),
            "presence.channel.dropped" => decode_event::<ChannelDropped>(record).ok().map(|e| e.app),
            _ => None,
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "peers" => {
                let channel = arg(args, 0, "channel")?;
                validate_channel_name(&channel)?;
                let Some(host) = ctx.host else {
                    return Ok(ReadValue::StringList(Vec::new()));
                };
                let json = host.sample(
                    "presence.peers",
                    &[ctx.app.to_string(), channel],
                )?;
                let peers: Vec<String> = serde_json::from_str(&json)
                    .map_err(|e| Error::Runtime(format!("presence peers decode failed: {e}")))?;
                Ok(ReadValue::StringList(peers))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: presence.{other}"
            ))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        _records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "publish" => Ok(ReadValue::OptString(Some("ok".to_string()))),
            other => Err(Error::InvalidInput(format!(
                "presence.{other} is not a callable resource"
            ))),
        }
    }
}

fn decide_define(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let channel = arg(args, 1, "channel")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_channel_name(&channel)?;
    let max_payload = optional_limit(args, 2, DEFAULT_MAX_PAYLOAD_BYTES, MAX_PAYLOAD_BYTES, "max_payload")?;
    let max_rate = optional_rate(args, 3)?;
    ensure_channel_capacity(ctx.state, &app, &channel)?;
    Ok(Decision::Commit(vec![channel_defined_event(
        app,
        channel,
        ChannelLimits {
            max_payload: max_payload as u32,
            max_rate,
        },
    )?]))
}

fn decide_drop(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let channel = arg(args, 1, "channel")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_channel_name(&channel)?;
    Ok(Decision::Commit(vec![channel_dropped_event(app, channel)?]))
}

fn decide_publish(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let channel = arg(args, 1, "channel")?;
    let payload = arg(args, 2, "payload")?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_channel_name(&channel)?;
    validate_payload_json(&payload)?;
    let limits = channel_limits(ctx.state, &app, &channel)?;
    if payload.len() > limits.max_payload as usize {
        return Err(Error::InvalidInput(format!(
            "presence payload for {app}/{channel} exceeds {} bytes",
            limits.max_payload
        )));
    }
    Ok(Decision::TransientEffect(Effect::PresencePublish {
        app,
        channel,
        payload,
    }))
}

fn channel_limits(state: &dyn StateStore, app: &str, channel: &str) -> Result<ChannelLimits> {
    Ok(state_ref::<PresenceState>(state, "presence")?
        .channels
        .get(app)
        .and_then(|channels| channels.get(channel))
        .cloned()
        .unwrap_or_default())
}

fn ensure_channel_capacity(state: &dyn StateStore, app: &str, channel: &str) -> Result<()> {
    let state = state_ref::<PresenceState>(state, "presence")?;
    let count = state.channels.get(app).map(BTreeMap::len).unwrap_or(0);
    let exists = state
        .channels
        .get(app)
        .is_some_and(|channels| channels.contains_key(channel));
    if !exists && count >= MAX_CHANNELS_PER_APP {
        return Err(Error::InvalidInput(format!(
            "presence app {app} already has the maximum {MAX_CHANNELS_PER_APP} channels"
        )));
    }
    Ok(())
}

fn validate_channel_name(channel: &str) -> Result<()> {
    let trimmed = channel.trim();
    if trimmed.is_empty() || trimmed != channel {
        return Err(Error::InvalidInput(
            "presence channel must be non-empty with no surrounding whitespace".into(),
        ));
    }
    if channel.chars().count() > MAX_CHANNEL_NAME_CHARS {
        return Err(Error::InvalidInput(format!(
            "presence channel must be at most {MAX_CHANNEL_NAME_CHARS} characters"
        )));
    }
    if channel.chars().any(char::is_control) {
        return Err(Error::InvalidInput(
            "presence channel must not contain control characters".into(),
        ));
    }
    Ok(())
}

fn validate_payload_json(payload: &str) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(payload)
        .map(|_| ())
        .map_err(|e| Error::InvalidInput(format!("presence payload must be JSON: {e}")))
}

fn optional_limit(
    args: &[String],
    index: usize,
    default: usize,
    max: usize,
    label: &str,
) -> Result<usize> {
    let Some(raw) = args.get(index) else {
        return Ok(default);
    };
    let value = raw
        .parse::<usize>()
        .map_err(|_| Error::InvalidInput(format!("{label} must be an integer byte limit")))?;
    if value == 0 || value > max {
        return Err(Error::InvalidInput(format!(
            "{label} must be between 1 and {max}"
        )));
    }
    Ok(value)
}

fn optional_rate(args: &[String], index: usize) -> Result<u32> {
    let Some(raw) = args.get(index) else {
        return Ok(DEFAULT_MAX_RATE_PER_SEC);
    };
    let value = raw
        .parse::<u32>()
        .map_err(|_| Error::InvalidInput("max_rate must be an integer messages/sec limit".into()))?;
    if value == 0 || value > DEFAULT_MAX_RATE_PER_SEC {
        return Err(Error::InvalidInput(format!(
            "max_rate must be between 1 and {DEFAULT_MAX_RATE_PER_SEC}"
        )));
    }
    Ok(value)
}
