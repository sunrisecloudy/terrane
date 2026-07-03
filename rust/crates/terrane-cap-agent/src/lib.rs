//! The `agent` capability — presentation + behaviour config for the assistant
//! agents a host offers in its shell (e.g. "Sara, the beautifier").
//!
//! This owns only the *definition* of an agent: its display name, personality
//! (the system-prompt flavour handed to the harness), the default harness and
//! model it runs on, an avatar colour, and the list of capabilities it is
//! allowed to touch. It is deliberately separate from `auth`, which owns the
//! security *principal* (`agent:{owner}:{id}`) and enforced permission grants —
//! this slice is the human-facing card, not the identity. Running the agent is
//! an edge concern (the host shells out to the harness); nothing here performs
//! effects, so definitions replay deterministically like any other slice.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::Capability;
use terrane_cap_interface::{
    arg, decode_event, encode_event, state_mut, state_ref, CapManifest, CommandCtx, CommandSpec,
    Decision, Error, EventRecord, EventSpec, Result, StateStore,
};

mod doc;

/// The default harness an agent runs on. `opencode` is already wired at the
/// host edge and can drive an app through the host's own MCP tools.
pub const DEFAULT_HARNESS: &str = "opencode";

/// The default model, expressed in opencode's `provider/model` form. The
/// `opencode-go` plan provider serves `kimi-k2.7-code`; each agent may override
/// this in its setup.
pub const DEFAULT_MODEL: &str = "opencode-go/kimi-k2.7-code";

/// One assistant agent as the user configures it in the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentDef {
    pub id: String,
    pub name: String,
    /// Free-text personality prepended to the harness prompt as the agent's
    /// standing instruction ("You are Sara, who loves to beautify things…").
    pub personality: String,
    /// Harness that runs the agent (default [`DEFAULT_HARNESS`]).
    pub harness: String,
    /// Model in `provider/model` form (default [`DEFAULT_MODEL`]).
    pub model: String,
    /// Avatar colour, a `#rrggbb` hex used by the shell's stacked avatars.
    pub color: String,
    /// Capabilities this agent is allowed to touch. Presentation-only in this
    /// slice; enforcement lives in `auth`.
    pub allowed_caps: Vec<String>,
}

/// This capability's slice of State: agents keyed by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentState {
    pub agents: BTreeMap<String, AgentDef>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Saved {
    id: String,
    name: String,
    personality: String,
    harness: String,
    model: String,
    color: String,
    allowed_caps: Vec<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Removed {
    id: String,
}

impl From<Saved> for AgentDef {
    fn from(e: Saved) -> Self {
        AgentDef {
            id: e.id,
            name: e.name,
            personality: e.personality,
            harness: e.harness,
            model: e.model,
            color: e.color,
            allowed_caps: e.allowed_caps,
        }
    }
}

impl AgentDef {
    fn to_saved(&self) -> Saved {
        Saved {
            id: self.id.clone(),
            name: self.name.clone(),
            personality: self.personality.clone(),
            harness: self.harness.clone(),
            model: self.model.clone(),
            color: self.color.clone(),
            allowed_caps: self.allowed_caps.clone(),
        }
    }
}

pub struct AgentCapability;

impl Capability for AgentCapability {
    fn namespace(&self) -> &'static str {
        "agent"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "agent.create",
                },
                CommandSpec {
                    name: "agent.update",
                },
                CommandSpec {
                    name: "agent.remove",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "agent.created",
                },
                EventSpec {
                    kind: "agent.updated",
                },
                EventSpec {
                    kind: "agent.removed",
                },
            ],
            queries: Vec::new(),
            resources: Vec::new(),
            grant_resources: Vec::new(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::agent_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "agent.create" => {
                let fields = parse_fields(args)?;
                let id = fields.id;
                validate_agent_id(&id)?;
                let display = fields
                    .name
                    .filter(|n| !n.trim().is_empty())
                    .ok_or_else(|| Error::InvalidInput("agent name must not be empty".into()))?;
                if state_ref::<AgentState>(ctx.state, "agent")?
                    .agents
                    .contains_key(&id)
                {
                    return Err(Error::InvalidInput(format!("agent already exists: {id}")));
                }
                let def = AgentDef {
                    id,
                    name: display,
                    personality: fields.personality.unwrap_or_default(),
                    harness: fields.harness.unwrap_or_else(|| DEFAULT_HARNESS.to_string()),
                    model: fields.model.unwrap_or_else(|| DEFAULT_MODEL.to_string()),
                    color: fields.color.unwrap_or_else(default_color),
                    allowed_caps: fields.allowed_caps.unwrap_or_default(),
                };
                Ok(Decision::Commit(vec![encode_event(
                    "agent.created",
                    &def.to_saved(),
                )?]))
            }
            "agent.update" => {
                let fields = parse_fields(args)?;
                let id = fields.id;
                let current = state_ref::<AgentState>(ctx.state, "agent")?
                    .agents
                    .get(&id)
                    .cloned()
                    .ok_or_else(|| Error::InvalidInput(format!("agent not found: {id}")))?;
                let next = AgentDef {
                    id: current.id,
                    name: fields.name.unwrap_or(current.name),
                    personality: fields.personality.unwrap_or(current.personality),
                    harness: fields.harness.unwrap_or(current.harness),
                    model: fields.model.unwrap_or(current.model),
                    color: fields.color.unwrap_or(current.color),
                    allowed_caps: fields.allowed_caps.unwrap_or(current.allowed_caps),
                };
                if next.name.trim().is_empty() {
                    return Err(Error::InvalidInput("agent name must not be empty".into()));
                }
                Ok(Decision::Commit(vec![encode_event(
                    "agent.updated",
                    &next.to_saved(),
                )?]))
            }
            "agent.remove" => {
                let id = arg(args, 0, "agent id")?;
                if !state_ref::<AgentState>(ctx.state, "agent")?
                    .agents
                    .contains_key(&id)
                {
                    return Err(Error::InvalidInput(format!("agent not found: {id}")));
                }
                Ok(Decision::Commit(vec![encode_event(
                    "agent.removed",
                    &Removed { id },
                )?]))
            }
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        match record.kind.as_str() {
            "agent.created" | "agent.updated" => {
                let e: Saved = decode_event(record)?;
                let def: AgentDef = e.into();
                state_mut::<AgentState>(state, "agent")?
                    .agents
                    .insert(def.id.clone(), def);
            }
            "agent.removed" => {
                let e: Removed = decode_event(record)?;
                state_mut::<AgentState>(state, "agent")?.agents.remove(&e.id);
            }
            _ => {}
        }
        Ok(())
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "agent.created" => {
                let e: Saved = decode_event(record).ok()?;
                Some(format!(
                    "agent.created {} \"{}\" [{}/{}]",
                    e.id, e.name, e.harness, e.model
                ))
            }
            "agent.updated" => {
                let e: Saved = decode_event(record).ok()?;
                Some(format!("agent.updated {} \"{}\"", e.id, e.name))
            }
            "agent.removed" => {
                let e: Removed = decode_event(record).ok()?;
                Some(format!("agent.removed {}", e.id))
            }
            _ => None,
        }
    }
}

/// Parsed command fields. Every value is optional so `agent.update` can carry a
/// partial change; `agent.create` fills the required ones or defaults.
struct Fields {
    id: String,
    name: Option<String>,
    personality: Option<String>,
    harness: Option<String>,
    model: Option<String>,
    color: Option<String>,
    allowed_caps: Option<Vec<String>>,
}

/// Parse `<id> [name words…] [--personality <p>] [--model <m>] [--harness <h>]
/// [--color <#rrggbb>] [--cap <ns>]…`. Trailing non-flag words form the name so
/// `agent.create sara Sara the beautifier` reads naturally; `--name` also works.
fn parse_fields(args: &[String]) -> Result<Fields> {
    let id = arg(args, 0, "agent id")?;
    let mut name_parts: Vec<&str> = Vec::new();
    let mut personality = None;
    let mut harness = None;
    let mut model = None;
    let mut color = None;
    let mut caps: Option<Vec<String>> = None;
    let mut name_flag: Option<String> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--name" => {
                name_flag = Some(take_value(args, &mut i, "--name")?);
            }
            "--personality" => {
                personality = Some(take_value(args, &mut i, "--personality")?);
            }
            "--harness" => {
                harness = Some(take_value(args, &mut i, "--harness")?);
            }
            "--model" => {
                model = Some(take_value(args, &mut i, "--model")?);
            }
            "--color" => {
                color = Some(take_value(args, &mut i, "--color")?);
            }
            "--cap" => {
                let value = take_value(args, &mut i, "--cap")?;
                caps.get_or_insert_with(Vec::new).push(value);
            }
            word => {
                name_parts.push(word);
                i += 1;
            }
        }
    }

    let name = name_flag.or_else(|| {
        if name_parts.is_empty() {
            None
        } else {
            Some(name_parts.join(" "))
        }
    });

    Ok(Fields {
        id,
        name,
        personality,
        harness,
        model,
        color,
        allowed_caps: caps,
    })
}

/// Consume the value that follows a flag, advancing the cursor past both.
fn take_value(args: &[String], i: &mut usize, flag: &str) -> Result<String> {
    let value = args
        .get(*i + 1)
        .ok_or_else(|| Error::InvalidInput(format!("`{flag}` needs a value")))?
        .clone();
    *i += 2;
    Ok(value)
}

fn validate_agent_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(Error::InvalidInput("agent id must not be empty".into()));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "agent id is unsafe: {id:?}; use ASCII letters, digits, '-' or '_'"
        )));
    }
    Ok(())
}

/// A calm default avatar colour when none is given.
fn default_color() -> String {
    "#6b7bff".to_string()
}
