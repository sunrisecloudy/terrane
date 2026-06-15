use forge_cli::{handle, install};
use forge_core::WorkspaceCore;
use forge_runtime::{InMemoryFileSystem, MockHttpClient};
use std::path::{Path, PathBuf};

struct ExampleCase {
    id: &'static str,
    input: serde_json::Value,
    collections: &'static [&'static str],
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

#[test]
fn every_forge_example_installs_runs_and_replays() {
    let cases = [
        ExampleCase {
            id: "notes-lite",
            input: serde_json::json!({ "title": "Replacement note" }),
            collections: &["notes"],
        },
        ExampleCase {
            id: "task-workbench",
            input: serde_json::json!({ "title": "Cut over examples", "status": "doing" }),
            collections: &["tasks"],
        },
        ExampleCase {
            id: "file-transformer",
            input: serde_json::json!({
                "name": "release-summary",
                "outputPath": "out/release-summary.txt",
                "bytesBase64": "Rm9yZ2UgZXhhbXBsZSBvdXRwdXQK"
            }),
            collections: &["transforms"],
        },
        ExampleCase {
            id: "api-dashboard",
            input: serde_json::json!({ "path": "weather" }),
            collections: &["requests"],
        },
        ExampleCase {
            id: "core-replay-lab",
            input: serde_json::json!({ "event": "example.run" }),
            collections: &["replay_events"],
        },
    ];

    let disk_examples: Vec<String> = std::fs::read_dir(examples_dir())
        .unwrap()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().join("manifest.json").exists())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(
        disk_examples.len(),
        cases.len(),
        "every forge/examples applet must have a run case"
    );

    for case in cases {
        run_case(case);
    }
}

fn run_case(case: ExampleCase) {
    let dir = examples_dir().join(case.id);
    let manifest_json = read(dir.join("manifest.json"));
    let main_ts = read(dir.join("src/main.ts"));

    let mut core = WorkspaceCore::in_memory(format!("ws-{}", case.id)).unwrap();
    core.set_http_client_factory(|| Box::new(MockHttpClient::canned()));
    core.set_file_system_factory(|| {
        Box::new(InMemoryFileSystem::new().with_handle_root("workspace_data", "workspace-root"))
    });

    install(&mut core, case.id, &manifest_json, &main_ts)
        .unwrap_or_else(|e| panic!("{} install failed: {e}", case.id));

    let run = handle(
        &mut core,
        Some(case.id),
        "runtime.run",
        serde_json::json!({
            "input": case.input,
            "random_seed": 7,
            "time_start": 500
        }),
    )
    .unwrap_or_else(|e| panic!("{} runtime.run failed: {e}", case.id));

    assert_eq!(run["ok"], serde_json::json!(true), "{} run ok", case.id);
    assert_eq!(
        run["result"]["ok"],
        serde_json::json!(true),
        "{} app result ok",
        case.id
    );
    assert!(
        run["ui_renders"]
            .as_array()
            .is_some_and(|renders| !renders.is_empty()),
        "{} rendered a UI tree",
        case.id
    );

    for collection in case.collections {
        let rows = handle(
            &mut core,
            None,
            "query.execute",
            serde_json::json!({ "collection": collection }),
        )
        .unwrap_or_else(|e| panic!("{} query {collection} failed: {e}", case.id));
        assert!(
            rows["rows"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{} wrote at least one {collection} record: {rows}",
            case.id
        );
    }

    let run_id = run["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{} returned no run_id: {run}", case.id));
    let replay = handle(
        &mut core,
        None,
        "runtime.replay",
        serde_json::json!({ "run_id": run_id }),
    )
    .unwrap_or_else(|e| panic!("{} runtime.replay failed: {e}", case.id));
    assert_eq!(
        replay["replays_identically"],
        serde_json::json!(true),
        "{} replay is byte-identical",
        case.id
    );
}
