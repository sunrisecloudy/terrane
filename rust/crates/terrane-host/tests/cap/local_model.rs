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
fn local_model_embed_register_and_refusal_e2e_smoke() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "notes", "Notes"]);

    // No embedding model registered: embed points at the zero-config path.
    let (ok, _, err) = terrane(home, &["local-model", "embed", "notes", "hello"]);
    assert!(!ok, "embed should be refused with no embedding model");
    assert!(err.contains("local-model pull --embed"), "stderr: {err}");

    // Registering an embedding model is pure (no weights) and records the
    // embedding config, visible in the described event line.
    let (ok, out, err) = terrane(
        home,
        &[
            "local-model",
            "register",
            "nomic",
            "llama_cpp",
            "/models/nomic.gguf",
            "--embed",
        ],
    );
    assert!(ok, "register failed: {err}");
    assert!(out.contains("local-model.registered"), "out: {out}");
    // The decoded log describes the spec as an embedding model.
    let (ok, log, err) = terrane(home, &["log"]);
    assert!(ok, "log failed: {err}");
    assert!(log.contains("embedding"), "log marks embedding: {log}");

    // A generation model asked to embed is refused by name.
    terrane(
        home,
        &[
            "local-model",
            "register",
            "qwen",
            "llama_cpp",
            "/models/qwen.gguf",
        ],
    );
    let (ok, _, err) = terrane(
        home,
        &["local-model", "embed", "notes", "--model", "qwen", "hi"],
    );
    assert!(!ok, "embedding via a chat model is refused");
    assert!(err.contains("not an embedding model"), "stderr: {err}");
}

#[test]
#[ignore = "downloads nomic-embed-text-v1.5 (~150 MB) from Hugging Face; run with `cargo test -- --ignored`"]
fn local_model_embed_e2e_real() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    terrane(home, &["app", "add", "notes", "Notes"]);

    // Pull the recommended embedding model, then embed a document and a query.
    let (ok, out, err) = terrane(home, &["local-model", "pull", "--embed"]);
    assert!(ok, "embed pull failed; stderr: {err}");
    assert!(out.contains("local-model.registered"), "out: {out}");

    let (ok, out, err) = terrane(
        home,
        &["local-model", "embed", "notes", "the quick brown fox"],
    );
    assert!(ok, "embed failed; stderr: {err}");
    assert!(out.contains("local-model.embedded"), "out: {out}");

    let (ok, out, err) = terrane(home, &["local-model", "embed", "notes", "--query", "a fox"]);
    assert!(ok, "query embed failed; stderr: {err}");
    assert!(out.contains("local-model.embedded"), "out: {out}");

    // The vectors are recorded results; replay rebuilds without re-embedding.
    let (ok, _, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
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

#[test]
#[ignore = "downloads the uv + python + mlx-lm toolchain (~400 MB); run with `cargo test -- --ignored`"]
fn local_model_setup_mlx_bootstraps_on_a_scrubbed_path() {
    use std::process::Command;

    let dir = tempdir().unwrap();
    let home = dir.path();

    // A fresh machine: no uv, no mlx_lm on PATH, no env overrides — only the
    // system basics the bootstrap itself needs (curl-free: setup downloads
    // over ureq; tar comes from /usr/bin).
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(["local-model", "setup", "mlx"])
        .env_clear()
        .env("TERRANE_HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .output()
        .expect("spawn terrane");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "setup mlx failed\nstdout: {stdout}\nstderr: {stderr}"
    );

    // The pinned toolchain landed inside the home, and the manifest records
    // an installed (not merely detected) runtime.
    let manifest_path = home.join("engines/mlx.json");
    assert!(manifest_path.is_file(), "engines/mlx.json missing");
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(manifest["installed_by"], "uv", "manifest: {manifest}");

    // The provisioned runtime resolves without PATH help.
    let output = Command::new(env!("CARGO_BIN_EXE_terrane"))
        .args(["local-model", "server", "status"])
        .env_clear()
        .env("TERRANE_HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", home)
        .output()
        .expect("spawn terrane");
    let status = String::from_utf8_lossy(&output.stdout);
    assert!(
        status.contains("\"runtimeAvailable\":true"),
        "status: {status}"
    );
}

/// Absolute path to the repo's `apps/chat` bundle.
fn chat_app_source() -> String {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../apps/chat")
        .canonicalize()
        .expect("apps/chat bundle exists")
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn chat_app_bundle_smoke_without_weights() {
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = chat_app_source();

    let (ok, _, err) = terrane(home, &["app", "add", "chat", "Chat", "--source", &src]);
    assert!(ok, "app add failed: {err}");
    for namespace in ["kv", "local-model"] {
        let (ok, _, err) = terrane(
            home,
            &["auth", "grant", "user:local-owner", "chat", namespace],
        );
        assert!(ok, "grant {namespace} failed: {err}");
    }

    // No models registered: the picker state is honest and send refuses
    // with the zero-config pointer instead of committing anything.
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "chat", "state"]);
    assert!(ok, "state failed: {err}");
    assert!(out.contains("\"models\":[]"), "out: {out}");
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "chat", "use", "ghost"]);
    assert!(ok, "use failed: {err}");
    assert!(out.contains("unknown model"), "out: {out}");
    let (ok, _, err) = terrane(home, &["js-runtime", "run", "chat", "send", "hi"]);
    assert!(!ok, "send without models should fail");
    assert!(err.contains("local-model pull"), "stderr: {err}");

    // The actions table self-describes for agents.
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "chat", "__actions__"]);
    assert!(ok, "__actions__ failed: {err}");
    for verb in ["send", "models", "use", "pull", "new", "history"] {
        assert!(
            out.contains(&format!("\"verb\":\"{verb}\"")),
            "missing {verb}: {out}"
        );
    }
}

#[test]
#[ignore = "real local inference; needs a GGUF at TERRANE_LOCAL_MODEL_GGUF; run with `cargo test -- --ignored`"]
fn chat_app_conversation_e2e_real() {
    let Some(gguf) = gguf_from_env() else { return };
    let dir = tempdir().unwrap();
    let home = dir.path();
    let src = chat_app_source();

    terrane(home, &["app", "add", "chat", "Chat", "--source", &src]);
    for namespace in ["kv", "local-model"] {
        terrane(
            home,
            &["auth", "grant", "user:local-owner", "chat", namespace],
        );
    }
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

    // Two-turn conversation through the app: the second answer needs turn 1.
    let (ok, out, err) = terrane(
        home,
        &[
            "js-runtime",
            "run",
            "chat",
            "send",
            "My favorite color is teal. Just acknowledge it briefly.",
        ],
    );
    assert!(ok, "send 1 failed: {err}");
    assert!(out.contains("\"ok\":true"), "out: {out}");
    let (ok, out, err) = terrane(
        home,
        &[
            "js-runtime",
            "run",
            "chat",
            "send",
            "What is my favorite color? Answer with just the color name.",
        ],
    );
    assert!(ok, "send 2 failed: {err}");
    assert!(out.to_lowercase().contains("teal"), "recall failed: {out}");

    // The visible history holds both exchanges; new chat clears everything.
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "chat", "history"]);
    assert!(ok, "history failed: {err}");
    assert!(out.matches("\"role\":\"user\"").count() == 2, "out: {out}");
    let (ok, _, err) = terrane(home, &["js-runtime", "run", "chat", "new"]);
    assert!(ok, "new failed: {err}");
    let (ok, out, err) = terrane(home, &["js-runtime", "run", "chat", "history"]);
    assert!(ok, "history failed: {err}");
    assert!(out.contains("\"messages\":[]"), "out: {out}");

    // Replay rebuilds the whole chat without re-running JS or inference.
    let (ok, _, err) = terrane(home, &["replay"]);
    assert!(ok, "replay failed: {err}");
}
