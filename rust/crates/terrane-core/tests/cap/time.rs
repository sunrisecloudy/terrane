//! Engine tests for the `time` capability — the recorded-effect mechanism, the
//! transient (unrecorded) `time.live` resource, the per-run recorded-call cap,
//! and replay identity.

use std::cell::Cell;
use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_cap_time::{observed_event, TimeState};
use terrane_core::{
    fold_records_in_memory, Core, Effect, EffectRunner, EventRecord, State, LOCAL_OWNER_SUBJECT,
};

use crate::helpers::req;

/// A runner that answers every `ObserveTime` with a strictly increasing
/// epoch-ms (so two reads in one run produce two distinct recorded facts), and
/// refuses anything else. Deterministic — no real clock.
struct ClockedObserve {
    next: Cell<u64>,
}

impl ClockedObserve {
    fn new(start: u64) -> Self {
        Self {
            next: Cell::new(start),
        }
    }
}

impl EffectRunner for ClockedObserve {
    fn run(&self, effect: &Effect, _state: &State) -> terrane_core::Result<Vec<EventRecord>> {
        match effect {
            Effect::ObserveTime { app } => {
                let ms = self.next.get();
                self.next.set(ms + 1);
                Ok(vec![observed_event(app, ms)?])
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

fn add_time_app(core: &mut Core<ClockedObserve>, dir: &Path, app: &str, backend: &str) -> String {
    let src = write_bundle(
        dir,
        app,
        &format!(
            r#"{{"id":"{app}","name":"{app}","runtime":"js","backend":"main.js","resources":["time","kv"]}}"#
        ),
        backend,
    );
    core.dispatch(req("app.add", &[app, app, "--source", &src]))
        .unwrap();
    core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, app, "time"]))
        .unwrap();
    src
}

#[test]
fn time_now_resource_records_each_observation_and_replays() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            var a = ctx.resource.time.now();
            var b = ctx.resource.time.now();
            return a + ";" + b;
        }
    "#;
    let mut core = Core::open_with(dir.path().join("log.bin"), ClockedObserve::new(1_700_000_000_000)).unwrap();
    add_time_app(&mut core, dir.path(), "watch", backend);

    let records = core
        .dispatch(req("js-runtime.run", &["watch", "go"]))
        .unwrap();

    // Two distinct recorded observations committed by the run.
    let observed: Vec<&EventRecord> = records.iter().filter(|r| r.kind == "time.observed").collect();
    assert_eq!(observed.len(), 2, "records: {records:?}");
    assert!(core.take_last_output().unwrap().contains("1700000000000"));
    // The last folded value is the second observation.
    assert_eq!(core.state().time.last["watch"], 1_700_000_000_001);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn time_live_resource_returns_value_but_records_nothing() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            return ctx.resource.time.live();
        }
    "#;
    let mut core = Core::open_with(dir.path().join("log.bin"), ClockedObserve::new(1_700_000_000_000)).unwrap();
    add_time_app(&mut core, dir.path(), "live", backend);

    let records = core.dispatch(req("js-runtime.run", &["live", "go"])).unwrap();

    // The live value reaches the backend…
    assert!(core
        .take_last_output()
        .unwrap()
        .starts_with("170000000000"));
    // …but the transient read records NOTHING: no event committed, no time.observed
    // folded into state.
    assert!(records.is_empty(), "time.live must record nothing, got: {records:?}");
    assert!(
        core.state().time.last.is_empty(),
        "time.live must not fold into state"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn time_now_per_run_cap_allows_32_then_a_fresh_run_allows_32_again() {
    let dir = tempdir().unwrap();
    let backend = r#"
        function handle(input) {
            var last = null;
            for (var i = 0; i < 32; i++) { last = ctx.resource.time.now(); }
            return "ok " + last;
        }
    "#;
    let mut core = Core::open_with(
        dir.path().join("log.bin"),
        ClockedObserve::new(1_700_000_000_000),
    )
    .unwrap();
    add_time_app(&mut core, dir.path(), "cap32", backend);

    // First run: exactly 32 recorded observations; run succeeds.
    let r1 = core.dispatch(req("js-runtime.run", &["cap32", "go"])).unwrap();
    assert_eq!(
        r1.iter().filter(|r| r.kind == "time.observed").count(),
        32,
        "first run: {r1:?}"
    );

    // Second run: the per-run counter is fresh, so another 32 succeed — proving
    // the cap resets between runs (not a cumulative budget over the log).
    let r2 = core.dispatch(req("js-runtime.run", &["cap32", "go"])).unwrap();
    assert_eq!(
        r2.iter().filter(|r| r.kind == "time.observed").count(),
        32,
        "second run: {r2:?}"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn time_now_per_run_cap_blocks_the_33rd_call() {
    let dir = tempdir().unwrap();
    // Loop up to 40, but bail the instant the cap returns null — so the test is
    // fast and the run aborts on the recorded-call overrun.
    let backend = r#"
        function handle(input) {
            for (var i = 0; i < 40; i++) {
                var r = ctx.resource.time.now();
                if (r == null) { return "blocked@" + i; }
            }
            return "ran";
        }
    "#;
    let mut core = Core::open_with(
        dir.path().join("log.bin"),
        ClockedObserve::new(1_700_000_000_000),
    )
    .unwrap();
    add_time_app(&mut core, dir.path(), "loop", backend);

    let err = core
        .dispatch(req("js-runtime.run", &["loop", "go"]))
        .unwrap_err();
    match err {
        terrane_core::Error::InvalidInput(msg) => {
            assert!(msg.contains("time.now"), "msg: {msg}");
            assert!(msg.contains("time.live"), "names escape hatch: {msg}");
        }
        other => panic!("expected InvalidInput naming time.live, got {other:?}"),
    }

    // Option A: a failed run commits nothing, so the log holds no time.observed
    // and folded time state stays empty — the log can't be bloated by the loop.
    assert!(
        core.state().time.last.is_empty(),
        "no observation should fold from the aborted run"
    );
    assert!(core.replay_matches().unwrap());
}

#[test]
fn time_now_requires_existing_app_before_any_effect() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    // A pure core (NoEffects): a valid time.now reaches the runner and is refused…
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    assert!(matches!(
        core.dispatch(req("time.now", &["notes"])),
        Err(terrane_core::Error::InvalidInput(_))
    ));
    // …but a time.now for a missing app is rejected in decide, before any effect.
    assert_eq!(
        core.dispatch(req("time.now", &["ghost"])),
        Err(terrane_core::Error::AppNotFound("ghost".into()))
    );
}

#[test]
fn time_observed_folds_last_value_without_a_clock() {
    let mut state = State::default();
    let records = vec![
        observed_event("demo", 1_700_000_000_000).unwrap(),
        observed_event("demo", 1_700_000_000_042).unwrap(),
    ];
    fold_records_in_memory(&mut state, &records).unwrap();
    assert_eq!(state.time.last["demo"], 1_700_000_000_042);

    // app.removed drops the entry: the broadcast fold reacts without a time command.
    let removed = make_app_removed("demo");
    fold_records_in_memory(&mut state, std::slice::from_ref(&removed)).unwrap();
    assert!(state.time.last.is_empty());

    let _n: &TimeState = &state.time; // ensure the slice type compiles against Core::State
}

/// Build an `app.removed` event matching the shared `AppRemoved` payload shape
/// without coupling this test to a foreign crate's serialization.
fn make_app_removed(id: &str) -> EventRecord {
    #[derive(borsh::BorshSerialize)]
    struct AppRemoved {
        id: String,
    }
    terrane_cap_interface::encode_event("app.removed", &AppRemoved { id: id.to_string() }).unwrap()
}