//! Real harness smoke tests. These are ignored by default because they call
//! external agent CLIs and may spend tokens, but they are not simulated.

use std::process::Command;

use tempfile::tempdir;

#[test]
#[ignore = "real Codex/Claude/opencode calls; run explicitly when validating harnesses"]
fn all_generation_harnesses_run_real_quickjs_file_writer() {
    for harness in ["codex", "claude-code", "opencode"] {
        run_real_harness(harness);
    }
}

fn run_real_harness(harness: &str) {
    assert!(
        command_exists(binary_for_harness(harness)),
        "{harness} binary is not on PATH"
    );
    let home = tempdir().unwrap();
    let run_id = format!("run-{}", harness.replace('-', "_"));
    let expected = format!("ok-{harness}");
    let marker = format!("file:marker-{harness}.txt");

    let add = terrane(
        home.path(),
        &["app", "add", "harness-sandbox", "Harness Sandbox"],
        None,
    );
    assert!(add.status.success(), "app add failed: {}", add.stderr);

    let prompt = format!(
        "Generate JavaScript for Terrane QuickJS. The JS must define function handle(input). In handle, call ctx.resource.kv.set({marker:?}, {expected:?}) and return {expected:?}. Do not write any other keys."
    );
    let run = terrane(
        home.path(),
        &[
            "codex",
            "run-js",
            "--harness",
            harness,
            &run_id,
            "harness-sandbox",
            &prompt,
        ],
        Some(("TERRANE_BUILDER_TIMEOUT_MS", "300000")),
    );
    assert!(
        run.status.success(),
        "{harness} run-js failed\nstdout:\n{}\nstderr:\n{}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stdout.contains("codex.js.completed"),
        "{harness} run did not complete:\n{}",
        run.stdout
    );
    assert!(
        run.stdout.contains("kv.set"),
        "{harness} run did not write through kv:\n{}",
        run.stdout
    );

    let state = terrane(home.path(), &["state"], None);
    assert!(state.status.success(), "state failed: {}", state.stderr);
    assert!(
        state
            .stdout
            .contains(&format!("harness-sandbox/{marker} = {expected}")),
        "{harness} marker missing from state:\n{}",
        state.stdout
    );
}

struct OutputText {
    status: std::process::ExitStatus,
    stdout: String,
    stderr: String,
}

fn terrane(home: &std::path::Path, args: &[&str], extra_env: Option<(&str, &str)>) -> OutputText {
    let mut command = Command::new(env!("CARGO_BIN_EXE_terrane"));
    command.args(args).env("TERRANE_HOME", home);
    if let Some((key, value)) = extra_env {
        command.env(key, value);
    }
    let output = command.output().expect("spawn terrane");
    OutputText {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn command_exists(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn binary_for_harness(harness: &str) -> &str {
    match harness {
        "claude-code" => "claude",
        other => other,
    }
}
