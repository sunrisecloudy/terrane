use std::collections::BTreeMap;

use ed25519_dalek::SigningKey;
use tempfile::tempdir;
use terrane_cap_interface::{Decision, Effect, Request};
use terrane_cap_manager::{CapabilityManager, CapabilityStatus};
use terrane_cap_protocol::{WorkerRequest, WorkerResponse};
use terrane_cap_worker::package::package_all;
use terrane_core::Core;

#[test]
fn signed_bundle_runs_replays_decides_and_snapshots_out_of_process() {
    let root = tempdir().unwrap();
    let packages = root.path().join("packages");
    let home = root.path().join("home");
    let signing_key = SigningKey::from_bytes(&[7; 32]);
    let worker = env!("CARGO_BIN_EXE_terrane-cap-worker");
    package_all(
        worker.as_ref(),
        &packages,
        &signing_key,
        std::env::consts::OS,
        std::env::consts::ARCH,
    )
    .unwrap();

    let log = root.path().join("static.log");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(Request::new(
        "app.add",
        vec!["notes".into(), "Notes".into()],
    ))
    .unwrap();
    let records = core.log_records().unwrap();

    let manager = CapabilityManager::open(&home, &packages, signing_key.verifying_key()).unwrap();
    manager.prepare(&["time".to_string()], &records).unwrap();
    let response = manager
        .call(
            "time",
            &records,
            WorkerRequest::Decide {
                request: Request::new("time.now", vec!["notes".into()]),
                overlay_records: Vec::new(),
                dependencies: BTreeMap::new(),
            },
        )
        .unwrap();
    assert_eq!(
        response,
        WorkerResponse::Decision {
            decision: Decision::Effect(Effect::ObserveTime {
                app: "notes".into()
            })
        }
    );
    let effect_response = manager
        .call_with_connector(
            "time",
            &records,
            WorkerRequest::ExecuteEffect {
                effect: Effect::ObserveTime {
                    app: "notes".into(),
                },
            },
            |_| panic!("time observation must execute inside the signed worker"),
        )
        .unwrap();
    let WorkerResponse::EffectRecords {
        records: effect_records,
    } = effect_response
    else {
        panic!("unexpected effect response: {effect_response:?}");
    };
    assert_eq!(effect_records.len(), 1);
    assert_eq!(effect_records[0].kind, "time.observed");
    assert!(manager
        .status()
        .iter()
        .any(|status| { status.namespace == "time" && status.status == CapabilityStatus::Ready }));

    assert!(manager.terminate_for_diagnostics("time").unwrap());
    let restarted = manager
        .call(
            "time",
            &records,
            WorkerRequest::Decide {
                request: Request::new("time.now", vec!["notes".into()]),
                overlay_records: Vec::new(),
                dependencies: BTreeMap::new(),
            },
        )
        .unwrap();
    assert_eq!(restarted, response);

    assert!(manager.evict("time").unwrap());
    assert!(home.join("capabilities/state/time.json").is_file());
}
