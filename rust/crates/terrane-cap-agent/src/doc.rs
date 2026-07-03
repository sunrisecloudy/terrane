use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, CommandDoc,
    EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

use crate::{DEFAULT_HARNESS, DEFAULT_MODEL};

pub fn agent_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "agent".to_string(),
        title: "Assistant Agents".to_string(),
        summary:
            "Presentation and behaviour config for the assistant agents a host offers in its shell."
                .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["host-implementer".to_string(), "app-author".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "agent.create".to_string(),
                "agent.update".to_string(),
                "agent.remove".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "agent.created".to_string(),
                "agent.updated".to_string(),
                "agent.removed".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: agent_commands(),
        queries: Vec::new(),
        events: agent_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Define a beautifier agent".to_string(),
            summary: "Register an agent with a personality; harness and model default to opencode."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane agent create sara Sara --personality \"You love making things beautiful\""
                .to_string(),
            expected: "records agent.created with defaults filled in".to_string(),
        }],
        constraints: vec![
            "agent.create validates the id is a safe slug and unique, and requires a non-empty name."
                .to_string(),
            "agent.update applies a partial change over the current definition; unspecified fields keep their value."
                .to_string(),
            "This slice is presentation-only: permission enforcement over the listed capabilities lives in `auth`."
                .to_string(),
        ],
        limits: vec![
            limit("defaultHarness", DEFAULT_HARNESS, "Harness used when none is given."),
            limit("defaultModel", DEFAULT_MODEL, "Model used when none is given (provider/model form)."),
        ],
        compatibility: vec![
            "Running an agent is an edge concern; this capability only stores its definition."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Identity vs presentation".to_string(),
                body: "The security principal (agent:{owner}:{id}) and enforced grants belong to `auth`; this slice is the human-facing card only."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn agent_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "agent.create",
            &[
                param("id", "Safe slug id for the agent.", "string"),
                param("name", "Display name (trailing words or --name).", "string"),
                param("personality", "--personality: standing instruction.", "string"),
                param("model", "--model: provider/model to run on.", "string"),
                param("harness", "--harness: harness that runs the agent.", "string"),
                param("color", "--color: avatar #rrggbb.", "string"),
                param("cap", "--cap: an allowed capability (repeatable).", "string"),
            ],
            "commit",
            "Register a new agent definition, filling defaults.",
        )
        .with_errors(&["duplicate id", "unsafe id", "empty name"])
        .with_emits(&["agent.created"]),
        command_doc(
            "agent.update",
            &[
                param("id", "Existing agent id.", "string"),
                param("...", "Any create field to override.", "string"),
            ],
            "commit",
            "Apply a partial change to an existing agent.",
        )
        .with_errors(&["agent not found", "empty name"])
        .with_emits(&["agent.updated"]),
        command_doc(
            "agent.remove",
            &[param("id", "Existing agent id.", "string")],
            "commit",
            "Remove an agent definition.",
        )
        .with_errors(&["agent not found"])
        .with_emits(&["agent.removed"]),
    ]
}

fn agent_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "agent.created",
            &[
                param("id", "Agent id.", "string"),
                param("name", "Display name.", "string"),
                param("personality", "Standing instruction.", "string"),
                param("harness", "Harness that runs it.", "string"),
                param("model", "Model in provider/model form.", "string"),
                param("color", "Avatar #rrggbb.", "string"),
                param("allowed_caps", "Allowed capability namespaces.", "string[]"),
            ],
            "Records a new agent definition.",
        )
        .with_effects(&["inserts AgentState.agents[id]"]),
        event_doc(
            "agent.updated",
            &[param("...", "Full definition after the update.", "object")],
            "Records the definition after a partial update.",
        )
        .with_effects(&["replaces AgentState.agents[id]"]),
        event_doc(
            "agent.removed",
            &[param("id", "Agent id.", "string")],
            "Records removal of an agent.",
        )
        .with_effects(&["removes AgentState.agents[id]"]),
    ]
}
