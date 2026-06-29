//! The `model` capability — calls to agent CLIs (`claude`, `codex`), recorded.
//!
//! Like `net`, the call is an [`Effect`](crate::Effect) run at the edge; its
//! output is recorded as an event, so replay reproduces the conversation without
//! re-running the agent. Reacts to `app.removed`.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, join_tail, state_mut,
    truncate, AppId, CapManifest, CommandCtx, CommandSpec, Decision, Effect, Error, EventPattern,
    EventRecord, EventSpec, Result, StateStore,
};

/// The agents this capability knows how to drive.
pub const AGENTS: [&str; 2] = ["claude", "codex"];

/// One recorded exchange with an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelTurn {
    pub agent: String,
    pub prompt: String,
    pub response: String,
    pub exit_code: i32,
}

/// This capability's slice of State: a per-app transcript of turns, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelState {
    pub turns: BTreeMap<AppId, Vec<ModelTurn>>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Responded {
    app: String,
    agent: String,
    prompt: String,
    response: String,
    exit_code: i32,
}

/// Build the recorded event for a completed agent call. Called by an
/// [`EffectRunner`](crate::EffectRunner) once it has run the agent, so the
/// `"model.responded"` kind and payload shape stay owned by this capability.
pub fn responded_event(
    app: &str,
    agent: &str,
    prompt: &str,
    response: String,
    exit_code: i32,
) -> Result<EventRecord> {
    encode_event(
        "model.responded",
        &Responded {
            app: app.to_string(),
            agent: agent.to_string(),
            prompt: prompt.to_string(),
            response,
            exit_code,
        },
    )
}

pub struct ModelCapability;

impl Capability for ModelCapability {
    fn namespace(&self) -> &'static str {
        "model"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec { name: "model.ask" }],
            events: vec![EventSpec {
                kind: "model.responded",
            }],
            queries: Vec::new(),
            resources: Vec::new(),
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "model.ask" => {
                let app = arg(args, 0, "app")?;
                let agent = arg(args, 1, "agent (claude|codex)")?;
                let prompt = join_tail(args, 2);
                // Validate purely; the agent runs at the edge.
                ensure_app_exists(ctx.bus, &app)?;
                if !AGENTS.contains(&agent.as_str()) {
                    return Err(Error::InvalidInput(format!(
                        "unknown agent {agent:?}; expected one of {AGENTS:?}"
                    )));
                }
                if prompt.trim().is_empty() {
                    return Err(Error::InvalidInput("prompt must not be empty".into()));
                }
                Ok(Decision::Effect(Effect::ModelCall { app, agent, prompt }))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "model.responded" => {
                let e: Responded = decode_event(record)?;
                state_mut::<ModelState>(state, "model")?
                    .turns
                    .entry(e.app)
                    .or_default()
                    .push(ModelTurn {
                        agent: e.agent,
                        prompt: e.prompt,
                        response: e.response,
                        exit_code: e.exit_code,
                    });
            }
            "app.removed" => {
                let e = decode_app_removed(record)?;
                state_mut::<ModelState>(state, "model")?.turns.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind == "model.responded" {
            let e: Responded = decode_event(record).ok()?;
            let prompt = truncate(&e.prompt, 40);
            return Some(format!(
                "model.responded {} via {} (exit {}): {:?} → {} chars",
                e.app,
                e.agent,
                e.exit_code,
                prompt,
                e.response.len()
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests;
