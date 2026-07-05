use terrane_cap_geo::{
    fix_json, observed_event, round_for_precision, GeoCapability, GeoFix, GeoPrecision, GeoState,
};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryCtx, QueryValue,
    ReadValue, StateStore,
};

#[derive(Default)]
struct Store {
    geo: GeoState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn std::any::Any> {
        match namespace {
            "geo" => Some(&self.geo),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn std::any::Any> {
        match namespace {
            "geo" => Some(&mut self.geo),
            _ => None,
        }
    }
}

struct Bus;

impl CapBus for Bus {
    fn query(
        &self,
        capability: &str,
        name: &str,
        args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (capability, name, args.first().map(String::as_str)) {
            ("app", "exists", Some("demo")) => Ok(QueryValue::Bool(true)),
            ("app", "exists", Some(_)) => Ok(QueryValue::Bool(false)),
            _ => Err(Error::InvalidInput("unexpected query".into())),
        }
    }
}

fn ctx<'a>(store: &'a Store, bus: &'a Bus) -> CommandCtx<'a> {
    CommandCtx { state: store, bus }
}

#[test]
fn coarse_rounding_happens_as_integer_e7_math() {
    let exact = round_for_precision(137_749_299, 1_005_016_941, 42, GeoPrecision::Exact);
    assert_eq!(exact, (137_749_299, 1_005_016_941, 42));

    let coarse = round_for_precision(137_749_299, 1_005_016_941, 42, GeoPrecision::Coarse);
    assert_eq!(coarse, (137_700_000, 1_005_000_000, 1_000));

    let negative = round_for_precision(-137_749_299, -1_005_016_941, 2_000, GeoPrecision::Coarse);
    assert_eq!(negative, (-137_700_000, -1_005_000_000, 2_000));
}

#[test]
fn locate_and_peek_return_recorded_and_transient_effects() {
    let cap = GeoCapability;
    let store = Store::default();
    let bus = Bus;

    let decision = cap
        .decide(ctx(&store, &bus), "geo.locate", &["demo".into(), "exact".into()])
        .unwrap();
    assert_eq!(
        decision,
        Decision::Effect(Effect::GeoLocate {
            app: "demo".into(),
            precision: "exact".into()
        })
    );

    let decision = cap
        .decide(ctx(&store, &bus), "geo.peek", &["demo".into(), "coarse".into()])
        .unwrap();
    assert_eq!(
        decision,
        Decision::TransientEffect(Effect::GeoLocate {
            app: "demo".into(),
            precision: "coarse".into()
        })
    );
}

#[test]
fn invalid_precision_and_missing_app_are_typed_errors() {
    let cap = GeoCapability;
    let store = Store::default();
    let bus = Bus;

    assert!(matches!(
        cap.decide(ctx(&store, &bus), "geo.locate", &["demo".into(), "fine".into()]),
        Err(Error::InvalidInput(_))
    ));
    assert_eq!(
        cap.decide(ctx(&store, &bus), "geo.locate", &["ghost".into(), "coarse".into()]),
        Err(Error::AppNotFound("ghost".into()))
    );
}

#[test]
fn fold_keeps_last_twenty_and_last_resource_returns_json() {
    let cap = GeoCapability;
    let mut store = Store::default();
    for i in 0..25 {
        cap.fold(
            &mut store,
            &observed_event("demo", 100 + i, 200 + i, 5, "exact", i as u64 * 10_000)
                .unwrap(),
        )
        .unwrap();
    }
    let fixes = store.geo.fixes.get("demo").unwrap();
    assert_eq!(fixes.len(), 20);
    assert_eq!(fixes.front().unwrap().observed_at, 50_000);
    assert_eq!(fixes.back().unwrap().lat_e7, 124);

    let last = cap
        .read_resource(
            terrane_cap_interface::ResourceReadCtx {
                state: &store,
                bus: &Bus,
                app: "demo",
                host: None,
            },
            "last",
            &[],
        )
        .unwrap();
    assert_eq!(
        last,
        ReadValue::OptString(Some(fix_json(&GeoFix {
            lat_e7: 124,
            lon_e7: 224,
            accuracy_m: 5,
            precision: "exact".into(),
            observed_at: 240_000,
        })))
    );
}

#[test]
fn fold_rejects_recorded_fixes_inside_rate_window() {
    let cap = GeoCapability;
    let mut store = Store::default();
    cap.fold(
        &mut store,
        &observed_event("demo", 100, 200, 5, "exact", 10_000).unwrap(),
    )
    .unwrap();
    let err = cap
        .fold(
            &mut store,
            &observed_event("demo", 101, 201, 5, "exact", 19_999).unwrap(),
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidInput(msg) if msg.contains("rate limit")));
}

#[test]
fn app_removed_drops_fixes_and_supports_defaults_false() {
    let cap = GeoCapability;
    let mut store = Store::default();
    cap.fold(
        &mut store,
        &observed_event("demo", 100, 200, 5, "exact", 10_000).unwrap(),
    )
    .unwrap();
    #[derive(borsh::BorshSerialize)]
    struct Removed {
        id: String,
    }
    let removed = encode_event("app.removed", &Removed { id: "demo".into() }).unwrap();
    cap.fold(&mut store, &removed).unwrap();
    assert!(store.geo.fixes.is_empty());

    assert_eq!(
        cap.query(
            QueryCtx {
                state: &store,
                bus: &Bus,
            },
            "supports",
            &[],
        )
        .unwrap(),
        QueryValue::Bool(false)
    );
}

#[test]
fn describe_redacts_coordinates() {
    let cap = GeoCapability;
    let record = observed_event("demo", 137_700_000, 1_005_000_000, 1_000, "coarse", 42).unwrap();
    let description = cap.describe(&record).unwrap();
    assert!(description.contains("precision=coarse"));
    assert!(description.contains("accuracy_m=1000"));
    assert!(!description.contains("137700000"));
    assert!(!description.contains("1005000000"));

    let output = cap
        .resource_call_output(&Store::default(), "demo", "current", &[record])
        .unwrap();
    assert!(matches!(output, ReadValue::OptString(Some(json)) if json.contains("\"lat_e7\"")));
}
