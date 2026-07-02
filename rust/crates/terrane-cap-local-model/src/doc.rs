use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, CommandDoc,
    EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

use crate::types::BACKENDS;

pub fn local_model_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "local-model".to_string(),
        title: "Recorded Local Model Inference".to_string(),
        summary:
            "Registered on-device LLMs (llama.cpp/GGUF and MLX on Apple Silicon) with recorded, \
             replayable generations, optionally constrained to a JSON schema or GBNF grammar."
                .to_string(),
        status: "alpha".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "local-model.register".to_string(),
                "local-model.pull".to_string(),
                "local-model.rm".to_string(),
                "local-model.ask".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "local-model.registered".to_string(),
                "local-model.removed".to_string(),
                "local-model.responded".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: local_model_commands(),
        queries: Vec::new(),
        events: local_model_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Pull a model and ask it for structured output".to_string(),
            summary: "Download Qwen3.5-0.8B once, then run a schema-constrained generation \
                      recorded in the event log."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane local-model pull qwen3_5_0_8b unsloth/Qwen3.5-0.8B-GGUF \
                   Qwen3.5-0.8B-Q4_K_M.gguf\nterrane local-model ask demo qwen3_5_0_8b --schema \
                   '{\"type\":\"object\",\"properties\":{\"answer\":{\"type\":\"string\"}},\"required\":[\"answer\"]}' \
                   what is terrane"
                .to_string(),
            expected: "streams tokens while generating; records local-model.responded".to_string(),
        }],
        constraints: vec![
            "Inference runs only at the edge effect runner, never during replay; replay folds \
             the recorded local-model.responded event."
                .to_string(),
            "Model specs are global machine configuration; transcripts are app-scoped and are \
             dropped when the app is removed."
                .to_string(),
            "register/rm validate purely; the weights file is checked at inference time, at the \
             edge."
                .to_string(),
            "--schema (JSON object) and --grammar (GBNF) are mutually exclusive; both constrain \
             decoding at the edge."
                .to_string(),
            "On llama_cpp a schema is token-mask enforced (llguidance); on mlx it is \
             prompt-guided with extraction, validation, and one retry. --grammar is \
             llama_cpp-only."
                .to_string(),
            "Different backends for the same weights (gguf vs mlx) are two engine targets, not \
             interchangeable engines: quantization, tokenizer/template handling, and samplers \
             all shift output."
                .to_string(),
        ],
        limits: vec![
            limit(
                "supportedBackends",
                &BACKENDS.join(","),
                "llama.cpp is the production baseline; mlx is the Apple-acceleration path (needs the mlx-lm runtime).",
            ),
            limit(
                "pullSource",
                "huggingface.co",
                "local-model.pull resolves org/name repos on Hugging Face only.",
            ),
            limit(
                "transcriptScope",
                "app",
                "Recorded turns are stored under the app that requested them.",
            ),
        ],
        compatibility: vec![
            "Weights availability is outside replay; deterministic behavior depends on recording \
             local-model.responded once at the edge."
                .to_string(),
            "Registered spec paths are host-local; replaying a log on another machine rebuilds \
             the specs but not the weight files."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::LocalModelCall and Effect::LocalModelPull are transient. \
                       local-model.registered and local-model.responded are the durable replay \
                       inputs; temperature is carried in thousandths to keep state integral."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn local_model_commands() -> Vec<CommandDoc> {
    let spec_flags = [
        param(
            "--context",
            "Optional context length override (positive integer).",
            "u32",
        ),
        param(
            "--template",
            "Optional chat template override; defaults to the model's embedded template.",
            "string",
        ),
        param(
            "--max-tokens",
            "Optional generation cap (positive integer).",
            "u32",
        ),
        param(
            "--temp",
            "Optional sampling temperature between 0.0 and 2.0.",
            "number",
        ),
    ];
    let mut register_params = vec![
        param(
            "id",
            "Model id ([A-Za-z0-9_.-]+) the spec is stored under.",
            "model_id",
        ),
        param("backend", "Inference backend; llama_cpp today.", "backend"),
        param(
            "path",
            "Path to the weights file on this machine.",
            "string",
        ),
    ];
    register_params.extend(spec_flags.iter().cloned());
    let mut pull_params = vec![
        param(
            "id",
            "Model id ([A-Za-z0-9_.-]+) the spec is stored under.",
            "model_id",
        ),
        param("repo", "Hugging Face repo as org/name.", "string"),
        param("file", "GGUF file name inside the repo.", "string"),
    ];
    pull_params.extend(spec_flags.iter().cloned());
    vec![
        command_doc(
            "local-model.register",
            &register_params,
            "commit",
            "Record (or overwrite) a model spec pointing at weights already on disk.",
        )
        .with_errors(&[
            "invalid id",
            "unknown or reserved backend",
            "empty path",
            "bad option",
        ])
        .with_emits(&["local-model.registered"]),
        command_doc(
            "local-model.pull",
            &pull_params,
            "effect",
            "Download weights from Hugging Face at the edge, then record the registered spec.",
        )
        .with_errors(&[
            "invalid id",
            "malformed repo",
            "non-gguf or unsafe file name",
        ])
        .with_effects(&["LocalModelPull"])
        .with_emits(&["local-model.registered"]),
        command_doc(
            "local-model.rm",
            &[param("id", "Registered model id.", "model_id")],
            "commit",
            "Unregister a model spec; weight files on disk are untouched.",
        )
        .with_errors(&["unknown model id"])
        .with_emits(&["local-model.removed"]),
        command_doc(
            "local-model.ask",
            &[
                param(
                    "app",
                    "Existing app id that owns the recorded turn.",
                    "app_id",
                ),
                param("model", "Registered model id.", "model_id"),
                param(
                    "--schema",
                    "Optional JSON schema (object) the output must satisfy.",
                    "json",
                ),
                param(
                    "--grammar",
                    "Optional raw GBNF grammar constraining the output.",
                    "string",
                ),
                param("prompt", "Prompt text for the model.", "string"),
            ],
            "effect",
            "Validate one app-scoped local generation request and return the edge effect.",
        )
        .with_errors(&[
            "app not found",
            "unknown model id",
            "empty prompt",
            "invalid schema",
            "--schema and --grammar together",
        ])
        .with_effects(&["LocalModelCall"])
        .with_emits(&["local-model.responded"]),
    ]
}

fn local_model_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "local-model.registered",
            &[
                param("id", "Model id the spec is stored under.", "model_id"),
                param("backend", "Inference backend.", "backend"),
                param("format", "Weights format (gguf).", "string"),
                param(
                    "local_path",
                    "Weights path on the recording machine.",
                    "string",
                ),
                param("context_length", "Optional context override.", "u32?"),
                param("chat_template", "Optional template override.", "string?"),
                param("max_tokens", "Optional generation cap.", "u32?"),
                param(
                    "temperature_milli",
                    "Optional temperature in thousandths.",
                    "u32?",
                ),
                param(
                    "source",
                    "Origin (hf:<repo>/<file>) when pulled.",
                    "string?",
                ),
                param("size_bytes", "Downloaded size when pulled.", "u64?"),
            ],
            "Records a model spec (register and pull both emit it; upsert by id).",
        )
        .with_effects(&["sets LocalModelState.specs[id]"]),
        event_doc(
            "local-model.removed",
            &[param("id", "Model id removed.", "model_id")],
            "Records an unregistered model spec.",
        )
        .with_effects(&["removes LocalModelState.specs[id]"]),
        event_doc(
            "local-model.responded",
            &[
                param("app", "App id that requested the generation.", "app_id"),
                param("model", "Registered model id that ran.", "model_id"),
                param("prompt", "Prompt supplied to the model.", "string"),
                param(
                    "response",
                    "Recorded model output (possibly partial on failure).",
                    "string",
                ),
                param("ok", "Whether generation completed cleanly.", "bool"),
                param(
                    "constrained",
                    "Whether a schema/grammar constrained decoding.",
                    "bool",
                ),
                param("token_count", "Tokens generated.", "u32"),
                param("duration_ms", "Wall-clock generation time.", "u64"),
            ],
            "Records the observed local generation for replay.",
        )
        .with_effects(&["appends LocalModelState.turns[app]"]),
    ]
}
