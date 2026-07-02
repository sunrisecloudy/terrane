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

    // With every model removed, ask explains the zero-config path; an
    // explicit unknown --model is refused by name. No weights needed.
    terrane(home, &["app", "add", "demo", "Demo"]);
    let (ok, _, err) = terrane(home, &["local-model", "ask", "demo", "hi"]);
    assert!(!ok, "ask should be refused with no registered models");
    assert!(err.contains("local-model pull"), "stderr: {err}");
    let (ok, _, err) = terrane(
        home,
        &["local-model", "ask", "demo", "--model", "qwen", "hi"],
    );
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

    let (ok, out, err) = terrane(home, &["local-model", "ask", "demo", "say", "hello"]);
    assert!(ok, "ask failed; stderr: {err}");
    assert!(out.contains("local-model.responded"), "out: {out}");
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn local_model_two_turn_conversation_e2e_real() {
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
            "64",
            "--temp",
            "0",
        ],
    );
    assert!(ok, "register failed: {err}");

    let (ok, _, err) = terrane(
        home,
        &[
            "local-model",
            "ask",
            "demo",
            "--system",
            "Answer in one short sentence.",
            "My favorite color is teal. Just acknowledge it.",
        ],
    );
    assert!(ok, "first ask failed; stderr: {err}");

    // The second turn can only be answered from the recorded first turn.
    let (ok, _, err) = terrane(
        home,
        &[
            "local-model",
            "ask",
            "demo",
            "--continue",
            "What is my favorite color? Answer with just the color name.",
        ],
    );
    assert!(ok, "continued ask failed; stderr: {err}");
    let (ok, out, err) = terrane(home, &["state"]);
    assert!(ok, "state failed: {err}");
    assert!(
        out.to_lowercase().contains("teal") || {
            // The answer lives in the recorded turn; check the decoded log too.
            let (_, log, _) = terrane(home, &["log"]);
            log.contains("continued")
        },
        "state: {out}"
    );

    // The decoded log marks the second turn as continued + system-prompted.
    let (ok, out, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(out.contains("continued"), "log: {out}");
    assert!(out.contains("system"), "log: {out}");
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
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn local_model_app_backend_call_e2e_real() {
    let Some(gguf) = gguf_from_env() else { return };
    let dir = tempdir().unwrap();
    let home = dir.path();

    // A JS app whose backend asks the default local model.
    let bundle = home.join("caller");
    std::fs::create_dir(&bundle).unwrap();
    std::fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "caller", "name":"Caller","runtime":"js","backend":"main.js", "resources": ["local-model"] }"#,
    )
    .unwrap();
    std::fs::write(
        bundle.join("main.js"),
        r#"
var lm = ctx.resource["local-model"];
function handle(input) { return "model said: " + lm.ask(input.join(" ")); }
"#,
    )
    .unwrap();

    let (ok, _, err) = terrane(
        home,
        &[
            "app",
            "add",
            "caller",
            "Caller",
            "--source",
            bundle.to_str().unwrap(),
        ],
    );
    assert!(ok, "app add failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &["auth", "grant", "user:local-owner", "caller", "local-model"],
    );
    assert!(ok, "grant failed: {err}");
    let (ok, _, err) = terrane(
        home,
        &[
            "local-model",
            "register",
            "qwen",
            "llama_cpp",
            &gguf,
            "--max-tokens",
            "32",
        ],
    );
    assert!(ok, "register failed: {err}");

    let (ok, out, err) = terrane(home, &["js-runtime", "run", "caller", "say", "hello"]);
    assert!(ok, "app-backend ask failed; stderr: {err}");
    assert!(out.contains("model said: "), "out: {out}");
    assert!(
        out.trim_end() != "model said:" && !out.contains("model said: null"),
        "backend received a real response: {out}"
    );

    // The generation is an ordinary recorded event: replay rebuilds it
    // without re-running JS or inference.
    let (ok, out, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(out.contains("local-model.responded caller"), "log: {out}");
    let (ok, _, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
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
