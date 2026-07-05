use std::any::Any;

use borsh::BorshSerialize;
use terrane_cap_applescript::{
    checked_event, ran_event, AppleScriptCapability, AppleScriptState, RunRecord,
    MAX_RUNS_PER_APP,
};
use terrane_cap_interface::{
    encode_event, CapBus, Capability, CommandCtx, Decision, Effect, Error, QueryValue, StateStore,
};

#[derive(Default)]
struct Store {
    applescript: AppleScriptState,
}

impl StateStore for Store {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "applescript" => Some(&self.applescript),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "applescript" => Some(&mut self.applescript),
            _ => None,
        }
    }
}

struct AppBus {
    exists: bool,
}

impl CapBus for AppBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(self.exists)),
            _ => Err(Error::InvalidInput(format!("unknown query: {cap}.{name}"))),
        }
    }
}

#[derive(BorshSerialize)]
struct Removed {
    id: String,
}

#[test]
fn applescript_run_returns_effect_and_folds_recorded_run() {
    let cap = AppleScriptCapability;
    let bus = AppBus { exists: true };
    let mut store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "applescript.run",
            &["demo".into(), "return 2 + 2".into()],
        )
        .unwrap(),
        Decision::Effect(Effect::AppleScriptRun {
            app: "demo".into(),
            script: "return 2 + 2".into()
        })
    );

    cap.fold(
        &mut store,
        &ran_event("demo", "return 2 + 2", true, "4", "", 0, 12).unwrap(),
    )
    .unwrap();
    assert_eq!(
        store.applescript.runs["demo"][0],
        RunRecord {
            script: "return 2 + 2".into(),
            ok: true,
            output: "4".into(),
            error: String::new(),
            exit_code: 0,
            duration_ms: 12,
        }
    );
}

#[test]
fn applescript_check_returns_effect() {
    let cap = AppleScriptCapability;
    let bus = AppBus { exists: true };
    let store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &bus,
            },
            "applescript.check",
            &["demo".into(), "return 1".into()],
        )
        .unwrap(),
        Decision::Effect(Effect::AppleScriptCheck {
            app: "demo".into(),
            script: "return 1".into()
        })
    );
}

#[test]
fn applescript_rejects_missing_apps_empty_and_oversize_scripts() {
    let cap = AppleScriptCapability;
    let store = Store::default();

    assert_eq!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: false },
            },
            "applescript.run",
            &["demo".into(), "return 1".into()],
        )
        .unwrap_err(),
        Error::AppNotFound("demo".into())
    );

    assert!(matches!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: true },
            },
            "applescript.run",
            &["demo".into(), "   ".into()],
        )
        .unwrap_err(),
        Error::InvalidInput(_)
    ));

    let huge = "x".repeat(65 * 1024);
    assert!(matches!(
        cap.decide(
            CommandCtx {
                state: &store,
                bus: &AppBus { exists: true },
            },
            "applescript.run",
            &["demo".into(), huge],
        )
        .unwrap_err(),
        Error::InvalidInput(_)
    ));
}

#[test]
fn applescript_truncates_runs_deterministically_and_cleans_removed_apps() {
    let cap = AppleScriptCapability;
    let mut store = Store::default();

    for i in 0..=MAX_RUNS_PER_APP {
        cap.fold(
            &mut store,
            &ran_event("demo", &format!("script {i}"), true, "", "", 0, 1).unwrap(),
        )
        .unwrap();
    }
    let runs = &store.applescript.runs["demo"];
    assert_eq!(runs.len(), MAX_RUNS_PER_APP);
    assert_eq!(runs.first().unwrap().script, "script 1");

    cap.fold(
        &mut store,
        &encode_event("app.removed", &Removed { id: "demo".into() }).unwrap(),
    )
    .unwrap();
    assert!(store.applescript.runs.is_empty());
}

#[test]
fn applescript_describe_is_non_empty_for_both_kinds() {
    let cap = AppleScriptCapability;
    assert!(cap
        .describe(&ran_event("demo", "return 1", true, "", "", 0, 1).unwrap())
        .unwrap()
        .contains("applescript.ran"));
    assert!(cap
        .describe(&checked_event("demo", "return 1", true, "").unwrap())
        .unwrap()
        .contains("applescript.checked"));
}

#[test]
fn applescript_doc_marks_machine_control() {
    let doc = AppleScriptCapability.doc(false);
    assert_eq!(doc.namespace, "applescript");
    assert!(doc
        .constraints
        .iter()
        .any(|c| c.contains("machine control")));
}