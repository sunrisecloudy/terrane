//! e2e smoke for `model` — a real agent call through the binary, so `#[ignore]`d.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use tempfile::tempdir;

use crate::helpers::{on_path, terrane};

#[test]
fn model_e2e_fake_agent_records_and_replays() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    let codex = bin.join("codex");
    fs::write(
        &codex,
        "#!/bin/sh\nif [ \"$1\" = \"exec\" ]; then printf 'fake-agent:%s\\n' \"$2\"; else exit 2; fi\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&codex).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&codex, perms).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );

    terrane(home, &["app", "add", "asst", "Assistant"]);
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args([
            "model",
            "ask",
            "asst",
            "codex",
            r#"{"parts":[{"text":"hello from fake"}]}"#,
        ])
        .env("TERRANE_HOME", home)
        .env("PATH", &path)
        .output()
        .expect("spawn terrane");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("model.responded"), "out: {out}");

    let (ok, _, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
}

#[test]
#[ignore = "real agent call (needs claude on PATH + auth; costs tokens); run with `--ignored`"]
fn model_e2e_smoke_real() {
    if !on_path("claude") {
        eprintln!("skipping model e2e: `claude` not on PATH");
        return;
    }
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "asst", "Assistant"]);

    let (ok, out, err) = terrane(
        home,
        &[
            "model",
            "ask",
            "asst",
            "claude",
            "Reply with exactly the two characters: OK",
        ],
    );
    assert!(ok, "agent call failed; stderr: {err}");
    assert!(out.contains("model.responded"), "out: {out}");
}
