//! Subprocess e2e: drive every `forge/examples` applet through the real `forge`
//! binary (`forge run …`) using a shared temp workspace per case.

use std::path::{Path, PathBuf};
use std::process::Command;

struct ExampleCase {
    id: &'static str,
    input: serde_json::Value,
    collections: &'static [&'static str],
}

fn forge_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_forge"))
}

fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples")
}

fn read(path: impl AsRef<Path>) -> String {
    std::fs::read_to_string(path.as_ref())
        .unwrap_or_else(|e| panic!("read {}: {e}", path.as_ref().display()))
}

fn write_json(path: &Path, value: &serde_json::Value) {
    std::fs::write(path, serde_json::to_string(value).unwrap()).unwrap();
}

fn run_forge(workspace_db: &Path, command: &str, extra: &[&str], payload: &serde_json::Value) -> serde_json::Value {
    let payload_path = workspace_db.with_extension("payload.json");
    write_json(&payload_path, payload);

    let output = Command::new(forge_bin())
        .args([
            "run",
            command,
            "--workspace",
            workspace_db.to_str().unwrap(),
            "--file",
            payload_path.to_str().unwrap(),
            "--json",
        ])
        .args(extra)
        .output()
        .unwrap_or_else(|e| panic!("spawn forge for {command}: {e}"));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "forge run {command} failed (status={:?})\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );

    serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("forge run {command} stdout is not JSON: {e}\nstdout:\n{stdout}")
    })
}

fn run_case(case: ExampleCase) {
    let dir = std::env::temp_dir().join(format!(
        "forge-bundled-e2e-{}-{}",
        case.id,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let workspace_db = dir.join("workspace.sqlite");

    let open = run_forge(&workspace_db, "workspace.open", &[], &serde_json::json!({}));
    assert_eq!(open["ok"], serde_json::json!(true), "{} workspace.open", case.id);

    let example_dir = examples_dir().join(case.id);
    let manifest: serde_json::Value =
        serde_json::from_str(&read(example_dir.join("manifest.json"))).unwrap();
    let entrypoint = manifest["entrypoint"].as_str().unwrap();
    let main_ts = read(example_dir.join(entrypoint));

    let install = run_forge(
        &workspace_db,
        "applet.install",
        &["--applet", case.id],
        &serde_json::json!({
            "applet_id": case.id,
            "manifest": manifest,
            "sources": { entrypoint: main_ts },
        }),
    );
    assert_eq!(install["ok"], serde_json::json!(true), "{} install", case.id);

    let run = run_forge(
        &workspace_db,
        "runtime.run",
        &["--applet", case.id],
        &serde_json::json!({
            "applet_id": case.id,
            "input": case.input,
            "random_seed": 7,
            "time_start": 500,
        }),
    );
    assert_eq!(run["ok"], serde_json::json!(true), "{} runtime.run", case.id);
    assert_eq!(
        run["payload"]["ok"],
        serde_json::json!(true),
        "{} app result",
        case.id
    );
    assert!(
        run["payload"]["ui_renders"]
            .as_array()
            .is_some_and(|renders| !renders.is_empty()),
        "{} rendered UI",
        case.id
    );

    for collection in case.collections {
        let rows = run_forge(
            &workspace_db,
            "query.execute",
            &[],
            &serde_json::json!({ "collection": collection }),
        );
        assert_eq!(rows["ok"], serde_json::json!(true), "{} query {collection}", case.id);
        assert!(
            rows["payload"]["rows"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "{} wrote {collection} records: {rows}",
            case.id
        );
    }

    let run_id = run["payload"]["run_id"]
        .as_str()
        .unwrap_or_else(|| panic!("{} missing run_id: {run}", case.id));
    let replay = run_forge(
        &workspace_db,
        "runtime.replay",
        &[],
        &serde_json::json!({ "run_id": run_id }),
    );
    assert_eq!(replay["ok"], serde_json::json!(true), "{} replay", case.id);
    assert_eq!(
        replay["payload"]["replays_identically"],
        serde_json::json!(true),
        "{} replay identical",
        case.id
    );
}

#[test]
fn every_bundled_example_runs_through_forge_cli() {
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
        ExampleCase {
            id: "calendar-planner",
            input: serde_json::json!({
                "title": "Design review",
                "date": "2026-06-16",
                "start": "10:30",
                "durationMinutes": 45,
                "notes": "Review local-first calendar fixture"
            }),
            collections: &["calendar_events"],
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
        "every forge/examples applet must have a CLI e2e case"
    );

    for case in cases {
        run_case(case);
    }
}