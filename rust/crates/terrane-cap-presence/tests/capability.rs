use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};
use terrane_cap_presence::{
    ChannelLimits, PresenceCapability, PresenceState, DEFAULT_MAX_PAYLOAD_BYTES,
    DEFAULT_MAX_RATE_PER_SEC, MAX_CHANNELS_PER_APP,
};

#[derive(Default)]
struct TestState {
    presence: PresenceState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "presence" => Some(&self.presence),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "presence" => Some(&mut self.presence),
            _ => None,
        }
    }
}

struct TestBus;

impl CapBus for TestBus {
    fn query(&self, cap: &str, name: &str, args: &[String]) -> terrane_cap_interface::Result<QueryValue> {
        if cap == "app" && name == "exists" {
            return Ok(QueryValue::Bool(args.first().is_some_and(|app| app == "demo")));
        }
        Ok(QueryValue::Bool(false))
    }
}

fn ctx<'a>(state: &'a TestState, bus: &'a TestBus) -> CommandCtx<'a> {
    CommandCtx { state, bus }
}

#[test]
fn define_and_drop_fold_channel_limits() {
    let cap = PresenceCapability;
    let bus = TestBus;
    let mut state = TestState::default();
    let decision = cap
        .decide(
            ctx(&state, &bus),
            "presence.channel.define",
            &["demo".into(), "cursor".into(), "2048".into(), "10".into()],
        )
        .unwrap();
    let records = match decision {
        Decision::Commit(records) => records,
        other => panic!("expected commit, got {other:?}"),
    };
    assert_eq!(records[0].kind, "presence.channel.defined");
    cap.fold(&mut state, &records[0]).unwrap();
    assert_eq!(
        state.presence.channels["demo"]["cursor"],
        ChannelLimits {
            max_payload: 2048,
            max_rate: 10,
        }
    );

    let drop = cap
        .decide(
            ctx(&state, &bus),
            "presence.channel.drop",
            &["demo".into(), "cursor".into()],
        )
        .unwrap();
    let records = match drop {
        Decision::Commit(records) => records,
        other => panic!("expected commit, got {other:?}"),
    };
    cap.fold(&mut state, &records[0]).unwrap();
    assert!(state.presence.channels.is_empty());
}

#[test]
fn publish_is_transient_effect_and_records_no_event() {
    let cap = PresenceCapability;
    let bus = TestBus;
    let state = TestState::default();
    let decision = cap
        .decide(
            ctx(&state, &bus),
            "presence.publish",
            &["demo".into(), "cursor".into(), r#"{"x":1}"#.into()],
        )
        .unwrap();
    assert!(matches!(
        decision,
        Decision::TransientEffect(Effect::PresencePublish { .. })
    ));
}

#[test]
fn publish_validates_channel_payload_and_limits() {
    let cap = PresenceCapability;
    let bus = TestBus;
    let mut state = TestState::default();
    cap.fold(
        &mut state,
        &terrane_cap_presence::channel_defined_event(
            "demo",
            "cursor",
            ChannelLimits {
                max_payload: 8,
                max_rate: DEFAULT_MAX_RATE_PER_SEC,
            },
        )
        .unwrap(),
    )
    .unwrap();
    assert!(matches!(
        cap.decide(
            ctx(&state, &bus),
            "presence.publish",
            &["demo".into(), "cursor".into(), "not json".into()],
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        cap.decide(
            ctx(&state, &bus),
            "presence.publish",
            &["demo".into(), "cursor".into(), r#"{"long":true}"#.into()],
        ),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        cap.decide(
            ctx(&state, &bus),
            "presence.publish",
            &["demo".into(), " bad ".into(), "{}".into()],
        ),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn channel_capacity_is_bounded_but_redefine_is_allowed() {
    let cap = PresenceCapability;
    let bus = TestBus;
    let mut state = TestState::default();
    for i in 0..MAX_CHANNELS_PER_APP {
        cap.fold(
            &mut state,
            &terrane_cap_presence::channel_defined_event(
                "demo",
                format!("c{i}"),
                ChannelLimits::default(),
            )
            .unwrap(),
        )
        .unwrap();
    }
    cap.decide(
        ctx(&state, &bus),
        "presence.channel.define",
        &["demo".into(), "c1".into()],
    )
    .unwrap();
    assert!(matches!(
        cap.decide(
            ctx(&state, &bus),
            "presence.channel.define",
            &["demo".into(), "overflow".into()],
        ),
        Err(Error::InvalidInput(_))
    ));
}

#[test]
fn app_removed_clears_channels() {
    #[derive(borsh::BorshSerialize)]
    struct Removed {
        id: String,
    }

    let cap = PresenceCapability;
    let mut state = TestState::default();
    cap.fold(
        &mut state,
        &terrane_cap_presence::channel_defined_event(
            "demo",
            "cursor",
            ChannelLimits::default(),
        )
        .unwrap(),
    )
    .unwrap();
    let removed = encode_event("app.removed", &Removed { id: "demo".into() }).unwrap();
    cap.fold(&mut state, &removed).unwrap();
    assert!(state.presence.channels.is_empty());
}

#[test]
fn defaults_match_plan() {
    assert_eq!(ChannelLimits::default().max_payload, DEFAULT_MAX_PAYLOAD_BYTES as u32);
    assert_eq!(ChannelLimits::default().max_rate, DEFAULT_MAX_RATE_PER_SEC);
}
