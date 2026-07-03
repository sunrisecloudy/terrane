//! Engine tests for the `local-model` capability — registered specs plus the
//! recorded-inference effect, driven end to end with a stub runner.

use std::fs;
use std::path::Path;

use tempfile::tempdir;
use terrane_cap_local_model::{
    embedded_event, registered_event, responded_event, EmbeddedRecord, LocalModelSpec,
    RespondedRecord,
};
use terrane_core::{fold_records_in_memory, Core, EffectRunner, Error, State, LOCAL_OWNER_SUBJECT};
use terrane_core::{Effect, EventRecord, Result};

use crate::helpers::req;

/// A deterministic stand-in for the llama.cpp edge engine: records a canned
/// response (embedding the history length so conversation plumbing is
/// observable) the way the real runner would.
struct StubLlm;

impl EffectRunner for StubLlm {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::LocalModelCall {
                app,
                model,
                prompt,
                system,
                history,
                schema,
                grammar,
            } => Ok(vec![responded_event(&RespondedRecord {
                app: app.clone(),
                model: model.clone(),
                prompt: prompt.clone(),
                system: system.clone(),
                continued: !history.is_empty(),
                response: format!("stub response (history={})", history.len()),
                ok: true,
                constraint: schema
                    .as_ref()
                    .map(|_| "schema-mask".to_string())
                    .or_else(|| grammar.as_ref().map(|_| "grammar".to_string())),
                token_count: 3,
                duration_ms: 12,
            })?]),
            Effect::LocalModelPull {
                id,
                repo,
                backend,
                file,
                ..
            } => Ok(vec![registered_event(
                id,
                &LocalModelSpec {
                    backend: if backend == "mlx" { "mlx" } else { "llama_cpp" }.to_string(),
                    format: if backend == "mlx" { "mlx" } else { "gguf" }.to_string(),
                    local_path: match file {
                        Some(file) => format!("/stub/{file}"),
                        None => repo.clone(),
                    },
                    context_length: None,
                    chat_template: None,
                    max_tokens: None,
                    temperature_milli: None,
                    source: Some(format!("hf:{repo}")),
                    size_bytes: Some(42),
                    draft_model: None,
                    embedding: None,
                },
            )?]),
            Effect::LocalModelEmbed {
                app,
                model,
                texts,
                query,
            } => Ok(vec![embedded_event(&EmbeddedRecord {
                app: app.clone(),
                model: model.clone(),
                query: *query,
                dim: 3,
                // Exactly-representable f32s so the JSON round-trip is unambiguous.
                vectors: texts.iter().map(|_| vec![0.5, 0.25, 0.125]).collect(),
                duration_ms: 1,
            })?]),
            other => Err(Error::InvalidInput(format!(
                "stub runner cannot perform {other:?}"
            ))),
        }
    }
}

#[test]
fn register_upserts_spec_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();

    core.dispatch(req(
        "local-model.register",
        &[
            "qwen",
            "llama_cpp",
            "/models/qwen.gguf",
            "--context",
            "8192",
            "--temp",
            "0.7",
        ],
    ))
    .unwrap();
    let spec = &core.state().local_model.specs["qwen"];
    assert_eq!(spec.local_path, "/models/qwen.gguf");
    assert_eq!(spec.format, "gguf");
    assert_eq!(spec.context_length, Some(8192));
    assert_eq!(spec.temperature_milli, Some(700));

    // Re-registering the same id overwrites the spec (one fact per id).
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen-q8.gguf"],
    ))
    .unwrap();
    assert_eq!(core.state().local_model.specs.len(), 1);
    assert_eq!(
        core.state().local_model.specs["qwen"].local_path,
        "/models/qwen-q8.gguf"
    );
    assert_eq!(core.state().local_model.specs["qwen"].context_length, None);

    assert!(core.replay_matches().unwrap());
    // A cold reopen rebuilds the same specs from the log alone.
    assert_eq!(
        Core::open(&log).unwrap().state().local_model.specs["qwen"].local_path,
        "/models/qwen-q8.gguf"
    );
}

#[test]
fn rm_removes_spec_and_rejects_unknown_ids() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();

    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();
    core.dispatch(req("local-model.rm", &["qwen"])).unwrap();
    assert!(core.state().local_model.specs.is_empty());

    assert!(matches!(
        core.dispatch(req("local-model.rm", &["qwen"])),
        Err(Error::InvalidInput(_))
    ));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn ask_and_pull_validate_purely_before_any_effect() {
    let dir = tempdir().unwrap();
    // A pure core (NoEffects): a valid ask reaches the runner and is refused…
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();
    assert!(core
        .dispatch(req("local-model.ask", &["demo", "hi"]))
        .unwrap_err()
        .to_string()
        .contains("no effect runner"));

    // …while bad requests are rejected in decide, before the runner.
    assert_eq!(
        core.dispatch(req("local-model.ask", &["ghost", "hi"])),
        Err(Error::AppNotFound("ghost".into()))
    );
    assert!(matches!(
        core.dispatch(req(
            "local-model.ask",
            &["demo", "--model", "unregistered", "hi"]
        )),
        Err(Error::InvalidInput(_))
    ));
    assert!(matches!(
        core.dispatch(req(
            "local-model.ask",
            &[
                "demo",
                "--schema",
                "{}",
                "--grammar",
                "root ::= \"x\"",
                "hi"
            ],
        )),
        Err(Error::InvalidInput(_))
    ));
    // Pull validation is pure too: a malformed repo never reaches the runner.
    assert!(matches!(
        core.dispatch(req("local-model.pull", &["qwen", "not-a-repo", "m.gguf"])),
        Err(Error::InvalidInput(_))
    ));
    // A bare pull resolves to the recommended model before the runner refuses.
    let bare = core
        .dispatch(req("local-model.pull", &[]))
        .unwrap_err()
        .to_string();
    assert!(
        bare.contains(terrane_cap_local_model::RECOMMENDED_GGUF_REPO),
        "{bare}"
    );
}

#[test]
fn default_model_selection_flows_through_ask() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubLlm).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "local-model.register",
        &["first", "llama_cpp", "/models/first.gguf"],
    ))
    .unwrap();
    core.dispatch(req(
        "local-model.register",
        &["second", "llama_cpp", "/models/second.gguf"],
    ))
    .unwrap();
    // First registration is the automatic default.
    assert_eq!(
        core.state().local_model.default_model.as_deref(),
        Some("first")
    );

    core.dispatch(req("local-model.ask", &["demo", "hi"]))
        .unwrap();
    assert_eq!(core.state().local_model.turns["demo"][0].model, "first");

    // An explicit default redirects subsequent asks.
    core.dispatch(req("local-model.default", &["second"]))
        .unwrap();
    core.dispatch(req("local-model.ask", &["demo", "hi again"]))
        .unwrap();
    assert_eq!(core.state().local_model.turns["demo"][1].model, "second");

    // Removing the default clears it; ask then explains itself.
    core.dispatch(req("local-model.rm", &["second"])).unwrap();
    assert_eq!(core.state().local_model.default_model, None);
    let err = core
        .dispatch(req("local-model.ask", &["demo", "hi"]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("first"), "{err}");

    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().local_model.default_model,
        None
    );
}

#[test]
fn continue_feeds_recorded_history_and_system_prompts_flow_through() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubLlm).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();

    // Two plain asks build up the transcript.
    core.dispatch(req("local-model.ask", &["demo", "first question"]))
        .unwrap();
    core.dispatch(req("local-model.ask", &["demo", "second question"]))
        .unwrap();
    let turns = &core.state().local_model.turns["demo"];
    assert!(!turns[0].continued && !turns[1].continued);

    // --continue hands both prior ok exchanges to the engine.
    core.dispatch(req(
        "local-model.ask",
        &["demo", "--continue", "third question"],
    ))
    .unwrap();
    let turn = &core.state().local_model.turns["demo"][2];
    assert!(turn.continued);
    assert_eq!(turn.response, "stub response (history=2)");

    // --system is carried into the effect and recorded on the turn.
    core.dispatch(req(
        "local-model.ask",
        &["demo", "--system", "be brief", "--continue", "fourth"],
    ))
    .unwrap();
    let turn = &core.state().local_model.turns["demo"][3];
    assert_eq!(turn.system.as_deref(), Some("be brief"));
    assert_eq!(turn.response, "stub response (history=3)");

    // A different app shares no history.
    core.dispatch(req("app.add", &["other", "Other"])).unwrap();
    core.dispatch(req(
        "local-model.ask",
        &["other", "--continue", "fresh start"],
    ))
    .unwrap();
    assert_eq!(
        core.state().local_model.turns["other"][0].response,
        "stub response (history=0)"
    );

    assert!(core.replay_matches().unwrap());
}

#[test]
fn ask_records_turns_via_runner_and_cascades_on_app_removal() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubLlm).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();

    let events = core
        .dispatch(req(
            "local-model.ask",
            &["demo", "--model", "qwen", "say", "hi"],
        ))
        .unwrap();
    assert_eq!(events.len(), 1, "one recorded response per ask");
    let turns = &core.state().local_model.turns["demo"];
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].prompt, "say hi");
    assert_eq!(turns[0].response, "stub response (history=0)");
    assert!(turns[0].constraint.is_none());

    core.dispatch(req(
        "local-model.ask",
        &["demo", "--schema", r#"{"type":"object"}"#, "say", "hi"],
    ))
    .unwrap();
    assert_eq!(
        core.state().local_model.turns["demo"][1]
            .constraint
            .as_deref(),
        Some("schema-mask")
    );
    assert!(core.replay_matches().unwrap());

    // Removing the app drops its transcript via broadcast fold but keeps the
    // machine-global spec.
    core.dispatch(req("app.remove", &["demo"])).unwrap();
    assert!(core.state().local_model.turns.is_empty());
    assert!(core.state().local_model.specs.contains_key("qwen"));
    assert!(core.replay_matches().unwrap());
    assert!(Core::open(&log)
        .unwrap()
        .state()
        .local_model
        .turns
        .is_empty());
}

/// A backend exercising the call surface: ask variants plus the chat-app
/// surface (chat context, model list, Hugging Face pull, reset).
const CALLER_BACKEND: &str = r#"
var lm = ctx.resource["local-model"];
function handle(input) {
    var verb = input[0];
    if (verb === "ask") { return String(lm.ask(input.slice(1).join(" "))); }
    if (verb === "askModel") { return String(lm.askModel(input[1], input.slice(2).join(" "))); }
    if (verb === "askJson") { return String(lm.askJson(input[1], input.slice(2).join(" "))); }
    if (verb === "chat") { return String(lm.chat(input.slice(1).join(" "))); }
    if (verb === "chatModel") { return String(lm.chatModel(input[1], input.slice(2).join(" "))); }
    if (verb === "embed") { return String(lm.embed(input.slice(1).join(" "))); }
    if (verb === "embedQuery") { return String(lm.embedQuery(input.slice(1).join(" "))); }
    if (verb === "embedModel") { return String(lm.embedModel(input[1], input.slice(2).join(" "))); }
    if (verb === "models") { return String(lm.models()); }
    if (verb === "pull") { return String(input.length > 2 ? lm.pullModel(input[1], input[2]) : lm.pullModel(input[1])); }
    if (verb === "reset") { return String(lm.resetChat()); }
    if (verb === "present") { return String(typeof lm); }
    return "?";
}
"#;

/// Install a JS app declaring the `local-model` resource on a stub-runner core.
fn install_caller(dir: &Path, log: &Path) -> Core<StubLlm> {
    let bundle = dir.join("caller");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{ "id": "caller", "name":"Caller","runtime":"js","backend":"main.js", "resources": ["local-model"] }"#,
    )
    .unwrap();
    fs::write(bundle.join("main.js"), CALLER_BACKEND).unwrap();

    let mut core = Core::open_with(log, StubLlm).unwrap();
    core.dispatch(req(
        "app.add",
        &["caller", "Caller", "--source", bundle.to_str().unwrap()],
    ))
    .unwrap();
    core.dispatch(req(
        "local-model.register",
        &["qwen", "llama_cpp", "/models/qwen.gguf"],
    ))
    .unwrap();
    core
}

#[test]
fn js_backend_calls_local_model_and_replay_never_reruns_inference() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = install_caller(dir.path(), &log);
    core.dispatch(req(
        "auth.grant",
        &[LOCAL_OWNER_SUBJECT, "caller", "local-model"],
    ))
    .unwrap();

    // ask → default model, response text handed back to JS, one recorded turn.
    let records = core
        .dispatch(req("js-runtime.run", &["caller", "ask", "say", "hi"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("stub response (history=0)")
    );
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "local-model.responded");
    let turn = &core.state().local_model.turns["caller"][0];
    assert_eq!(turn.model, "qwen");
    assert_eq!(turn.prompt, "say hi");

    // askModel names the model explicitly; askJson constrains with a schema.
    core.dispatch(req(
        "js-runtime.run",
        &["caller", "askModel", "qwen", "pick one"],
    ))
    .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("stub response (history=0)")
    );
    core.dispatch(req(
        "js-runtime.run",
        &["caller", "askJson", r#"{"type":"object"}"#, "as json"],
    ))
    .unwrap();
    let turns = &core.state().local_model.turns["caller"];
    assert_eq!(turns.len(), 3);
    assert_eq!(turns[2].constraint.as_deref(), Some("schema-mask"));

    // Option A: the log carries only recorded responses — replay rebuilds the
    // identical transcript without JS or inference.
    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().local_model.turns["caller"].len(), 3);

    // A model the decide step can't resolve fails the run, records nothing.
    let before = core.state().local_model.turns["caller"].len();
    assert!(core
        .dispatch(req(
            "js-runtime.run",
            &["caller", "askModel", "ghost", "hi"]
        ))
        .is_err());
    assert_eq!(core.state().local_model.turns["caller"].len(), before);
    assert!(core.replay_matches().unwrap());
}

#[test]
fn js_backend_embeds_and_replay_never_reembeds() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = install_caller(dir.path(), &log);
    core.dispatch(req(
        "auth.grant",
        &[LOCAL_OWNER_SUBJECT, "caller", "local-model"],
    ))
    .unwrap();

    // Registering an embedding model sets the embed default without disturbing
    // the chat default (qwen, from install_caller).
    core.dispatch(req(
        "local-model.register",
        &["nomic", "llama_cpp", "/models/nomic.gguf", "--embed"],
    ))
    .unwrap();
    assert_eq!(
        core.state().local_model.default_embed_model.as_deref(),
        Some("nomic")
    );
    assert_eq!(
        core.state().local_model.default_model.as_deref(),
        Some("qwen")
    );

    // embed → the JS app receives the vector as a JSON array; one embedded
    // record is committed.
    let records = core
        .dispatch(req("js-runtime.run", &["caller", "embed", "hello world"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("[0.5,0.25,0.125]"));
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, "local-model.embedded");

    // embedQuery (search side) and embedModel (named) also hand back vectors.
    core.dispatch(req("js-runtime.run", &["caller", "embedQuery", "find"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("[0.5,0.25,0.125]"));
    core.dispatch(req(
        "js-runtime.run",
        &["caller", "embedModel", "nomic", "a document"],
    ))
    .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("[0.5,0.25,0.125]"));

    // Option A: the embedded events fold to no-op, so no vectors enter State —
    // replay rebuilds identical State without re-running inference, and a cold
    // reopen agrees on the embed default.
    assert!(!core.state().local_model.turns.contains_key("caller"));
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log)
            .unwrap()
            .state()
            .local_model
            .default_embed_model
            .as_deref(),
        Some("nomic")
    );

    // A generation model asked to embed is refused in decide; nothing recorded.
    assert!(core
        .dispatch(req("js-runtime.run", &["caller", "embedModel", "qwen", "hi"]))
        .is_err());
    assert!(core.replay_matches().unwrap());
}

#[test]
fn js_backend_chat_surface_carries_context_pulls_and_resets() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = install_caller(dir.path(), &log);
    core.dispatch(req(
        "auth.grant",
        &[LOCAL_OWNER_SUBJECT, "caller", "local-model"],
    ))
    .unwrap();

    // models() lists the registered spec and marks the default.
    core.dispatch(req("js-runtime.run", &["caller", "models"]))
        .unwrap();
    let listed = core.take_last_output().unwrap();
    assert!(listed.contains("\"id\":\"qwen\""), "{listed}");
    assert!(listed.contains("\"default\":true"), "{listed}");

    // chat feeds back this app's prior ok exchanges as context.
    core.dispatch(req("js-runtime.run", &["caller", "chat", "first"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("stub response (history=0)")
    );
    core.dispatch(req("js-runtime.run", &["caller", "chat", "second"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("stub response (history=1)")
    );

    // resetChat starts a fresh conversation.
    core.dispatch(req("js-runtime.run", &["caller", "reset"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("ok"));
    assert!(!core.state().local_model.turns.contains_key("caller"));
    core.dispatch(req("js-runtime.run", &["caller", "chat", "third"]))
        .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("stub response (history=0)")
    );

    // pullModel downloads (stubbed), registers under a derived id, and the
    // new model is immediately usable by name.
    core.dispatch(req(
        "js-runtime.run",
        &[
            "caller",
            "pull",
            "unsloth/Qwen3.5-0.8B-GGUF",
            "Qwen3.5-0.8B-Q4_K_M.gguf",
        ],
    ))
    .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("qwen3.5-0.8b-gguf")
    );
    let spec = &core.state().local_model.specs["qwen3.5-0.8b-gguf"];
    assert_eq!(spec.backend, "llama_cpp");
    assert_eq!(spec.local_path, "/stub/Qwen3.5-0.8B-Q4_K_M.gguf");

    // A file-less pull snapshots the repo for mlx.
    core.dispatch(req(
        "js-runtime.run",
        &["caller", "pull", "mlx-community/Qwen3.5-0.8B-MLX-4bit"],
    ))
    .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("qwen3.5-0.8b-mlx-4bit")
    );
    assert_eq!(
        core.state().local_model.specs["qwen3.5-0.8b-mlx-4bit"].backend,
        "mlx"
    );
    core.dispatch(req(
        "js-runtime.run",
        &["caller", "chatModel", "qwen3.5-0.8b-mlx-4bit", "hello"],
    ))
    .unwrap();
    assert_eq!(
        core.take_last_output().as_deref(),
        Some("stub response (history=0)")
    );

    // Everything above is ordinary recorded events: replay is identical.
    assert!(core.replay_matches().unwrap());
    let reopened = Core::open(&log).unwrap();
    assert_eq!(reopened.state().local_model.specs.len(), 3);
}

#[test]
fn ungranted_local_model_resource_is_not_installed() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = install_caller(dir.path(), &log);

    // Declared in the manifest but never granted → the namespace is absent.
    core.dispatch(req("js-runtime.run", &["caller", "present"]))
        .unwrap();
    assert_eq!(core.take_last_output().as_deref(), Some("undefined"));
}

#[test]
fn responded_event_folds_recorded_generation_without_inference() {
    let mut state = State::default();
    let records = vec![responded_event(&RespondedRecord {
        app: "demo".into(),
        model: "qwen".into(),
        prompt: "say hi".into(),
        system: Some("be brief".into()),
        continued: false,
        response: "hello".to_string(),
        ok: true,
        constraint: None,
        token_count: 2,
        duration_ms: 15,
    })
    .unwrap()];

    fold_records_in_memory(&mut state, &records).unwrap();

    let turns = &state.local_model.turns["demo"];
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].model, "qwen");
    assert_eq!(turns[0].response, "hello");
    assert!(turns[0].ok);
    assert_eq!(turns[0].token_count, 2);
    assert_eq!(turns[0].system.as_deref(), Some("be brief"));
    assert!(!turns[0].continued);
}
