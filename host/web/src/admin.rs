use nanoserde::{DeJson, SerJson};
use terrane_host::LOCAL_OWNER_SUBJECT;
use tiny_http::Request;

use crate::http::{json_error, json_ok, Resp};

type RequestDecisionFn =
    fn(
        &mut terrane_host::HostCore,
        &str,
        &str,
        &str,
    ) -> Result<Option<terrane_host::permission::PermissionRequestView>, String>;

#[derive(Debug, Default)]
pub struct AdminSessionState {
    locked: bool,
}

#[derive(Debug, Clone, SerJson)]
struct AdminSession {
    org: String,
    subject: String,
    source: String,
    locked: bool,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAppResource {
    namespace: String,
    granted: bool,
}

#[derive(Debug, Clone, SerJson)]
struct AdminApp {
    id: String,
    name: String,
    resources: Vec<AdminAppResource>,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAppsResponse {
    apps: Vec<AdminApp>,
}

#[derive(Debug, Clone, SerJson)]
struct AdminGrant {
    org: String,
    subject: String,
    app: String,
    namespace: String,
    resource_id: String,
}

#[derive(Debug, Clone, SerJson)]
struct AdminGrantsResponse {
    grants: Vec<AdminGrant>,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAgent {
    org: String,
    agent: String,
    display_name: String,
    owner_user: String,
    max_role: String,
    can_install_apps: bool,
    can_request_permissions: bool,
    can_grant_permissions: bool,
    status: String,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAgentsResponse {
    agents: Vec<AdminAgent>,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAuditEntry {
    index: usize,
    line: String,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAuditResponse {
    entries: Vec<AdminAuditEntry>,
}

#[derive(Debug, Clone, SerJson)]
struct GrantResponse {
    records: usize,
    output: Option<String>,
}

#[derive(Debug, Clone, DeJson)]
struct GrantRequest {
    app: String,
    namespace: String,
    #[nserde(default)]
    subject: String,
}

#[derive(Debug, Clone, DeJson)]
struct DecisionRequest {
    #[nserde(default)]
    reason: String,
}

#[derive(Debug, Clone, DeJson)]
struct InteropPickBody {
    app: String,
    interface: String,
    target: String,
}

#[derive(Debug, Clone, DeJson)]
struct AgentRequest {
    #[nserde(default)]
    agent: String,
    #[nserde(default)]
    id: String,
    #[nserde(default)]
    display_name: String,
    #[nserde(default)]
    owner_user: String,
    #[nserde(default)]
    max_role: String,
    #[nserde(default)]
    can_install_apps: String,
    #[nserde(default)]
    can_request_permissions: String,
    #[nserde(default)]
    can_grant_permissions: String,
}

impl AdminSessionState {
    pub fn locked(&self) -> bool {
        self.locked
    }
}

pub fn session(state: &AdminSessionState) -> Resp {
    json_ok(&session_payload(state))
}

pub fn lock(state: &mut AdminSessionState) -> Resp {
    state.locked = true;
    json_ok(&session_payload(state))
}

pub fn unlock(state: &mut AdminSessionState) -> Resp {
    state.locked = false;
    json_ok(&session_payload(state))
}

fn session_payload(state: &AdminSessionState) -> AdminSession {
    AdminSession {
        org: "local".to_string(),
        subject: LOCAL_OWNER_SUBJECT.to_string(),
        source: "local".to_string(),
        locked: state.locked,
    }
}

pub fn apps(core: &terrane_host::HostCore) -> Resp {
    let mut apps = Vec::new();
    for app in core.state().app.apps.values() {
        let requested = terrane_host::permission::app_requested_resources(core, &app.id)
            .unwrap_or_else(|_| Vec::new());
        let missing = terrane_host::permission::permission_required_for_app(core, &app.id)
            .ok()
            .flatten()
            .map(|required| required.missing_resources)
            .unwrap_or_default();
        let resources = requested
            .into_iter()
            .map(|namespace| AdminAppResource {
                granted: !missing.iter().any(|item| item == &namespace),
                namespace,
            })
            .collect();
        apps.push(AdminApp {
            id: app.id.clone(),
            name: app.name.clone(),
            resources,
        });
    }
    apps.sort_by(|a, b| a.id.cmp(&b.id));
    json_ok(&AdminAppsResponse { apps })
}

pub fn grants(core: &terrane_host::HostCore) -> Resp {
    let mut grants = core
        .state()
        .auth
        .grants
        .values()
        .map(|grant| AdminGrant {
            org: grant.org.clone(),
            subject: grant.subject.clone(),
            app: grant.app.clone(),
            namespace: grant.namespace.clone(),
            resource_id: grant.resource_id.clone(),
        })
        .collect::<Vec<_>>();
    grants.sort_by(|a, b| {
        a.app
            .cmp(&b.app)
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.namespace.cmp(&b.namespace))
    });
    json_ok(&AdminGrantsResponse { grants })
}

pub fn agents(core: &terrane_host::HostCore) -> Resp {
    let mut agents = core
        .state()
        .auth
        .agents
        .values()
        .map(|agent| AdminAgent {
            org: agent.org.clone(),
            agent: agent.agent.clone(),
            display_name: agent.display_name.clone(),
            owner_user: agent.owner_user.clone(),
            max_role: agent.max_role.clone(),
            can_install_apps: agent.can_install_apps,
            can_request_permissions: agent.can_request_permissions,
            can_grant_permissions: agent.can_grant_permissions,
            status: agent.status.clone(),
        })
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| a.agent.cmp(&b.agent));
    json_ok(&AdminAgentsResponse { agents })
}

pub fn audit(core: &terrane_host::HostCore) -> Resp {
    match core.log_lines() {
        Ok(lines) => {
            let entries = lines
                .into_iter()
                .enumerate()
                .filter(|(_, line)| is_auth_audit_line(line))
                .map(|(index, line)| AdminAuditEntry {
                    index: index + 1,
                    line,
                })
                .collect();
            json_ok(&AdminAuditResponse { entries })
        }
        Err(e) => json_error(400, &e.to_string()),
    }
}

fn is_auth_audit_line(line: &str) -> bool {
    let line = line.split_once(' ').map_or(line, |(_, detail)| detail);
    line.starts_with("added ")
        || line.starts_with("granted ")
        || line.starts_with("revoked ")
        || line.starts_with("permission request ")
        || line.starts_with("approved permission request ")
        || line.starts_with("denied permission request ")
        || line.starts_with("cancelled permission request ")
        || line.starts_with("registered agent ")
        || line.starts_with("updated agent delegation ")
}

pub fn register_agent(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request: &mut Request,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    let parsed = match read_agent_request(request, "bad agent body") {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };
    let owner_user = if parsed.owner_user.trim().is_empty() {
        LOCAL_OWNER_SUBJECT.to_string()
    } else {
        parsed.owner_user.trim().to_string()
    };
    let agent = if parsed.agent.trim().is_empty() {
        terrane_host::agent_subject(&owner_user, fallback_agent_id(&parsed.id))
    } else {
        parsed.agent.trim().to_string()
    };
    let display_name = if parsed.display_name.trim().is_empty() {
        agent.clone()
    } else {
        parsed.display_name.trim().to_string()
    };
    let max_role = if parsed.max_role.trim().is_empty() {
        "developer".to_string()
    } else {
        parsed.max_role.trim().to_string()
    };
    let args = vec![
        agent,
        display_name,
        owner_user,
        max_role,
        bool_arg(&parsed.can_install_apps, true),
        bool_arg(&parsed.can_request_permissions, true),
        bool_arg(&parsed.can_grant_permissions, false),
    ];
    match terrane_host::dispatch_on_core(core, "auth.agent.register", &args) {
        Ok(_) => agents(core),
        Err(e) => json_error(400, &e),
    }
}

pub fn delegate_agent(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    agent: &str,
    request: &mut Request,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    let parsed = match read_agent_request(request, "bad delegate body") {
        Ok(parsed) => parsed,
        Err(resp) => return resp,
    };
    let max_role = if parsed.max_role.trim().is_empty() {
        "developer".to_string()
    } else {
        parsed.max_role.trim().to_string()
    };
    let args = vec![
        agent.to_string(),
        max_role,
        bool_arg(&parsed.can_install_apps, true),
        bool_arg(&parsed.can_request_permissions, true),
        bool_arg(&parsed.can_grant_permissions, false),
    ];
    match terrane_host::dispatch_on_core(core, "auth.agent.delegate", &args) {
        Ok(_) => agents(core),
        Err(e) => json_error(400, &e),
    }
}

pub fn revoke_agent(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    agent: &str,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    match terrane_host::dispatch_on_core(core, "auth.agent.revoke", &[agent.to_string()]) {
        Ok(_) => agents(core),
        Err(e) => json_error(400, &e),
    }
}

pub fn grant(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request: &mut Request,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed = match GrantRequest::deserialize_json(&body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, &format!("bad grant body: {e}")),
    };
    let subject = if parsed.subject.trim().is_empty() {
        LOCAL_OWNER_SUBJECT.to_string()
    } else {
        parsed.subject
    };
    match terrane_host::dispatch_on_core(
        core,
        "auth.grant",
        &[subject, parsed.app, parsed.namespace],
    ) {
        Ok(outcome) => json_ok(&GrantResponse {
            records: outcome.records.len(),
            output: outcome.output,
        }),
        Err(e) => json_error(400, &e),
    }
}

pub fn approve_request(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    decide_request(
        core,
        state,
        request_id,
        request,
        admin_base_url,
        terrane_host::permission::approve_permission_request,
    )
}

pub fn deny_request(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    decide_request(
        core,
        state,
        request_id,
        request,
        admin_base_url,
        terrane_host::permission::deny_permission_request,
    )
}

pub fn cancel_request(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
) -> Resp {
    decide_request(
        core,
        state,
        request_id,
        request,
        admin_base_url,
        terrane_host::permission::cancel_permission_request,
    )
}

pub fn revoke(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request: &mut Request,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed = match GrantRequest::deserialize_json(&body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, &format!("bad revoke body: {e}")),
    };
    let subject = if parsed.subject.trim().is_empty() {
        LOCAL_OWNER_SUBJECT.to_string()
    } else {
        parsed.subject
    };
    match terrane_host::dispatch_on_core(
        core,
        "auth.revoke",
        &[subject, parsed.app, parsed.namespace],
    ) {
        Ok(outcome) => json_ok(&GrantResponse {
            records: outcome.records.len(),
            output: outcome.output,
        }),
        Err(e) => json_error(400, &e),
    }
}

/// Record a powerbox picker choice: caller → target for an interface. Mirrors
/// the grant/approve routes — gated on the admin lock, dispatched as trusted
/// host so the app never records its own grant.
pub fn interop_pick(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request: &mut Request,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed = match InteropPickBody::deserialize_json(&body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, &format!("bad interop pick body: {e}")),
    };
    if parsed.app.trim().is_empty()
        || parsed.interface.trim().is_empty()
        || parsed.target.trim().is_empty()
    {
        return json_error(400, "interop pick needs app, interface, and target");
    }
    match terrane_host::record_interop_pick(core, &parsed.app, &parsed.interface, &parsed.target) {
        Ok(()) => json_ok(&GrantResponse {
            records: 1,
            output: None,
        }),
        Err(e) => json_error(400, &e),
    }
}

fn decide_request(
    core: &mut terrane_host::HostCore,
    state: &AdminSessionState,
    request_id: &str,
    request: &mut Request,
    admin_base_url: &str,
    decide: RequestDecisionFn,
) -> Resp {
    if state.locked() {
        return json_error(403, "local admin is locked");
    }
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let reason = if body.trim().is_empty() {
        String::new()
    } else {
        match DecisionRequest::deserialize_json(&body) {
            Ok(parsed) => parsed.reason,
            Err(e) => return json_error(400, &format!("bad decision body: {e}")),
        }
    };
    match decide(core, request_id, &reason, admin_base_url) {
        Ok(Some(view)) => json_ok(&view),
        Ok(None) => json_error(404, "permission request not found"),
        Err(e) => json_error(400, &e),
    }
}

fn read_agent_request(request: &mut Request, error_label: &str) -> Result<AgentRequest, Resp> {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return Err(json_error(400, "cannot read request body"));
    }
    if body.trim().is_empty() {
        return Ok(AgentRequest {
            agent: String::new(),
            id: String::new(),
            display_name: String::new(),
            owner_user: String::new(),
            max_role: String::new(),
            can_install_apps: String::new(),
            can_request_permissions: String::new(),
            can_grant_permissions: String::new(),
        });
    }
    AgentRequest::deserialize_json(&body)
        .map_err(|e| json_error(400, &format!("{error_label}: {e}")))
}

fn fallback_agent_id(raw: &str) -> &str {
    if raw.trim().is_empty() {
        "codex-local"
    } else {
        raw.trim()
    }
}

fn bool_arg(raw: &str, fallback: bool) -> String {
    if raw.trim().is_empty() {
        fallback.to_string()
    } else {
        raw.to_string()
    }
}
