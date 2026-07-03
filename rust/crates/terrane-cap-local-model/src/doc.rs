use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
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
                "local-model.default".to_string(),
                "local-model.ask".to_string(),
                "local-model.embed".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "local-model.registered".to_string(),
                "local-model.removed".to_string(),
                "local-model.default-set".to_string(),
                "local-model.responded".to_string(),
                "local-model.chat-cleared".to_string(),
                "local-model.embedded".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_method_docs(),
        },
        commands: local_model_commands(),
        queries: Vec::new(),
        events: local_model_events(),
        resources: vec![ResourceDoc {
            namespace: "local-model".to_string(),
            summary: "Backend resource surface installed as ctx.resource[\"local-model\"] \
                      (bracket access — the namespace contains a dash) for apps that declare \
                      the local-model resource and hold the namespace grant (verb: call). Each \
                      method runs one recorded generation at the edge and returns the reply \
                      text; the recorded local-model.responded event replays without re-running \
                      inference."
                .to_string(),
            methods: resource_method_docs(),
        }],
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
        },
        ExampleDoc {
            title: "Chat with the local model from an app backend".to_string(),
            summary: "Apps declare the local-model resource and call it with bracket access \
                      (the namespace contains a dash). Requires the local-model grant."
                .to_string(),
            language: "js".to_string(),
            code: "var lm = ctx.resource[\"local-model\"];\nfunction handle(input) {\n  var reply = lm.ask(input.join(\" \"));\n  return reply == null ? \"(generation failed)\" : reply;\n}"
                .to_string(),
            expected: "returns the model's reply text; one local-model.responded event is recorded"
                .to_string(),
        }],
        constraints: vec![
            "Inference runs only at the edge effect runner, never during replay; replay folds \
             the recorded local-model.responded event."
                .to_string(),
            "Model specs are global machine configuration; transcripts are app-scoped and are \
             dropped when the app is removed."
                .to_string(),
            "--continue rebuilds the conversation from the app's recorded ok turns with the \
             same model (most recent 8), oldest first; --system prepends a system message."
                .to_string(),
            "register/rm validate purely; the weights file is checked at inference time, at the \
             edge."
                .to_string(),
            "Hugging Face pulls reuse the standard HF hub cache when possible; pulled GGUF \
             specs record an hf:<repo>/<file> source so hosts can resolve the common cache if \
             the originally recorded path moves."
                .to_string(),
            "--schema (JSON object) and --grammar (GBNF) are mutually exclusive; both constrain \
             decoding at the edge."
                .to_string(),
            "On llama_cpp a schema is token-mask enforced (llguidance); on mlx it is \
             prompt-guided with extraction, validation, and one retry. --grammar is \
             llama_cpp-only."
                .to_string(),
            "The mlx backend keeps one resident mlx_lm.server per home (auto-start on ask, \
             idle auto-exit, lazy restart); provision the runtime with `terrane local-model \
             setup mlx` and inspect it with `terrane local-model server status|stop` — host \
             verbs that record nothing."
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
                body: "Effect::LocalModelCall, Effect::LocalModelPull, and Effect::LocalModelEmbed \
                       are transient. local-model.registered and local-model.responded are the \
                       durable replay inputs; local-model.embedded records the vectors for the \
                       caller but is not folded into State (floats aren't replay-comparable). \
                       Temperature is carried in thousandths to keep state integral."
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
        param(
            "--draft",
            "Optional draft model for speculative decoding (mlx only; needs a \
             smaller same-tokenizer model and rewindable caches).",
            "string",
        ),
        param(
            "--embed",
            "Mark this as an embedding model using the recommended preset (nomic); \
             a bare `pull --embed` fetches the recommended embedding model.",
            "flag",
        ),
        param(
            "--embed-preset",
            "Mark this as an embedding model using a named encoder preset (e.g. nomic), \
             which sets pooling, prefixes, and normalization.",
            "string",
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
            "Optional model id; a bare pull uses the recommended model.",
            "model_id",
        ),
        param(
            "repo",
            "Optional Hugging Face repo as org/name (defaults to the recommended model).",
            "string",
        ),
        param("file", "GGUF file name inside the repo.", "string"),
        param("--backend", "gguf (default) or mlx.", "backend"),
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
            "Unregister a model spec; weight files on disk are untouched. Clears the \
             default when it pointed here.",
        )
        .with_errors(&["unknown model id"])
        .with_emits(&["local-model.removed"]),
        command_doc(
            "local-model.default",
            &[param("id", "Registered model id.", "model_id")],
            "commit",
            "Choose the model asks use when --model is omitted (the first registered \
             model is the automatic default).",
        )
        .with_errors(&["unknown model id"])
        .with_emits(&["local-model.default-set"]),
        command_doc(
            "local-model.ask",
            &[
                param(
                    "app",
                    "Existing app id that owns the recorded turn.",
                    "app_id",
                ),
                param(
                    "--model",
                    "Optional registered model id; defaults to the home's default model.",
                    "model_id",
                ),
                param(
                    "--system",
                    "Optional system prompt rendered ahead of the conversation.",
                    "string",
                ),
                param(
                    "--continue",
                    "Feed back this app+model's recorded turns (most recent 8) as context.",
                    "flag",
                ),
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
        command_doc(
            "local-model.embed",
            &[
                param(
                    "app",
                    "Existing app id that owns the recorded embedding.",
                    "app_id",
                ),
                param(
                    "--model",
                    "Optional registered embedding model id; defaults to the home's \
                     default embedding model.",
                    "model_id",
                ),
                param(
                    "--query",
                    "Apply the model's query prefix (search side) instead of the \
                     document prefix (index side).",
                    "flag",
                ),
                param("text", "Text to encode into a dense vector.", "string"),
            ],
            "effect",
            "Validate one app-scoped embedding request and return the edge effect; the \
             pooled, normalized vector is recorded.",
        )
        .with_errors(&[
            "app not found",
            "unknown model id",
            "not an embedding model",
            "no embedding model registered",
            "empty text",
        ])
        .with_effects(&["LocalModelEmbed"])
        .with_emits(&["local-model.embedded"]),
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
        .with_effects(&["removes LocalModelState.specs[id]; clears a default pointing here"]),
        event_doc(
            "local-model.default-set",
            &[param(
                "id",
                "Model id that becomes the default.",
                "model_id",
            )],
            "Records an explicit default-model choice (the first registered model is the \
             automatic default).",
        )
        .with_effects(&["sets LocalModelState.default_model"]),
        event_doc(
            "local-model.chat-cleared",
            &[param(
                "app",
                "App id whose transcript is cleared.",
                "app_id",
            )],
            "Records a fresh-conversation request: the app's transcript is dropped, so \
             later chat/--continue turns start without prior context.",
        )
        .with_effects(&["removes LocalModelState.turns[app]"]),
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
                param("system", "System prompt used, when given.", "string?"),
                param(
                    "continued",
                    "Whether prior recorded turns were fed back as context.",
                    "bool",
                ),
                param("ok", "Whether generation completed cleanly.", "bool"),
                param(
                    "constraint",
                    "schema-mask, schema-guided, or grammar when decoding was constrained.",
                    "string?",
                ),
                param("token_count", "Tokens generated.", "u32"),
                param("duration_ms", "Wall-clock generation time.", "u64"),
            ],
            "Records the observed local generation for replay.",
        )
        .with_effects(&["appends LocalModelState.turns[app]"]),
        event_doc(
            "local-model.embedded",
            &[
                param("app", "App id that requested the embedding.", "app_id"),
                param("model", "Registered embedding model id that ran.", "model_id"),
                param(
                    "query",
                    "Whether the query prefix (vs the document prefix) was applied.",
                    "bool",
                ),
                param("dim", "Dimension of each returned vector.", "u32"),
                param(
                    "vectors",
                    "One pooled, (optionally) L2-normalized vector per input text.",
                    "f32[][]",
                ),
                param("duration_ms", "Wall-clock encoding time.", "u64"),
            ],
            "Records the observed vectors so replay never re-runs inference. A derived \
             read-model: the vectors are consumed by the caller at commit time and are \
             deliberately not folded into State (floats aren't replay-comparable).",
        )
        .with_effects(&["none — vectors are returned to the caller, not stored in State"]),
    ]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    let with_returns = |mut method: ResourceMethodDoc, returns: &str| {
        method.returns = returns.to_string();
        method
    };
    vec![
        with_returns(resource_method(
            "ask",
            "call",
            &[param("prompt", "The user prompt to answer.", "string")],
            "One recorded generation by the home's default local model; returns the reply text \
             (string, or null when the generation recorded a failure).",
        ), "string | null"),
        with_returns(resource_method(
            "askModel",
            "call",
            &[
                param("model", "A registered local model id.", "model_id"),
                param("prompt", "The user prompt to answer.", "string"),
            ],
            "Like ask, but names the registered model explicitly instead of using the default.",
        ), "string | null"),
        with_returns(resource_method(
            "askJson",
            "call",
            &[
                param(
                    "schema",
                    "A JSON-schema object the output must satisfy (token-mask enforced).",
                    "json",
                ),
                param("prompt", "The user prompt to answer.", "string"),
            ],
            "Schema-constrained generation by the default model; returns JSON text matching \
             the schema.",
        ), "string (JSON matching schema) | null"),
        with_returns(resource_method(
            "chat",
            "call",
            &[param("prompt", "The next user message.", "string")],
            "A conversation turn with the default model: this app's recorded exchanges \
             (most recent 8) are fed back as context, so the reply follows the ongoing \
             conversation. Returns the reply text.",
        ), "string | null"),
        with_returns(resource_method(
            "chatModel",
            "call",
            &[
                param("model", "A registered local model id.", "model_id"),
                param("prompt", "The next user message.", "string"),
            ],
            "Like chat, but with an explicitly named registered model (each model keeps \
             its own conversation context).",
        ), "string | null"),
        with_returns(resource_method(
            "embed",
            "call",
            &[param("text", "Document-side text to encode.", "string")],
            "Encode text into a dense vector with the home's default embedding model, \
             applying the model's document prefix (index side). Returns the vector as a \
             JSON array of floats (null when nothing was produced).",
        ), "string (JSON number array) | null"),
        with_returns(resource_method(
            "embedQuery",
            "call",
            &[param("text", "Search-side text to encode.", "string")],
            "Like embed, but applies the model's query prefix for the search side, so \
             query and document vectors are comparable.",
        ), "string (JSON number array) | null"),
        with_returns(resource_method(
            "embedModel",
            "call",
            &[
                param("model", "A registered embedding model id.", "model_id"),
                param("text", "Document-side text to encode.", "string"),
            ],
            "Like embed, but names the embedding model explicitly instead of using the \
             default.",
        ), "string (JSON number array) | null"),
        with_returns(resource_method(
            "pullModel",
            "call",
            &[
                param("repo", "Hugging Face repo as org/name.", "string"),
                param(
                    "file",
                    "A .gguf file inside the repo (llama_cpp backend); omit to snapshot \
                     the repo for the mlx backend.",
                    "string",
                ),
            ],
            "Download weights from Hugging Face and register them (id derived from the \
             repo name); the first registered model becomes the default. Blocking: the \
             download runs before the call returns. Returns the new model id.",
        ), "string (model id) | null"),
        with_returns(resource_method(
            "resetChat",
            "call",
            &[],
            "Start a fresh conversation: clears this app's recorded transcript so the \
             next chat call has no prior context.",
        ), "\"ok\""),
        with_returns(resource_method(
            "models",
            "read",
            &[],
            "The registered local models as a JSON array of {id, backend, default}.",
        ), "string (JSON array)"),
    ]
}
