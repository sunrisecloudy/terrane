//! The `local-model` capability — locally-run LLM inference, recorded.
//!
//! Like `model`, generation is an [`Effect`](terrane_cap_interface::Effect) run
//! at the edge (the llama.cpp engine lives outside this crate); its output is
//! recorded as an event, so replay reproduces the transcript without ever
//! re-running inference. Model specs are plain registered facts. Reacts to
//! `app.removed` by dropping that app's transcript.

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod doc;
mod events;
mod types;

pub use events::{
    chat_cleared_event, default_set_event, registered_event, removed_event, responded_event,
    RespondedRecord,
};
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
                EventSpec {
                    kind: "local-model.chat-cleared",
                },
            ],
            queries: Vec::new(),
            resources: vec![
                ResourceMethod::Call {
                    name: "ask",
                    params: &["prompt"],
                },
                ResourceMethod::Call {
                    name: "askModel",
                    params: &["model", "prompt"],
                },
                ResourceMethod::Call {
                    name: "askJson",
                    params: &["schema", "prompt"],
                },
                ResourceMethod::Call {
                    name: "chat",
                    params: &["prompt"],
                },
                ResourceMethod::Call {
                    name: "chatModel",
                    params: &["model", "prompt"],
                },
                ResourceMethod::Call {
                    name: "pullModel",
                    params: &["repo", "file"],
                },
                ResourceMethod::Call {
                    name: "resetChat",
                    params: &[],
                },
                ResourceMethod::Read {
                    name: "models",
                    params: &[],
                },
            ],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "local-model",
                &["call", "read"],
                "Recorded local LLM generations (default or named model), the registered \
                 model list, and Hugging Face pulls.",
            )],
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
            // ResourceMethod::Call routes (app-scoped args, positional).
            "local-model.askModel" => commands::decide_ask_model(ctx, args),
            "local-model.askJson" => commands::decide_ask_json(ctx, args),
            "local-model.chat" => commands::decide_chat(ctx, args),
            "local-model.chatModel" => commands::decide_chat_model(ctx, args),
            "local-model.pullModel" => commands::decide_pull_model(ctx, args),
            "local-model.resetChat" => commands::decide_reset_chat(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        events::fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        events::describe(record)
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "models" => commands::read_models(ctx.state, args),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: local-model.{other}"
            ))),
        }
    }

    fn resource_call_output(
        &self,
        _state: &dyn StateStore,
        _app: &str,
        method: &str,
        records: &[EventRecord],
    ) -> Result<ReadValue> {
        match method {
            "ask" | "askModel" | "askJson" | "chat" | "chatModel" => Ok(ReadValue::OptString(
                events::response_text_from_records(records),
            )),
            "pullModel" => Ok(ReadValue::OptString(events::registered_id_from_records(
                records,
            ))),
            "resetChat" => Ok(ReadValue::OptString(Some("ok".to_string()))),
            other => Err(Error::InvalidInput(format!(
                "local-model.{other} is not a callable resource"
            ))),
        }
    }
}

#[cfg(test)]
mod tests;
