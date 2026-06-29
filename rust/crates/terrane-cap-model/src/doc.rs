use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, CommandDoc,
    EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

use crate::AGENTS;

pub fn model_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "model".to_string(),
        title: "Recorded Model Calls".to_string(),
        summary: "Recorded calls to supported agent CLIs so app generation and model-assisted work replay deterministically."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["model.ask".to_string()],
            queries: Vec::new(),
            events: vec!["model.responded".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: model_commands(),
        queries: Vec::new(),
        events: model_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Ask Codex and record the answer".to_string(),
            summary: "Run a supported agent at the edge and keep the prompt, response, and exit code in the event log."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane model ask demo codex build a counter app".to_string(),
            expected: "returns Effect::ModelCall; the runner records model.responded".to_string(),
        }],
        constraints: vec![
            "model.ask validates that the app exists, the agent is supported, and the prompt is non-empty."
                .to_string(),
            "The agent CLI is executed only by the edge effect runner, never by replay.".to_string(),
            "A completed call is recorded as model.responded with app id, agent, prompt, response, and exit code."
                .to_string(),
            "Replay folds recorded model.responded events into an ordered per-app transcript.".to_string(),
            "Folding app.removed removes all recorded model turns for that app.".to_string(),
        ],
        limits: vec![
            limit("supportedAgents", &AGENTS.join(","), "Initial recorded agent CLI allow-list."),
            limit("transcriptScope", "app", "Model turns are stored under the app that requested them."),
        ],
        compatibility: vec![
            "Model availability is outside replay; deterministic behavior depends on recording model.responded once at the edge."
                .to_string(),
            "App removal cleanup is driven by the app.removed subscription and does not require a model-specific command."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::ModelCall is transient. model.responded is the durable replay input and stores the observed agent output."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn model_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "model.ask",
        &[
            param(
                "app",
                "Existing app id that owns the recorded turn.",
                "app_id",
            ),
            param("agent", "Supported agent CLI name.", "agent"),
            param("prompt", "Prompt text passed to the agent CLI.", "string"),
        ],
        "effect",
        "Validate one app-scoped agent request and return the edge effect.",
    )
    .with_errors(&["app not found", "unsupported agent", "empty prompt"])
    .with_effects(&["ModelCall"])
    .with_emits(&["model.responded"])]
}

fn model_events() -> Vec<EventDoc> {
    vec![event_doc(
        "model.responded",
        &[
            param("app", "App id that requested the model call.", "app_id"),
            param("agent", "Agent CLI that ran.", "agent"),
            param("prompt", "Prompt supplied to the agent.", "string"),
            param("response", "Recorded agent output.", "string"),
            param("exit_code", "Agent process exit code.", "i32"),
        ],
        "Records the observed agent response for replay.",
    )
    .with_effects(&["appends ModelState.turns[app]"])]
}
