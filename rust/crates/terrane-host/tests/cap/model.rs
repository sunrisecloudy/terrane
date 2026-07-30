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
fn model_e2e_fake_opencode_forwards_model_and_image_file() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let bin = home.join("bin");
    fs::create_dir(&bin).unwrap();
    let opencode = bin.join("opencode");
    fs::write(
        &opencode,
        "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$TERRANE_TEST_OPENCODE_ARGS\"\nprintf '{\"food_name\":\"fake OpenCode meal\"}\\n'\n",
    )
    .unwrap();
    let mut perms = fs::metadata(&opencode).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&opencode, perms).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let args_file = home.join("opencode-args.txt");
    let image = home.join("food.png");
    fs::write(&image, b"fake-png-image-bytes").unwrap();

    terrane(home, &["app", "add", "health", "Health"]);
    let (ok, _, err) = terrane(
        home,
        &[
            "blob",
            "put",
            "health",
            "imports/food.png",
            "image/png",
            image.to_str().unwrap(),
        ],
    );
    assert!(ok, "blob put failed: {err}");

    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args([
            "model",
            "ask",
            "health",
            "opencode:opencode-go/kimi-k2.6",
            r#"{"parts":[{"text":"analyze"},{"blob":"imports/food.png"}]}"#,
        ])
        .env("TERRANE_HOME", home)
        .env("TERRANE_TEST_OPENCODE_ARGS", &args_file)
        .env("PATH", &path)
        .output()
        .expect("spawn terrane");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let args = fs::read_to_string(&args_file).unwrap();
    assert!(args.contains("--pure"), "args: {args}");
    assert!(
        args.contains("--model\nopencode-go/kimi-k2.6"),
        "args: {args}"
    );
    assert!(args.contains("--file\n"), "args: {args}");
    assert!(args.contains("image-0.png"), "args: {args}");
    assert!(args.contains("\n--\n"), "args: {args}");

    let (ok, state, err) = terrane(home, &["state"]);
    assert!(ok, "state failed: {err}");
    assert!(
        state.contains("health [opencode:opencode-go/kimi-k2.6] exit 0"),
        "state: {state}"
    );
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
