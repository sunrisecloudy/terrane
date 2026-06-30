use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, Instant};

use nanoserde::{DeJson, SerJson};
use terrane_cap_kv::{app_bundle_app_id, app_bundle_files};
use terrane_core::{ExecutionPrincipal, LOCAL_OWNER_SUBJECT};

use crate::{BundleManifest, HostCore};

pub const DEFAULT_ADMIN_BASE_URL: &str = "http://127.0.0.1:8780";

#[derive(Debug, Clone, PartialEq, Eq, SerJson)]
pub struct PermissionRequired {
    #[nserde(rename = "type")]
    pub kind: String,
    pub status: String,
    #[nserde(rename = "requestId")]
    pub request_id: String,
    pub app: String,
    #[nserde(rename = "appName")]
    pub app_name: String,
    pub org: String,
    pub subject: String,
    pub source: String,
    #[nserde(rename = "missingResources")]
    pub missing_resources: Vec<String>,
    #[nserde(rename = "adminUrl")]
    pub admin_url: String,
    #[nserde(rename = "grantCommands")]
    pub grant_commands: Vec<String>,
    #[nserde(rename = "requestStatus")]
    pub request_status: String,
    #[nserde(rename = "resumeTool")]
    pub resume_tool: String,
    #[nserde(rename = "resumeTokenHash")]
    pub resume_token_hash: String,
    pub message: String,
}

impl PermissionRequired {
    pub fn message(&self) -> String {
        self.message.clone()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, SerJson)]
pub struct PermissionRequestResourceView {
    pub namespace: String,
    #[nserde(rename = "selectorSchemaId")]
    pub selector_schema_id: String,
    #[nserde(rename = "resourceId")]
    pub resource_id: String,
    pub verbs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, SerJson)]
pub struct PermissionRequestView {
    #[nserde(rename = "requestId")]
    pub request_id: String,
    pub org: String,
    pub subject: String,
    pub app: String,
    #[nserde(rename = "appName")]
    pub app_name: String,
    pub operation: String,
    pub source: String,
    #[nserde(rename = "resumeTokenHash")]
    pub resume_token_hash: String,
    pub resources: Vec<PermissionRequestResourceView>,
    pub status: String,
    #[nserde(rename = "adminUrl")]
    pub admin_url: String,
    #[nserde(rename = "decidedBy")]
    pub decided_by: String,
    #[nserde(rename = "decisionReason")]
    pub decision_reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, SerJson)]
pub struct PermissionRequestsResponse {
    pub requests: Vec<PermissionRequestView>,
}

pub fn permission_required_for_app(
    core: &HostCore,
    app: &str,
) -> Result<Option<PermissionRequired>, String> {
    permission_required_for_app_with_admin_base(core, app, DEFAULT_ADMIN_BASE_URL)
}

pub fn permission_required_for_app_with_admin_base(
    core: &HostCore,
    app: &str,
    admin_base_url: &str,
) -> Result<Option<PermissionRequired>, String> {
    let principal = ExecutionPrincipal::local_owner();
    let requested = app_requested_resources(core, app)?;
    let grantable: BTreeSet<_> = terrane_core::grant_resource_namespaces()
        .into_iter()
        .collect();
    let mut missing = Vec::new();
    for namespace in requested {
        if !grantable.contains(namespace.as_str()) {
            continue;
        }
        let granted =
            terrane_cap_auth::namespace_granted(core.state(), &principal, app, &namespace)
                .map_err(|e| e.to_string())?;
        if !granted {
            missing.push(namespace);
        }
    }
    missing.sort();
    missing.dedup();
    if missing.is_empty() {
        return Ok(None);
    }

    let request_id = permission_request_id(app, &principal.subject, &missing);
    let admin_url = admin_url(admin_base_url, &request_id);
    let app_name = core
        .state()
        .app
        .apps
        .get(app)
        .map(|record| record.name.clone())
        .unwrap_or_else(|| app.to_string());
    let resume_token_hash = resume_token_hash(&request_id);
    let request_status = terrane_cap_auth::permission_request(core.state(), &request_id)
        .map_err(|e| e.to_string())?
        .map(|request| request.status)
        .unwrap_or_else(|| "unrecorded".to_string());
    let grant_commands = missing
        .iter()
        .map(|namespace| format!("terrane auth grant {LOCAL_OWNER_SUBJECT} {app} {namespace}"))
        .collect::<Vec<_>>();
    let resources = missing.join(", ");
    Ok(Some(PermissionRequired {
        kind: "permission_required".to_string(),
        status: "permission_required".to_string(),
        request_id,
        app: app.to_string(),
        app_name,
        org: principal.org,
        subject: principal.subject,
        source: String::new(),
        missing_resources: missing,
        admin_url: admin_url.clone(),
        grant_commands,
        request_status,
        resume_tool: "permission_check".to_string(),
        resume_token_hash,
        message: format!("permission required for app {app}: grant {resources}; open {admin_url}"),
    }))
}

pub fn request_permission_for_app_with_admin_base(
    core: &mut HostCore,
    app: &str,
    operation: &str,
    source: &str,
    admin_base_url: &str,
) -> Result<Option<PermissionRequired>, String> {
    let Some(mut required) =
        permission_required_for_app_with_admin_base(core, app, admin_base_url)?
    else {
        return Ok(None);
    };
    required.source = source.to_string();
    let resources = required.missing_resources.join(",");
    crate::dispatch_on_core(
        core,
        "auth.permission.request",
        &[
            required.request_id.clone(),
            required.subject.clone(),
            app.to_string(),
            operation.to_string(),
            source.to_string(),
            resources,
            required.app_name.clone(),
            required.resume_token_hash.clone(),
        ],
    )?;
    if let Some(request) = terrane_cap_auth::permission_request(core.state(), &required.request_id)
        .map_err(|e| e.to_string())?
    {
        required.request_status = request.status;
    }
    Ok(Some(required))
}

pub fn permission_request_view(
    core: &HostCore,
    request_id: &str,
    admin_base_url: &str,
) -> Result<Option<PermissionRequestView>, String> {
    terrane_cap_auth::permission_request(core.state(), request_id)
        .map_err(|e| e.to_string())?
        .map(|request| Ok(request_view(request, admin_base_url)))
        .transpose()
}

pub fn permission_requests(
    core: &HostCore,
    admin_base_url: &str,
) -> Result<PermissionRequestsResponse, String> {
    let requests = terrane_cap_auth::permission_requests(core.state())
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|request| request_view(request, admin_base_url))
        .collect();
    Ok(PermissionRequestsResponse { requests })
}

pub fn approve_permission_request(
    core: &mut HostCore,
    request_id: &str,
    reason: &str,
    admin_base_url: &str,
) -> Result<Option<PermissionRequestView>, String> {
    crate::dispatch_on_core(
        core,
        "auth.permission.approve",
        &[request_id.to_string(), reason.to_string()],
    )?;
    permission_request_view(core, request_id, admin_base_url)
}

pub fn deny_permission_request(
    core: &mut HostCore,
    request_id: &str,
    reason: &str,
    admin_base_url: &str,
) -> Result<Option<PermissionRequestView>, String> {
    crate::dispatch_on_core(
        core,
        "auth.permission.deny",
        &[request_id.to_string(), reason.to_string()],
    )?;
    permission_request_view(core, request_id, admin_base_url)
}

pub fn cancel_permission_request(
    core: &mut HostCore,
    request_id: &str,
    reason: &str,
    admin_base_url: &str,
) -> Result<Option<PermissionRequestView>, String> {
    crate::dispatch_on_core(
        core,
        "auth.permission.cancel",
        &[request_id.to_string(), reason.to_string()],
    )?;
    permission_request_view(core, request_id, admin_base_url)
}

pub fn wait_for_permission_decision_at_home(
    home: impl AsRef<Path>,
    request_id: &str,
    admin_base_url: &str,
    timeout: Duration,
) -> Result<Option<PermissionRequestView>, String> {
    let home = home.as_ref();
    let start = Instant::now();
    loop {
        let core = crate::open_at_home(home)?;
        let view = permission_request_view(&core, request_id, admin_base_url)?;
        if view.as_ref().is_some_and(|view| view.status != "pending") {
            return Ok(view);
        }
        if start.elapsed() >= timeout {
            return Ok(view);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn app_requested_resources(core: &HostCore, app: &str) -> Result<Vec<String>, String> {
    let record = core
        .state()
        .app
        .apps
        .get(app)
        .ok_or_else(|| format!("no such app: {app}"))?;
    let source = record
        .source
        .as_deref()
        .ok_or_else(|| format!("app {app} has no --source bundle"))?;

    let manifest = if let Some(source_app) = app_bundle_app_id(source) {
        if source_app != app {
            return Err(format!(
                "app {app} points at kv bundle for different app {source_app}"
            ));
        }
        let files = app_bundle_files(core.state(), app).map_err(|e| e.to_string())?;
        let manifest_text = files
            .get("manifest.json")
            .ok_or_else(|| format!("app {app} has kv bundle source but no manifest.json"))?;
        BundleManifest::deserialize_json(manifest_text)
            .map_err(|e| format!("manifest.json: {e}"))?
    } else {
        crate::read_manifest(Path::new(source)).map_err(|e| e.to_string())?
    };

    let mut resources = manifest.resources;
    resources.sort();
    resources.dedup();
    Ok(resources)
}

pub fn admin_url(admin_base_url: &str, request_id: &str) -> String {
    let base = admin_base_url.trim_end_matches('/');
    format!("{base}/__terrane/admin/requests/{request_id}")
}

pub fn permission_request_id(app: &str, subject: &str, missing: &[String]) -> String {
    let canonical = format!("{}\0{}\0{}", app, subject, missing.join("\0"));
    format!(
        "local-{}-{}-{}-{:016x}",
        safe_token(app),
        safe_token(subject),
        safe_token(&missing.join("_")),
        stable_hash(canonical.as_bytes())
    )
}

pub fn resume_token_hash(request_id: &str) -> String {
    format!("{:016x}", stable_hash(request_id.as_bytes()))
}

fn safe_token(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_') {
            out.push(byte as char);
        } else {
            out.push('-');
        }
    }
    while out.contains("--") {
        out = out.replace("--", "-");
    }
    out.trim_matches('-').to_string()
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn request_view(
    request: terrane_cap_auth::AuthPermissionRequest,
    admin_base_url: &str,
) -> PermissionRequestView {
    PermissionRequestView {
        admin_url: admin_url(admin_base_url, &request.request_id),
        request_id: request.request_id,
        org: request.org,
        subject: request.subject,
        app: request.app,
        app_name: request.app_name,
        operation: request.operation,
        source: request.source,
        resume_token_hash: request.resume_token_hash,
        resources: request
            .resources
            .into_iter()
            .map(|resource| PermissionRequestResourceView {
                namespace: resource.namespace,
                selector_schema_id: resource.selector_schema_id,
                resource_id: resource.resource_id,
                verbs: resource.verbs,
            })
            .collect(),
        status: request.status,
        decided_by: request.decided_by,
        decision_reason: request.decision_reason,
    }
}
