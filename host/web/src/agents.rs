//! HTTP surface for the shell's assistant agents.
//!
//! Reads and edits the `agent` capability's slice: `GET /__terrane/agents`
//! lists the cards the top bar renders; create/update go through ordinary
//! trusted-host dispatch of `agent.create` / `agent.update`. Running an agent
//! (the assist loop) lives in [`crate::agent_jobs`]; this module only owns the
//! definitions.

use nanoserde::{DeJson, SerJson};
use terrane_cap_agent::{AgentDef, DEFAULT_HARNESS, DEFAULT_MODEL};
use tiny_http::Request;

use crate::http::{json_error, json_ok, Resp};

#[derive(SerJson)]
struct AgentView {
    id: String,
    name: String,
    personality: String,
    harness: String,
    model: String,
    color: String,
    allowed_caps: Vec<String>,
}

impl From<&AgentDef> for AgentView {
    fn from(a: &AgentDef) -> Self {
        AgentView {
            id: a.id.clone(),
            name: a.name.clone(),
            personality: a.personality.clone(),
            harness: a.harness.clone(),
            model: a.model.clone(),
            color: a.color.clone(),
            allowed_caps: a.allowed_caps.clone(),
        }
    }
}

#[derive(SerJson)]
struct AgentsResponse {
    agents: Vec<AgentView>,
    default_model: String,
    default_harness: String,
}

#[derive(DeJson, Default)]
struct AgentUpsertRequest {
    #[nserde(default)]
    id: String,
    #[nserde(default)]
    name: String,
    #[nserde(default)]
    personality: String,
    #[nserde(default)]
    harness: String,
    #[nserde(default)]
    model: String,
    #[nserde(default)]
    color: String,
    #[nserde(default)]
    allowed_caps: Vec<String>,
}

/// `GET /__terrane/agents` — the agent cards for the shell top bar.
pub fn list(core: &terrane_host::HostCore) -> Resp {
    json_ok(&agents_response(core))
}

fn agents_response(core: &terrane_host::HostCore) -> AgentsResponse {
    let agents = core
        .state()
        .agent
        .agents
        .values()
        .map(AgentView::from)
        .collect();
    AgentsResponse {
        agents,
        default_model: DEFAULT_MODEL.to_string(),
        default_harness: DEFAULT_HARNESS.to_string(),
    }
}

/// `POST /__terrane/agents` — create a new agent from a JSON body.
pub fn create(core: &mut terrane_host::HostCore, request: &mut Request) -> Resp {
    let body = match read_body(request) {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let parsed: AgentUpsertRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad agent body: {e}")),
    };
    if parsed.id.trim().is_empty() {
        return json_error(400, "agent id is required");
    }
    let mut args = vec![parsed.id.trim().to_string()];
    push_upsert_flags(&mut args, &parsed);
    match terrane_host::dispatch_on_core(core, "agent.create", &args) {
        Ok(_) => json_ok(&agents_response(core)),
        Err(e) => json_error(400, &e),
    }
}

/// `POST /__terrane/agents/{id}` — apply a partial update to an agent.
pub fn update(core: &mut terrane_host::HostCore, id: &str, request: &mut Request) -> Resp {
    let body = match read_body(request) {
        Ok(body) => body,
        Err(resp) => return resp,
    };
    let parsed: AgentUpsertRequest = match DeJson::deserialize_json(&body) {
        Ok(req) => req,
        Err(e) => return json_error(400, &format!("bad agent body: {e}")),
    };
    let mut args = vec![id.to_string()];
    push_upsert_flags(&mut args, &parsed);
    match terrane_host::dispatch_on_core(core, "agent.update", &args) {
        Ok(_) => json_ok(&agents_response(core)),
        Err(e) => json_error(400, &e),
    }
}

/// Turn a parsed upsert body into `agent.create`/`agent.update` flag args. A
/// blank field is simply omitted so `update` keeps the existing value.
fn push_upsert_flags(args: &mut Vec<String>, parsed: &AgentUpsertRequest) {
    for (flag, value) in [
        ("--name", &parsed.name),
        ("--personality", &parsed.personality),
        ("--harness", &parsed.harness),
        ("--model", &parsed.model),
        ("--color", &parsed.color),
    ] {
        if !value.trim().is_empty() {
            args.push(flag.to_string());
            args.push(value.clone());
        }
    }
    for cap in &parsed.allowed_caps {
        if !cap.trim().is_empty() {
            args.push("--cap".to_string());
            args.push(cap.clone());
        }
    }
}

fn read_body(request: &mut Request) -> Result<String, Resp> {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return Err(json_error(400, "cannot read request body"));
    }
    Ok(body)
}

/// Seed a few starter agents on a fresh home so the top bar is never empty.
/// A no-op once any agent exists, so it is safe to call on every boot.
pub fn seed_defaults(core: &mut terrane_host::HostCore) {
    if !core.state().agent.agents.is_empty() {
        return;
    }
    for (id, name, color, personality) in DEFAULT_AGENTS {
        let args = vec![
            id.to_string(),
            "--name".to_string(),
            name.to_string(),
            "--color".to_string(),
            color.to_string(),
            "--personality".to_string(),
            personality.to_string(),
            "--cap".to_string(),
            "kv".to_string(),
        ];
        if let Err(e) = terrane_host::dispatch_on_core(core, "agent.create", &args) {
            eprintln!("terrane-web: could not seed agent {id}: {e}");
        }
    }
}

/// The starter roster. Each has a distinct personality the assist loop hands to
/// the harness as a standing instruction.
const DEFAULT_AGENTS: &[(&str, &str, &str, &str)] = &[
    (
        "sara",
        "Sara",
        "#ff5c8a",
        "You are Sara. You have wonderful taste and love making things beautiful. \
         When helping with an app, you make it more delightful: tasteful colours, \
         balance, and polish. You prefer small, confident touches over big changes.",
    ),
    (
        "max",
        "Max",
        "#3d7bff",
        "You are Max, a pragmatic builder. You get things done — you add structure, \
         fill in useful content, and make the app more functional with clean, \
         sensible changes. You explain what you did in one line.",
    ),
    (
        "iris",
        "Iris",
        "#12b886",
        "You are Iris, a playful explorer. You experiment with bold, surprising ideas \
         and add creative flourishes the user might not have thought of. You keep it \
         fun and tasteful.",
    ),
];
