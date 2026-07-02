//! e2e smoke for `local-model`. Logic detail is covered by
//! `rust/crates/terrane-core/tests/cap/local_model.rs`; real inference and
//! downloads are `#[ignore]`d.

use tempfile::tempdir;

use crate::helpers::terrane;

#[test]
fn local_model_register_and_rm_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(
        home,
        &[
            "local-model",
            "register",
            "qwen",
            "llama_cpp",
            "/models/qwen.gguf",
            "--temp",
            "0.7",
        ],
    );
    assert!(ok, "stderr: {err}");
    assert!(out.contains("local-model.registered"), "out: {out}");

    let (ok, out, err) = terrane(home, &["local-model", "rm", "qwen"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("local-model.removed"), "out: {out}");

    // Asking against an unregistered model is refused in decide, before any
    // engine work — no weights are needed for this to be exercised.
    terrane(home, &["app", "add", "demo", "Demo"]);
    let (ok, _, err) = terrane(home, &["local-model", "ask", "demo", "qwen", "hi"]);
    assert!(!ok, "ask should be refused for an unregistered model");
    assert!(err.contains("unknown local model"), "stderr: {err}");
}

#[test]
fn local_model_server_status_and_stop_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(home, &["local-model", "server", "status"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("\"running\":false"), "out: {out}");

    let (ok, out, err) = terrane(home, &["local-model", "server", "stop"]);
    assert!(ok, "stderr: {err}");
    assert!(out.contains("no resident"), "out: {out}");

    // Bad sub-verbs get usage, not a dispatch attempt.
    let (ok, _, err) = terrane(home, &["local-model", "server", "restart"]);
    assert!(!ok);
    assert!(err.contains("usage:"), "stderr: {err}");
}

fn gguf_from_env() -> Option<String> {
    match std::env::var("TERRANE_LOCAL_MODEL_GGUF") {
        Ok(path) if !path.trim().is_empty() => Some(path),
        _ => {
            eprintln!("skipping: set TERRANE_LOCAL_MODEL_GGUF to a local .gguf file");
            None
        }
    }
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn local_model_ask_e2e_real() {
    let Some(gguf) = gguf_from_env() else { return };
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "demo", "Demo"]);
    let (ok, _, err) = terrane(
        home,
        &[
            "local-model",
            "register",
            "qwen",
            "llama_cpp",
            &gguf,
            "--max-tokens",
            "48",
        ],
    );
    assert!(ok, "register failed: {err}");

    let (ok, out, err) = terrane(
        home,
        &["local-model", "ask", "demo", "qwen", "say", "hello"],
    );
    assert!(ok, "ask failed; stderr: {err}");
    assert!(out.contains("local-model.responded"), "out: {out}");
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn local_model_schema_ask_e2e_real() {
    let Some(gguf) = gguf_from_env() else { return };
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "demo", "Demo"]);
    let (ok, _, err) = terrane(
        home,
        &[
            "local-model",
            "register",
            "qwen",
            "llama_cpp",
            &gguf,
            "--max-tokens",
            "128",
            "--temp",
            "0",
        ],
    );
    assert!(ok, "register failed: {err}");

    let schema =
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#;
    let (ok, out, err) = terrane(
        home,
        &[
            "local-model",
            "ask",
            "demo",
            "qwen",
            "--schema",
            schema,
            "What is the capital of France? Answer as JSON.",
        ],
    );
    assert!(ok, "schema ask failed; stderr: {err}");
    assert!(out.contains("local-model.responded"), "out: {out}");

    // The decoded log line marks the turn as constrained.
    let (ok, out, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(out.contains("constrained"), "log: {out}");
}

#[test]
#[ignore = "downloads ~500 MB from Hugging Face; run with `cargo test -- --ignored`"]
fn local_model_pull_e2e_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();

    let (ok, out, err) = terrane(
        home,
        &[
            "local-model",
            "pull",
            "qwen3_5_0_8b",
            "unsloth/Qwen3.5-0.8B-GGUF",
            "Qwen3.5-0.8B-Q4_K_M.gguf",
        ],
    );
    assert!(ok, "pull failed; stderr: {err}");
    assert!(out.contains("local-model.registered"), "out: {out}");
    assert!(
        home.join("models/Qwen3.5-0.8B-Q4_K_M.gguf").is_file(),
        "weights land under $TERRANE_HOME/models"
    );
}
