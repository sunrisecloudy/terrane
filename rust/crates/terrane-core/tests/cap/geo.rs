//! Engine tests for the `geo` capability: recorded location observations,
//! transient peeks, replay identity, folded truncation, and deterministic
//! recorded-rate validation.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_cap_geo::{observed_event, round_for_precision, GeoPrecision, GeoState};
use terrane_core::{
    fold_records_in_memory, Core, Effect, EffectRunner, EventRecord, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

#[derive(Clone)]
struct GeoEdge;

impl EffectRunner for GeoEdge {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::GeoLocate { app, precision } => {
                let parsed = GeoPrecision::parse(precision)?;
                let (lat_e7, lon_e7, accuracy_m) =
                    round_for_precision(137_749_299, 1_005_016_941, 42, parsed);
                Ok(vec![observed_event(
                    app,
                    lat_e7,
                    lon_e7,
                    accuracy_m,
                    parsed.as_str(),
                    1_700_000_000_000,
                )?])
            }
            other => Err(terrane_core::Error::Runtime(format!(
                "unexpected effect: {other:?}"
            ))),
        }
    }
}

fn write_bundle(dir: &Path, name: &str, manifest: &str, backend: &str) -> String {
    let bundle = dir.join(name);
    fs::create_dir(&bundle).unwrap();
    fs::write(bundle.join("manifest.json"), manifest).unwrap();
    fs::write(bundle.join("main.js"), backend).unwrap();
    bundle.to_str().unwrap().to_string()
}

fn add_geo_app(core: &mut Core<GeoEdge>, dir: &Path, app: &str, backend: &str) {
    let src = write_bundle(
        dir,
        app,
        &format!(
            r#"{{"id":"{app}","name":"{app}","runtime":"js","backend":"main.js","resources":["geo"]}}"#
        ),
        backend,
    );
    core.dispatch(req("app.add", &[app, app, "--source", &src]))
        .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, app, "geo"]))
        .unwrap();
}

#[test]
fn geo_current_records_rounded_observation_and_replays() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.geo.current("coarse");
        }
    "#;
    let mut core = Core::open_with(dir.path().join("log.bin"), GeoEdge).unwrap();
    add_geo_app(&mut core, dir.path(), "map", backend);

    let records = core.dispatch(req("js-runtime.run", &["map", "go"])).unwrap();

    assert_eq!(
        records.iter().filter(|r| r.kind == "geo.observed").count(),
        1,
        "records: {records:?}"
    );
    let output = core.take_last_output().unwrap();
    assert!(output.contains(r#""precision":"coarse""#), "out: {output}");
    assert!(output.contains(r#""accuracy_m":1000"#), "out: {output}");
    assert_eq!(core.state().geo.fixes["map"].back().unwrap().lat_e7, 137_700_000);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn geo_peek_returns_value_but_records_nothing() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.geo.peek("exact");
        }
    "#;
    let mut core = Core::open_with(dir.path().join("log.bin"), GeoEdge).unwrap();
    add_geo_app(&mut core, dir.path(), "peek", backend);

    let records = core.dispatch(req("js-runtime.run", &["peek", "go"])).unwrap();

    assert!(core.take_last_output().unwrap().contains(r#""precision":"exact""#));
    assert!(records.is_empty(), "geo.peek must record nothing: {records:?}");
    assert!(core.state().geo.fixes.is_empty());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn geo_locate_requires_valid_precision_and_existing_app() {
    let dir = tempdir().unwrap();
    let mut core = Core::open_with(dir.path().join("log.bin"), GeoEdge).unwrap();
    core.dispatch(req("app.add", &["map", "Map"])).unwrap();

    assert!(matches!(
        core.dispatch(req("geo.locate", &["map", "fine"])),
        Err(terrane_core::Error::InvalidInput(_))
    ));
    assert_eq!(
        core.dispatch(req("geo.locate", &["ghost", "coarse"])),
        Err(terrane_core::Error::AppNotFound("ghost".into()))
    );
}

#[test]
fn geo_fold_replay_identity_truncation_rate_limit_and_app_removed() {
    let mut state = State::default();
    let mut records = Vec::new();
    for i in 0..21 {
        records.push(
            observed_event("demo", i, i + 10, 5, "exact", i as u64 * 10_000).unwrap(),
        );
    }
    fold_records_in_memory(&mut state, &records).unwrap();
    assert_eq!(state.geo.fixes["demo"].len(), 20);
    assert_eq!(state.geo.fixes["demo"].front().unwrap().observed_at, 10_000);

    let too_soon = observed_event("demo", 99, 99, 5, "exact", 209_999).unwrap();
    let err = fold_records_in_memory(&mut state, &[too_soon]).unwrap_err();
    assert!(matches!(err, terrane_core::Error::InvalidInput(msg) if msg.contains("rate limit")));

    let removed = make_app_removed("demo");
    fold_records_in_memory(&mut state, std::slice::from_ref(&removed)).unwrap();
    assert!(state.geo.fixes.is_empty());

    let _n: &GeoState = &state.geo;
}

fn make_app_removed(id: &str) -> EventRecord {
    #[derive(borsh::BorshSerialize)]
    struct AppRemoved {
        id: String,
    }
    terrane_cap_interface::encode_event("app.removed", &AppRemoved { id: id.to_string() }).unwrap()
}
