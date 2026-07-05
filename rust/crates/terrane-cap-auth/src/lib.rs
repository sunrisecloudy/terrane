//! The `auth` capability owns durable authorization facts and folded AuthState.

use std::collections::BTreeMap;
use std::path::Path;

use borsh::{BorshDeserialize, BorshSerialize};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    CapBus, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern,
    EventRecord, EventSpec, ExecutionPrincipal, GrantResourceSpec, ReadValue, ResourceReadCtx,
    Result, StateStore, LOCAL_OWNER_SUBJECT, LOCAL_SOURCE, NAMESPACE_SELECTOR_SCHEMA_ID,
};
use terrane_cap_kv::KvStorageBinding;

mod doc;
#[cfg(test)]
mod tests;

pub const AUTH_PROJECTION_APP_ID: &str = "__terrane/auth";
pub const AUTH_PROJECTION_KEY_PREFIX: &str = "__terrane/auth/v1";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthState {
    pub members: BTreeMap<String, AuthMember>,
    pub grants: BTreeMap<String, AuthGrant>,
    pub permission_requests: BTreeMap<String, AuthPermissionRequest>,
    pub agents: BTreeMap<String, AuthAgent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMember {
    pub org: String,
    pub subject: String,
    pub role: String,
    pub status: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthGrant {
    pub org: String,
    pub subject: String,
    pub app: String,
    pub namespace: String,
    pub selector_schema_id: String,
    pub selector_id: String,
    pub selector_json: String,
    pub resource_id: String,
    pub verbs: Vec<String>,
    pub granted_by: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AuthPermissionResource {
    pub namespace: String,
    pub selector_schema_id: String,
    pub selector_id: String,
    pub selector_json: String,
    pub resource_id: String,
    pub verbs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthPermissionRequest {
    pub request_id: String,
    pub org: String,
    pub subject: String,
    pub app: String,
    pub app_name: String,
    pub operation: String,
    pub source: String,
    pub resume_token_hash: String,
    pub resources: Vec<AuthPermissionResource>,
    pub status: String,
    pub decided_by: String,
    pub decision_reason: String,
    pub decision_source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthAgent {
    pub org: String,
    pub agent: String,
    pub display_name: String,
    pub owner_user: String,
    pub max_role: String,
    pub can_install_apps: bool,
    pub can_request_permissions: bool,
    pub can_grant_permissions: bool,
    pub status: String,
    pub source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Granted {
    org: String,
    subject: String,
    app: String,
    namespace: String,
    selector_schema_id: String,
    selector_id: String,
    selector_json: String,
    resource_id: String,
    verbs: Vec<String>,
    granted_by: String,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Revoked {
    org: String,
    subject: String,
    app: String,
    resource_id: String,
    revoked_by: String,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct PermissionRequested {
    request_id: String,
    org: String,
    subject: String,
    app: String,
    app_name: String,
    operation: String,
    source: String,
    resume_token_hash: String,
    resources: Vec<AuthPermissionResource>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct PermissionRequestedV1 {
    request_id: String,
    org: String,
    subject: String,
    app: String,
    operation: String,
    source: String,
    resources: Vec<AuthPermissionResource>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct PermissionDecision {
    request_id: String,
    decided_by: String,
    reason: String,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct MemberAdded {
    org: String,
    subject: String,
    role: String,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct AgentRegistered {
    org: String,
    agent: String,
    display_name: String,
    owner_user: String,
    max_role: String,
    can_install_apps: bool,
    can_request_permissions: bool,
    can_grant_permissions: bool,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct AgentDelegated {
    org: String,
    agent: String,
    max_role: String,
    can_install_apps: bool,
    can_request_permissions: bool,
    can_grant_permissions: bool,
    source: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct AgentRevoked {
    org: String,
    agent: String,
    source: String,
}

pub struct AuthCapability;

impl Capability for AuthCapability {
    fn namespace(&self) -> &'static str {
        "auth"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "auth.member.ensure-local-owner",
                },
                CommandSpec { name: "auth.grant" },
                CommandSpec {
                    name: "auth.revoke",
                },
                CommandSpec {
                    name: "auth.permission.request",
                },
                CommandSpec {
                    name: "auth.permission.approve",
                },
                CommandSpec {
                    name: "auth.permission.deny",
                },
                CommandSpec {
                    name: "auth.permission.cancel",
                },
                CommandSpec {
                    name: "auth.agent.register",
                },
                CommandSpec {
                    name: "auth.agent.delegate",
                },
                CommandSpec {
                    name: "auth.agent.revoke",
                },
            ],
            events: vec![
                EventSpec {
                    kind: "auth.member.added",
                },
                EventSpec {
                    kind: "auth.granted",
                },
                EventSpec {
                    kind: "auth.revoked",
                },
                EventSpec {
                    kind: "auth.permission.requested",
                },
                EventSpec {
                    kind: "auth.permission.approved",
                },
                EventSpec {
                    kind: "auth.permission.denied",
                },
                EventSpec {
                    kind: "auth.permission.cancelled",
                },
                EventSpec {
                    kind: "auth.agent.registered",
                },
                EventSpec {
                    kind: "auth.agent.delegated",
                },
                EventSpec {
                    kind: "auth.agent.revoked",
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
        doc::auth_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "auth.member.ensure-local-owner" => decide_member_ensure_local_owner(ctx, args),
            "auth.grant" => decide_grant(ctx, args),
            "auth.revoke" => decide_revoke(ctx, args),
            "auth.permission.request" => decide_permission_request(ctx, args),
            "auth.permission.approve" => decide_permission_approve(ctx, args),
            "auth.permission.deny" => decide_permission_decision(ctx, args, "denied"),
            "auth.permission.cancel" => decide_permission_decision(ctx, args, "cancelled"),
            "auth.agent.register" => decide_agent_register(ctx, args),
            "auth.agent.delegate" => decide_agent_delegate(ctx, args),
            "auth.agent.revoke" => decide_agent_revoke(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        match record.kind.as_str() {
            "auth.member.added" => decode_event::<MemberAdded>(record)
                .ok()
                .map(|e| format!("added {} as {} member in org {}", e.subject, e.role, e.org)),
            "auth.granted" => decode_event::<Granted>(record).ok().map(|e| {
                format!(
                    "granted {} access to {} for app {}",
                    e.subject, e.resource_id, e.app
                )
            }),
            "auth.revoked" => decode_event::<Revoked>(record).ok().map(|e| {
                format!(
                    "revoked {} access to {} for app {}",
                    e.subject, e.resource_id, e.app
                )
            }),
            "auth.permission.requested" => decode_permission_requested(record)
                .ok()
                .map(|e| format!("permission request {} for app {}", e.request_id, e.app_name)),
            "auth.permission.approved" => decode_event::<PermissionDecision>(record)
                .ok()
                .map(|e| format!("approved permission request {}", e.request_id)),
            "auth.permission.denied" => decode_event::<PermissionDecision>(record)
                .ok()
                .map(|e| format!("denied permission request {}", e.request_id)),
            "auth.permission.cancelled" => decode_event::<PermissionDecision>(record)
                .ok()
                .map(|e| format!("cancelled permission request {}", e.request_id)),
            "auth.agent.registered" => decode_event::<AgentRegistered>(record)
                .ok()
                .map(|e| format!("registered agent {}", e.agent)),
            "auth.agent.delegated" => decode_event::<AgentDelegated>(record)
                .ok()
                .map(|e| format!("updated agent delegation {}", e.agent)),
            "auth.agent.revoked" => decode_event::<AgentRevoked>(record)
                .ok()
                .map(|e| format!("revoked agent {}", e.agent)),
            _ => None,
        }
    }

    fn read_resource(
        &self,
        _ctx: ResourceReadCtx<'_>,
        name: &str,
        _args: &[String],
    ) -> Result<ReadValue> {
        Err(Error::InvalidInput(format!(
            "auth has no public resource API: {name}"
        )))
    }
}

fn decide_grant(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let subject = non_empty(arg(args, 0, "subject")?, "subject")?;
    let app = non_empty(arg(args, 1, "app")?, "app")?;
    let raw_resource = non_empty(arg(args, 2, "namespace")?, "namespace")?;
    let grant_target = parse_grant_target(&raw_resource)?;
    let namespace = grant_target.namespace.clone();
    ensure_app_exists(ctx.bus, &app)?;
    validate_segment_input("subject", &subject)?;
    validate_segment_input("app", &app)?;
    validate_segment_input("namespace", &raw_resource)?;
    let spec = namespace_v1_spec(ctx.bus, &namespace)?;

    let verbs = match args.get(3) {
        Some(raw) => parse_grant_verbs(raw, spec.verbs)?,
        None => spec.verbs.iter().map(|verb| (*verb).to_string()).collect(),
    };
    ensure_grantable_subject(ctx.state, &subject)?;
    let resource_id = grant_target.resource_id;
    let key = grant_key(
        terrane_cap_interface::LOCAL_ORG,
        &subject,
        &app,
        &resource_id,
    );
    if state_ref::<AuthState>(ctx.state, "auth")?
        .grants
        .contains_key(&key)
    {
        return Ok(Decision::Commit(Vec::new()));
    }

    Ok(Decision::Commit(vec![granted_event(Granted {
        org: terrane_cap_interface::LOCAL_ORG.to_string(),
        subject,
        app,
        namespace: namespace.clone(),
        selector_schema_id: spec.selector_schema_id.to_string(),
        selector_id: grant_target.selector_id,
        selector_json: grant_target.selector_json,
        resource_id,
        verbs,
        granted_by: LOCAL_OWNER_SUBJECT.to_string(),
        source: LOCAL_SOURCE.to_string(),
    })?]))
}

fn decide_member_ensure_local_owner(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    if !args.is_empty() {
        return Err(Error::InvalidInput(format!(
            "auth.member.ensure-local-owner takes no arguments, got {}",
            args.len()
        )));
    }
    let key = member_key(terrane_cap_interface::LOCAL_ORG, LOCAL_OWNER_SUBJECT);
    if state_ref::<AuthState>(ctx.state, "auth")?
        .members
        .contains_key(&key)
    {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![member_added_event(MemberAdded {
        org: terrane_cap_interface::LOCAL_ORG.to_string(),
        subject: LOCAL_OWNER_SUBJECT.to_string(),
        role: "owner".to_string(),
        source: LOCAL_SOURCE.to_string(),
    })?]))
}

fn decide_revoke(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let subject = non_empty(arg(args, 0, "subject")?, "subject")?;
    let app = non_empty(arg(args, 1, "app")?, "app")?;
    let raw_resource = non_empty(arg(args, 2, "namespace")?, "namespace")?;
    let grant_target = parse_grant_target(&raw_resource)?;
    ensure_app_exists(ctx.bus, &app)?;
    validate_segment_input("subject", &subject)?;
    validate_segment_input("app", &app)?;
    validate_segment_input("namespace", &raw_resource)?;
    namespace_v1_spec(ctx.bus, &grant_target.namespace)?;

    let resource_id = grant_target.resource_id;
    let key = grant_key(
        terrane_cap_interface::LOCAL_ORG,
        &subject,
        &app,
        &resource_id,
    );
    if !state_ref::<AuthState>(ctx.state, "auth")?
        .grants
        .contains_key(&key)
    {
        return Ok(Decision::Commit(Vec::new()));
    }

    Ok(Decision::Commit(vec![revoked_event(Revoked {
        org: terrane_cap_interface::LOCAL_ORG.to_string(),
        subject,
        app,
        resource_id,
        revoked_by: LOCAL_OWNER_SUBJECT.to_string(),
        source: LOCAL_SOURCE.to_string(),
    })?]))
}

fn decide_permission_request(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let request_id = non_empty(arg(args, 0, "request_id")?, "request_id")?;
    let subject = non_empty(arg(args, 1, "subject")?, "subject")?;
    let app = non_empty(arg(args, 2, "app")?, "app")?;
    let operation = non_empty(arg(args, 3, "operation")?, "operation")?;
    let source = non_empty(arg(args, 4, "source")?, "source")?;
    let namespaces = parse_namespaces(&arg(args, 5, "resources")?)?;
    let app_name = non_empty_or_arg(args, 6, &app);
    let resume_token_hash = args.get(7).cloned().unwrap_or_default();
    ensure_app_exists(ctx.bus, &app)?;
    validate_segment_input("request_id", &request_id)?;
    validate_segment_input("subject", &subject)?;
    validate_segment_input("app", &app)?;
    validate_segment_input("app_name", &app_name)?;
    validate_segment_input("operation", &operation)?;
    validate_segment_input("source", &source)?;
    validate_segment_input("resume_token_hash", &resume_token_hash)?;
    if state_ref::<AuthState>(ctx.state, "auth")?
        .permission_requests
        .contains_key(&request_id)
    {
        return Ok(Decision::Commit(Vec::new()));
    }

    let resources = namespaces
        .into_iter()
        .map(|namespace| permission_resource(ctx.bus, namespace))
        .collect::<Result<Vec<_>>>()?;
    Ok(Decision::Commit(vec![permission_requested_event(
        PermissionRequested {
            request_id,
            org: terrane_cap_interface::LOCAL_ORG.to_string(),
            subject,
            app,
            app_name,
            operation,
            source,
            resume_token_hash,
            resources,
        },
    )?]))
}

fn decide_permission_approve(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let request_id = non_empty(arg(args, 0, "request_id")?, "request_id")?;
    let reason = args.get(1).cloned().unwrap_or_default();
    let request = match state_ref::<AuthState>(ctx.state, "auth")?
        .permission_requests
        .get(&request_id)
    {
        Some(request) => request.clone(),
        None => {
            return Err(Error::InvalidInput(format!(
                "unknown permission request: {request_id}"
            )))
        }
    };
    if request.status == "approved" {
        return Ok(Decision::Commit(Vec::new()));
    }
    if request.status != "pending" {
        return Err(Error::InvalidInput(format!(
            "permission request {request_id} is {}",
            request.status
        )));
    }

    let mut records = Vec::new();
    let grants = &state_ref::<AuthState>(ctx.state, "auth")?.grants;
    for resource in &request.resources {
        let key = grant_key(
            &request.org,
            &request.subject,
            &request.app,
            &resource.resource_id,
        );
        if grants.contains_key(&key) {
            continue;
        }
        records.push(granted_event(Granted {
            org: request.org.clone(),
            subject: request.subject.clone(),
            app: request.app.clone(),
            namespace: resource.namespace.clone(),
            selector_schema_id: resource.selector_schema_id.clone(),
            selector_id: resource.selector_id.clone(),
            selector_json: resource.selector_json.clone(),
            resource_id: resource.resource_id.clone(),
            verbs: resource.verbs.clone(),
            granted_by: LOCAL_OWNER_SUBJECT.to_string(),
            source: LOCAL_SOURCE.to_string(),
        })?);
    }
    records.push(permission_decision_event(
        "auth.permission.approved",
        PermissionDecision {
            request_id,
            decided_by: LOCAL_OWNER_SUBJECT.to_string(),
            reason,
            source: LOCAL_SOURCE.to_string(),
        },
    )?);
    Ok(Decision::Commit(records))
}

fn decide_permission_decision(
    ctx: CommandCtx<'_>,
    args: &[String],
    status: &str,
) -> Result<Decision> {
    let request_id = non_empty(arg(args, 0, "request_id")?, "request_id")?;
    let reason = args.get(1).cloned().unwrap_or_default();
    let request = match state_ref::<AuthState>(ctx.state, "auth")?
        .permission_requests
        .get(&request_id)
    {
        Some(request) => request,
        None => {
            return Err(Error::InvalidInput(format!(
                "unknown permission request: {request_id}"
            )))
        }
    };
    if request.status == status {
        return Ok(Decision::Commit(Vec::new()));
    }
    if request.status != "pending" {
        return Err(Error::InvalidInput(format!(
            "permission request {request_id} is {}",
            request.status
        )));
    }
    let kind = match status {
        "denied" => "auth.permission.denied",
        "cancelled" => "auth.permission.cancelled",
        _ => return Err(Error::InvalidInput(format!("bad request status: {status}"))),
    };
    Ok(Decision::Commit(vec![permission_decision_event(
        kind,
        PermissionDecision {
            request_id,
            decided_by: LOCAL_OWNER_SUBJECT.to_string(),
            reason,
            source: LOCAL_SOURCE.to_string(),
        },
    )?]))
}

fn decide_agent_register(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let agent = non_empty(arg(args, 0, "agent")?, "agent")?;
    let display_name = non_empty_or_arg(args, 1, &agent);
    let owner_user = non_empty_or_arg(args, 2, LOCAL_OWNER_SUBJECT);
    let max_role = non_empty_or_arg(args, 3, "developer");
    let can_install_apps = parse_bool_arg(args, 4, true)?;
    let can_request_permissions = parse_bool_arg(args, 5, true)?;
    let can_grant_permissions = parse_bool_arg(args, 6, false)?;
    validate_agent_id(&agent)?;
    validate_segment_input("owner_user", &owner_user)?;
    validate_segment_input("max_role", &max_role)?;
    if state_ref::<AuthState>(ctx.state, "auth")?
        .agents
        .get(&agent)
        .is_some_and(|record| record.status == "active")
    {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![agent_registered_event(
        AgentRegistered {
            org: terrane_cap_interface::LOCAL_ORG.to_string(),
            agent,
            display_name,
            owner_user,
            max_role,
            can_install_apps,
            can_request_permissions,
            can_grant_permissions,
            source: LOCAL_SOURCE.to_string(),
        },
    )?]))
}

fn decide_agent_delegate(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let agent = non_empty(arg(args, 0, "agent")?, "agent")?;
    let max_role = non_empty_or_arg(args, 1, "developer");
    let can_install_apps = parse_bool_arg(args, 2, true)?;
    let can_request_permissions = parse_bool_arg(args, 3, true)?;
    let can_grant_permissions = parse_bool_arg(args, 4, false)?;
    validate_agent_id(&agent)?;
    validate_segment_input("max_role", &max_role)?;
    let existing = state_ref::<AuthState>(ctx.state, "auth")?
        .agents
        .get(&agent)
        .cloned()
        .ok_or_else(|| Error::InvalidInput(format!("unknown agent: {agent}")))?;
    if existing.status != "active" {
        return Err(Error::InvalidInput(format!("agent {agent} is revoked")));
    }
    if existing.max_role == max_role
        && existing.can_install_apps == can_install_apps
        && existing.can_request_permissions == can_request_permissions
        && existing.can_grant_permissions == can_grant_permissions
    {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![agent_delegated_event(
        AgentDelegated {
            org: terrane_cap_interface::LOCAL_ORG.to_string(),
            agent,
            max_role,
            can_install_apps,
            can_request_permissions,
            can_grant_permissions,
            source: LOCAL_SOURCE.to_string(),
        },
    )?]))
}

fn decide_agent_revoke(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let agent = non_empty(arg(args, 0, "agent")?, "agent")?;
    validate_agent_id(&agent)?;
    let Some(existing) = state_ref::<AuthState>(ctx.state, "auth")?
        .agents
        .get(&agent)
    else {
        return Ok(Decision::Commit(Vec::new()));
    };
    if existing.status == "revoked" {
        return Ok(Decision::Commit(Vec::new()));
    }
    Ok(Decision::Commit(vec![agent_revoked_event(AgentRevoked {
        org: terrane_cap_interface::LOCAL_ORG.to_string(),
        agent,
        source: LOCAL_SOURCE.to_string(),
    })?]))
}

fn granted_event(event: Granted) -> Result<EventRecord> {
    encode_event("auth.granted", &event)
}

pub fn granted_namespace_event(
    principal: &ExecutionPrincipal,
    app: &str,
    namespace: &str,
    source: &str,
) -> Result<EventRecord> {
    granted_event(Granted {
        org: principal.org.clone(),
        subject: principal.subject.clone(),
        app: app.to_string(),
        namespace: namespace.to_string(),
        selector_schema_id: NAMESPACE_SELECTOR_SCHEMA_ID.to_string(),
        selector_id: String::new(),
        selector_json: format!(r#"{{"namespace":"{}"}}"#, json_string(namespace)),
        resource_id: namespace_resource_id(namespace),
        verbs: vec!["call".to_string(), "read".to_string(), "write".to_string()],
        granted_by: LOCAL_OWNER_SUBJECT.to_string(),
        source: source.to_string(),
    })
}

fn member_added_event(event: MemberAdded) -> Result<EventRecord> {
    encode_event("auth.member.added", &event)
}

fn revoked_event(event: Revoked) -> Result<EventRecord> {
    encode_event("auth.revoked", &event)
}

fn permission_requested_event(event: PermissionRequested) -> Result<EventRecord> {
    encode_event("auth.permission.requested", &event)
}

fn permission_decision_event(kind: &str, event: PermissionDecision) -> Result<EventRecord> {
    encode_event(kind, &event)
}

fn agent_registered_event(event: AgentRegistered) -> Result<EventRecord> {
    encode_event("auth.agent.registered", &event)
}

fn agent_delegated_event(event: AgentDelegated) -> Result<EventRecord> {
    encode_event("auth.agent.delegated", &event)
}

fn agent_revoked_event(event: AgentRevoked) -> Result<EventRecord> {
    encode_event("auth.agent.revoked", &event)
}

fn decode_permission_requested(record: &EventRecord) -> Result<PermissionRequested> {
    match decode_event::<PermissionRequested>(record) {
        Ok(event) => Ok(event),
        Err(_) => {
            let old: PermissionRequestedV1 = decode_event(record)?;
            Ok(PermissionRequested {
                app_name: old.app.clone(),
                resume_token_hash: String::new(),
                request_id: old.request_id,
                org: old.org,
                subject: old.subject,
                app: old.app,
                operation: old.operation,
                source: old.source,
                resources: old.resources,
            })
        }
    }
}

fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    match record.kind.as_str() {
        "auth.member.added" => {
            let event: MemberAdded = decode_event(record)?;
            let key = member_key(&event.org, &event.subject);
            state_mut::<AuthState>(state, "auth")?.members.insert(
                key,
                AuthMember {
                    org: event.org,
                    subject: event.subject,
                    role: event.role,
                    status: "active".to_string(),
                    source: event.source,
                },
            );
        }
        "auth.granted" => {
            let event: Granted = decode_event(record)?;
            let key = grant_key(&event.org, &event.subject, &event.app, &event.resource_id);
            state_mut::<AuthState>(state, "auth")?.grants.insert(
                key,
                AuthGrant {
                    org: event.org,
                    subject: event.subject,
                    app: event.app,
                    namespace: event.namespace,
                    selector_schema_id: event.selector_schema_id,
                    selector_id: event.selector_id,
                    selector_json: event.selector_json,
                    resource_id: event.resource_id,
                    verbs: event.verbs,
                    granted_by: event.granted_by,
                    source: event.source,
                },
            );
        }
        "auth.revoked" => {
            let event: Revoked = decode_event(record)?;
            let key = grant_key(&event.org, &event.subject, &event.app, &event.resource_id);
            state_mut::<AuthState>(state, "auth")?.grants.remove(&key);
        }
        "auth.permission.requested" => {
            let event = decode_permission_requested(record)?;
            let key = event.request_id.clone();
            state_mut::<AuthState>(state, "auth")?
                .permission_requests
                .insert(
                    key,
                    AuthPermissionRequest {
                        request_id: event.request_id,
                        org: event.org,
                        subject: event.subject,
                        app: event.app,
                        app_name: event.app_name,
                        operation: event.operation,
                        source: event.source,
                        resume_token_hash: event.resume_token_hash,
                        resources: event.resources,
                        status: "pending".to_string(),
                        decided_by: String::new(),
                        decision_reason: String::new(),
                        decision_source: String::new(),
                    },
                );
        }
        "auth.permission.approved" => {
            let event: PermissionDecision = decode_event(record)?;
            set_permission_status(state, event, "approved")?;
        }
        "auth.permission.denied" => {
            let event: PermissionDecision = decode_event(record)?;
            set_permission_status(state, event, "denied")?;
        }
        "auth.permission.cancelled" => {
            let event: PermissionDecision = decode_event(record)?;
            set_permission_status(state, event, "cancelled")?;
        }
        "auth.agent.registered" => {
            let event: AgentRegistered = decode_event(record)?;
            state_mut::<AuthState>(state, "auth")?.agents.insert(
                event.agent.clone(),
                AuthAgent {
                    org: event.org,
                    agent: event.agent,
                    display_name: event.display_name,
                    owner_user: event.owner_user,
                    max_role: event.max_role,
                    can_install_apps: event.can_install_apps,
                    can_request_permissions: event.can_request_permissions,
                    can_grant_permissions: event.can_grant_permissions,
                    status: "active".to_string(),
                    source: event.source,
                },
            );
        }
        "auth.agent.delegated" => {
            let event: AgentDelegated = decode_event(record)?;
            if let Some(agent) = state_mut::<AuthState>(state, "auth")?
                .agents
                .get_mut(&event.agent)
            {
                agent.max_role = event.max_role;
                agent.can_install_apps = event.can_install_apps;
                agent.can_request_permissions = event.can_request_permissions;
                agent.can_grant_permissions = event.can_grant_permissions;
                agent.source = event.source;
            }
        }
        "auth.agent.revoked" => {
            let event: AgentRevoked = decode_event(record)?;
            let auth = state_mut::<AuthState>(state, "auth")?;
            if let Some(agent) = auth.agents.get_mut(&event.agent) {
                agent.status = "revoked".to_string();
                agent.source = event.source;
            }
            auth.grants.retain(|_, grant| grant.subject != event.agent);
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            let auth = state_mut::<AuthState>(state, "auth")?;
            auth.grants.retain(|_, grant| grant.app != event.id);
            auth.permission_requests
                .retain(|_, request| request.app != event.id);
        }
        _ => {}
    }
    Ok(())
}

fn set_permission_status(
    state: &mut dyn StateStore,
    event: PermissionDecision,
    status: &str,
) -> Result<()> {
    if let Some(request) = state_mut::<AuthState>(state, "auth")?
        .permission_requests
        .get_mut(&event.request_id)
    {
        request.status = status.to_string();
        request.decided_by = event.decided_by;
        request.decision_reason = event.reason;
        request.decision_source = event.source;
    }
    Ok(())
}

pub fn namespace_granted(
    state: &dyn StateStore,
    principal: &ExecutionPrincipal,
    app: &str,
    namespace: &str,
) -> Result<bool> {
    if principal.subject.starts_with("agent:") && !agent_is_active(state, &principal.subject)? {
        return Ok(false);
    }
    let resource_id = namespace_resource_id(namespace);
    let key = grant_key(&principal.org, &principal.subject, app, &resource_id);
    Ok(state_ref::<AuthState>(state, "auth")?
        .grants
        .contains_key(&key))
}

pub fn resource_granted(
    state: &dyn StateStore,
    principal: &ExecutionPrincipal,
    app: &str,
    resource_id: &str,
) -> Result<bool> {
    if principal.subject.starts_with("agent:") && !agent_is_active(state, &principal.subject)? {
        return Ok(false);
    }
    let key = grant_key(&principal.org, &principal.subject, app, resource_id);
    Ok(state_ref::<AuthState>(state, "auth")?
        .grants
        .contains_key(&key))
}

pub fn any_resource_granted_in_namespace(
    state: &dyn StateStore,
    principal: &ExecutionPrincipal,
    app: &str,
    namespace: &str,
) -> Result<bool> {
    if principal.subject.starts_with("agent:") && !agent_is_active(state, &principal.subject)? {
        return Ok(false);
    }
    Ok(state_ref::<AuthState>(state, "auth")?.grants.values().any(|grant| {
        grant.org == principal.org
            && grant.subject == principal.subject
            && grant.app == app
            && grant.namespace == namespace
    }))
}

pub fn namespace_resource_id(namespace: &str) -> String {
    namespace.to_string()
}

pub fn permission_request(
    state: &dyn StateStore,
    request_id: &str,
) -> Result<Option<AuthPermissionRequest>> {
    Ok(state_ref::<AuthState>(state, "auth")?
        .permission_requests
        .get(request_id)
        .cloned())
}

pub fn permission_requests(state: &dyn StateStore) -> Result<Vec<AuthPermissionRequest>> {
    let mut requests = state_ref::<AuthState>(state, "auth")?
        .permission_requests
        .values()
        .cloned()
        .collect::<Vec<_>>();
    requests.sort_by(|a, b| a.request_id.cmp(&b.request_id));
    Ok(requests)
}

pub fn auth_agents(state: &dyn StateStore) -> Result<Vec<AuthAgent>> {
    let mut agents = state_ref::<AuthState>(state, "auth")?
        .agents
        .values()
        .cloned()
        .collect::<Vec<_>>();
    agents.sort_by(|a, b| a.agent.cmp(&b.agent));
    Ok(agents)
}

pub fn auth_members(state: &dyn StateStore) -> Result<Vec<AuthMember>> {
    let mut members = state_ref::<AuthState>(state, "auth")?
        .members
        .values()
        .cloned()
        .collect::<Vec<_>>();
    members.sort_by(|a, b| a.subject.cmp(&b.subject));
    Ok(members)
}

pub fn local_owner_member_exists(state: &dyn StateStore) -> Result<bool> {
    Ok(state_ref::<AuthState>(state, "auth")?
        .members
        .contains_key(&member_key(
            terrane_cap_interface::LOCAL_ORG,
            LOCAL_OWNER_SUBJECT,
        )))
}

pub fn agent_subject(owner_user: &str, agent_id: &str) -> String {
    let owner = owner_user.strip_prefix("user:").unwrap_or(owner_user);
    format!("agent:{owner}:{agent_id}")
}

pub fn member_key(org: &str, subject: &str) -> String {
    format!(
        "orgs/{}/members/{}",
        encode_segment(org),
        encode_segment(subject)
    )
}

pub fn grant_key(org: &str, subject: &str, app: &str, resource_id: &str) -> String {
    format!(
        "orgs/{}/subjects/{}/apps/{}/resources/{}",
        encode_segment(org),
        encode_segment(subject),
        encode_segment(app),
        encode_segment(resource_id)
    )
}

pub fn reserved_projection_entries(state: &AuthState) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for member in state.members.values() {
        out.insert(
            member_projection_key(member),
            member_projection_value(member),
        );
    }
    for agent in state.agents.values() {
        out.insert(
            agent_member_projection_key(agent),
            agent_projection_value(agent),
        );
        out.insert(
            agent_delegation_projection_key(agent),
            agent_projection_value(agent),
        );
    }
    for grant in state.grants.values() {
        let value = grant_projection_value(grant);
        out.insert(grant_projection_key(grant), value.clone());
        out.insert(grant_by_app_projection_key(grant), value);
    }
    for request in state.permission_requests.values() {
        let value = permission_request_projection_value(request);
        out.insert(permission_request_projection_key(request), value.clone());
        out.insert(permission_request_by_app_projection_key(request), value);
    }
    out
}

pub fn sync_reserved_projection(
    home: &Path,
    binding: &KvStorageBinding,
    state: &AuthState,
) -> Result<()> {
    let entries = reserved_projection_entries(state);
    terrane_cap_kv::sync_logical_store(home, binding, AUTH_PROJECTION_APP_ID, Some(&entries))
}

pub fn sync_reserved_projection_after_commit(
    home: &Path,
    before_binding: &KvStorageBinding,
    after_binding: &KvStorageBinding,
    after_state: &AuthState,
) -> Result<()> {
    if before_binding != after_binding {
        terrane_cap_kv::sync_logical_store(home, before_binding, AUTH_PROJECTION_APP_ID, None)?;
    }
    sync_reserved_projection(home, after_binding, after_state)
}

fn member_projection_key(member: &AuthMember) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/members/users/{}",
        encode_segment(&member.org),
        encode_segment(&member.subject)
    )
}

fn member_projection_value(member: &AuthMember) -> String {
    format!(
        r#"{{"org":"{}","subject":"{}","role":"{}","status":"{}","source":"{}"}}"#,
        json_string(&member.org),
        json_string(&member.subject),
        json_string(&member.role),
        json_string(&member.status),
        json_string(&member.source)
    )
}

fn agent_member_projection_key(agent: &AuthAgent) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/members/agents/{}",
        encode_segment(&agent.org),
        encode_segment(&agent.agent)
    )
}

fn agent_delegation_projection_key(agent: &AuthAgent) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/agent_delegations/{}",
        encode_segment(&agent.org),
        encode_segment(&agent.agent)
    )
}

fn agent_projection_value(agent: &AuthAgent) -> String {
    format!(
        r#"{{"org":"{}","agent":"{}","displayName":"{}","ownerUser":"{}","maxRole":"{}","canInstallApps":{},"canRequestPermissions":{},"canGrantPermissions":{},"status":"{}","source":"{}"}}"#,
        json_string(&agent.org),
        json_string(&agent.agent),
        json_string(&agent.display_name),
        json_string(&agent.owner_user),
        json_string(&agent.max_role),
        agent.can_install_apps,
        agent.can_request_permissions,
        agent.can_grant_permissions,
        json_string(&agent.status),
        json_string(&agent.source)
    )
}

fn grant_projection_key(grant: &AuthGrant) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/grants/subjects/{}/apps/{}/resources/{}",
        encode_segment(&grant.org),
        encode_segment(&grant.subject),
        encode_segment(&grant.app),
        encode_segment(&grant.resource_id)
    )
}

fn grant_by_app_projection_key(grant: &AuthGrant) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/grants_by_app/apps/{}/subjects/{}/resources/{}",
        encode_segment(&grant.org),
        encode_segment(&grant.app),
        encode_segment(&grant.subject),
        encode_segment(&grant.resource_id)
    )
}

fn grant_projection_value(grant: &AuthGrant) -> String {
    format!(
        r#"{{"org":"{}","subject":"{}","app":"{}","resource":{{"namespace":"{}","selectorSchemaId":"{}","selectorId":"{}","selectorJson":{},"resourceId":"{}","verbs":{}}},"grantedBy":"{}","source":"{}","status":"active"}}"#,
        json_string(&grant.org),
        json_string(&grant.subject),
        json_string(&grant.app),
        json_string(&grant.namespace),
        json_string(&grant.selector_schema_id),
        json_string(&grant.selector_id),
        grant.selector_json,
        json_string(&grant.resource_id),
        json_array(&grant.verbs),
        json_string(&grant.granted_by),
        json_string(&grant.source)
    )
}

fn permission_request_projection_key(request: &AuthPermissionRequest) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/permission_requests/{}",
        encode_segment(&request.org),
        encode_segment(&request.request_id)
    )
}

fn permission_request_by_app_projection_key(request: &AuthPermissionRequest) -> String {
    format!(
        "{AUTH_PROJECTION_KEY_PREFIX}/orgs/{}/permission_requests_by_app/{}/{}",
        encode_segment(&request.org),
        encode_segment(&request.app),
        encode_segment(&request.request_id)
    )
}

fn permission_request_projection_value(request: &AuthPermissionRequest) -> String {
    let resources = request
        .resources
        .iter()
        .map(|resource| {
            format!(
                r#"{{"namespace":"{}","selectorSchemaId":"{}","selectorId":"{}","selectorJson":{},"resourceId":"{}","verbs":{}}}"#,
                json_string(&resource.namespace),
                json_string(&resource.selector_schema_id),
                json_string(&resource.selector_id),
                resource.selector_json,
                json_string(&resource.resource_id),
                json_array(&resource.verbs)
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"requestId":"{}","org":"{}","subject":"{}","app":"{}","appName":"{}","operation":"{}","source":"{}","resumeTokenHash":"{}","resources":[{}],"status":"{}","decidedBy":"{}","decisionReason":"{}","decisionSource":"{}"}}"#,
        json_string(&request.request_id),
        json_string(&request.org),
        json_string(&request.subject),
        json_string(&request.app),
        json_string(&request.app_name),
        json_string(&request.operation),
        json_string(&request.source),
        json_string(&request.resume_token_hash),
        resources,
        json_string(&request.status),
        json_string(&request.decided_by),
        json_string(&request.decision_reason),
        json_string(&request.decision_source)
    )
}

pub fn encode_segment(raw: &str) -> String {
    let mut out = String::new();
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(hex(byte >> 4));
            out.push(hex(byte & 0x0f));
        }
    }
    out
}

pub fn decode_segment(encoded: &str) -> Result<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        if i + 2 >= bytes.len() {
            return Err(Error::InvalidInput(format!(
                "bad percent escape in key segment: {encoded:?}"
            )));
        }
        let high = unhex(bytes[i + 1])?;
        let low = unhex(bytes[i + 2])?;
        out.push((high << 4) | low);
        i += 3;
    }
    String::from_utf8(out)
        .map_err(|e| Error::InvalidInput(format!("bad UTF-8 in key segment: {e}")))
}

fn non_empty(value: String, label: &str) -> Result<String> {
    if value.trim().is_empty() {
        return Err(Error::InvalidInput(format!("{label} must not be empty")));
    }
    Ok(value)
}

fn validate_segment_input(label: &str, value: &str) -> Result<()> {
    if value.contains('%') {
        return Err(Error::InvalidInput(format!(
            "{label} must not contain raw percent escapes"
        )));
    }
    Ok(())
}

struct GrantTarget {
    namespace: String,
    selector_id: String,
    selector_json: String,
    resource_id: String,
}

fn parse_grant_target(raw: &str) -> Result<GrantTarget> {
    if let Some(name) = raw.strip_prefix("connection:") {
        let name = terrane_cap_connection::validate_name(name)?;
        let resource_id = terrane_cap_connection::connection_resource_id(&name)?;
        return Ok(GrantTarget {
            namespace: "connection".to_string(),
            selector_id: name.clone(),
            selector_json: format!(
                r#"{{"namespace":"connection","name":"{}"}}"#,
                json_string(&name)
            ),
            resource_id,
        });
    }
    if let Some(name) = raw.strip_prefix("mcp:") {
        let name = terrane_cap_connection::validate_name(name)?;
        return Ok(GrantTarget {
            namespace: "mcp".to_string(),
            selector_id: name.clone(),
            selector_json: format!(
                r#"{{"namespace":"mcp","name":"{}"}}"#,
                json_string(&name)
            ),
            resource_id: format!("mcp:{name}"),
        });
    }
    Ok(GrantTarget {
        namespace: raw.to_string(),
        selector_id: String::new(),
        selector_json: format!(r#"{{"namespace":"{}"}}"#, json_string(raw)),
        resource_id: namespace_resource_id(raw),
    })
}

fn parse_verbs(raw: &str) -> Result<Vec<String>> {
    let verbs: Vec<_> = raw
        .split(',')
        .map(str::trim)
        .filter(|verb| !verb.is_empty())
        .map(ToString::to_string)
        .collect();
    if verbs.is_empty() {
        return Err(Error::InvalidInput("verbs must not be empty".into()));
    }
    for verb in &verbs {
        if !verb
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            return Err(Error::InvalidInput(format!("unsafe grant verb: {verb:?}")));
        }
    }
    Ok(verbs)
}

fn parse_namespaces(raw: &str) -> Result<Vec<String>> {
    let mut namespaces = raw
        .split(',')
        .map(str::trim)
        .filter(|namespace| !namespace.is_empty())
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    namespaces.sort();
    namespaces.dedup();
    if namespaces.is_empty() {
        return Err(Error::InvalidInput(
            "permission request resources must not be empty".into(),
        ));
    }
    for namespace in &namespaces {
        validate_segment_input("namespace", namespace)?;
    }
    Ok(namespaces)
}

fn ensure_grantable_subject(state: &dyn StateStore, subject: &str) -> Result<()> {
    if subject.starts_with("agent:") && !agent_is_active(state, subject)? {
        return Err(Error::InvalidInput(format!(
            "agent subject is not active: {subject}"
        )));
    }
    Ok(())
}

fn agent_is_active(state: &dyn StateStore, agent: &str) -> Result<bool> {
    Ok(state_ref::<AuthState>(state, "auth")?
        .agents
        .get(agent)
        .is_some_and(|record| record.status == "active"))
}

fn non_empty_or_arg(args: &[String], index: usize, fallback: &str) -> String {
    args.get(index)
        .filter(|value| !value.trim().is_empty())
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}

fn parse_bool_arg(args: &[String], index: usize, fallback: bool) -> Result<bool> {
    let Some(value) = args.get(index).map(|value| value.trim()) else {
        return Ok(fallback);
    };
    if value.is_empty() {
        return Ok(fallback);
    }
    match value {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(Error::InvalidInput(format!(
            "boolean argument {index} must be true or false, got {other:?}"
        ))),
    }
}

fn validate_agent_id(agent: &str) -> Result<()> {
    if !agent.starts_with("agent:") {
        return Err(Error::InvalidInput(format!(
            "agent subject must start with agent:, got {agent:?}"
        )));
    }
    if !agent
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b':' | b'-' | b'_' | b'.'))
    {
        return Err(Error::InvalidInput(format!(
            "agent subject must use only ASCII letters, digits, ':', '-', '_', or '.', got {agent:?}"
        )));
    }
    validate_segment_input("agent", agent)
}

fn permission_resource(bus: &dyn CapBus, namespace: String) -> Result<AuthPermissionResource> {
    let spec = namespace_v1_spec(bus, &namespace)?;
    Ok(AuthPermissionResource {
        namespace: namespace.clone(),
        selector_schema_id: spec.selector_schema_id.to_string(),
        selector_id: String::new(),
        selector_json: format!(r#"{{"namespace":"{}"}}"#, json_string(&namespace)),
        resource_id: namespace_resource_id(&namespace),
        verbs: spec.verbs.iter().map(|verb| (*verb).to_string()).collect(),
    })
}

fn namespace_v1_spec(bus: &dyn CapBus, namespace: &str) -> Result<GrantResourceSpec> {
    bus.grant_resource_spec(namespace, NAMESPACE_SELECTOR_SCHEMA_ID)?
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "unknown grant resource namespace or selector schema: {namespace}/{NAMESPACE_SELECTOR_SCHEMA_ID}"
            ))
        })
}

fn parse_grant_verbs(raw: &str, allowed: &[&str]) -> Result<Vec<String>> {
    let verbs = parse_verbs(raw)?;
    for verb in &verbs {
        if !allowed.iter().any(|allowed| allowed == verb) {
            return Err(Error::InvalidInput(format!(
                "unknown grant verb {verb:?}; allowed verbs: {}",
                allowed.join(", ")
            )));
        }
    }
    Ok(verbs)
}

fn hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'A' + (n - 10)) as char,
        _ => unreachable!("nibble"),
    }
}

fn unhex(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(Error::InvalidInput(format!(
            "bad hex digit in key segment: {:?}",
            byte as char
        ))),
    }
}

fn json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn json_array(items: &[String]) -> String {
    let values = items
        .iter()
        .map(|item| format!(r#""{}""#, json_string(item)))
        .collect::<Vec<_>>()
        .join(",");
    format!("[{values}]")
}
