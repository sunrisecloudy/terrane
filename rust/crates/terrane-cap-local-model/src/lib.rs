//! The `local-model` capability — locally-run LLM inference, recorded.
//!
//! Like `model`, generation is an [`Effect`](terrane_cap_interface::Effect) run
//! at the edge (the llama.cpp engine lives outside this crate); its output is
//! recorded as an event, so replay reproduces the transcript without ever
//! re-running inference. Model specs are plain registered facts. Reacts to
//! `app.removed` by dropping that app's transcript.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, Result, StateStore,
};

mod commands;
mod doc;
mod events;
mod types;

pub use events::{default_set_event, registered_event, removed_event, responded_event};
pub use types::{
    LocalModelSpec, LocalModelState, LocalModelTurn, BACKENDS, RECOMMENDED_GGUF_FILE,
    RECOMMENDED_GGUF_REPO, RECOMMENDED_MLX_MODEL_ID, RECOMMENDED_MLX_REPO, RECOMMENDED_MODEL_ID,
};

pub struct LocalModelCapability;

impl Capability for LocalModelCapability {
    fn namespace(&self) -> &'static str {
        "local-model"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "local-model.register",
                },
                CommandSpec {
                    name: "local-model.pull",
                },
                CommandSpec {
                    name: "local-model.rm",
                },
                CommandSpec {
                    name: "local-model.default",
                },
                CommandSpec {
                    name: "local-model.ask",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "local-model.registered",
                },
                EventSpec {
                    kind: "local-model.removed",
                },
                EventSpec {
                    kind: "local-model.default-set",
                },
                EventSpec {
                    kind: "local-model.responded",
                },
            ],
            queries: Vec::new(),
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::local_model_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "local-model.register" => commands::decide_register(ctx, args),
            "local-model.pull" => commands::decide_pull(ctx, args),
            "local-model.rm" => commands::decide_rm(ctx, args),
            "local-model.default" => commands::decide_default(ctx, args),
            "local-model.ask" => commands::decide_ask(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }
}

#[cfg(test)]
mod tests;
