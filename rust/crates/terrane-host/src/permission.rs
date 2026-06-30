use std::collections::BTreeSet;
use std::path::Path;

use nanoserde::{DeJson, SerJson};
use terrane_cap_kv::{app_bundle_app_id, app_bundle_files};
use terrane_core::{ExecutionPrincipal, LOCAL_OWNER_SUBJECT};

use crate::{BundleManifest, HostCore};

pub const DEFAULT_ADMIN_BASE_URL: &str = "http://127.0.0.1:8780";

#[derive(Debug, Clone, PartialEq, Eq, SerJson)]
pub struct PermissionRequired {
    #[nserde(rename = "type")]
    pub kind: String,
    #[nserde(rename = "requestId")]
    pub request_id: String,
    pub app: String,
    pub org: String,
    pub subject: String,
    #[nserde(rename = "missingResources")]
    pub missing_resources: Vec<String>,
    #[nserde(rename = "adminUrl")]
    pub admin_url: String,
    #[nserde(rename = "grantCommands")]
    pub grant_commands: Vec<String>,
    pub message: String,
}

impl PermissionRequired {
    pub fn message(&self) -> String {
        self.message.clone()
    }
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
    let grant_commands = missing
        .iter()
        .map(|namespace| format!("terrane auth grant {LOCAL_OWNER_SUBJECT} {app} {namespace}"))
        .collect::<Vec<_>>();
    let resources = missing.join(", ");
    Ok(Some(PermissionRequired {
        kind: "permission_required".to_string(),
        request_id,
        app: app.to_string(),
        org: principal.org,
        subject: principal.subject,
        missing_resources: missing,
        admin_url: admin_url.clone(),
        grant_commands,
        message: format!("permission required for app {app}: grant {resources}; open {admin_url}"),
    }))
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
