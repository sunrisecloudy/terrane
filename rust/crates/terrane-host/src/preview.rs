use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path};

use nanoserde::{DeJson, SerJson};
use terrane_cap_app::AppRecord;
use terrane_cap_js_runtime::{run_js_bundle, JsRuntimeBundle};
use terrane_core::{
    fold_records_in_memory, ExecutionPrincipal, RuntimeHostHandle, RuntimeResourceHost, State,
    LOCAL_OWNER_SUBJECT,
};

const SUPPORTED_EXTENSIONS: &[&str] = &["html", "htm", "css", "js", "mjs", "json", "svg"];

#[derive(Clone, Debug, PartialEq, Eq, DeJson, SerJson)]
pub struct PreviewFile {
    pub path: String,
    pub content: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct PreviewCreated {
    pub id: String,
    #[nserde(rename = "frameUrl")]
    pub frame_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, SerJson)]
pub struct PreviewAsset {
    pub content: String,
    #[nserde(rename = "contentType")]
    pub content_type: String,
}

#[derive(Default)]
pub struct PreviewStore {
    counter: u64,
    previews: BTreeMap<String, Preview>,
}

struct Preview {
    id: String,
    install_app_id: String,
    name: String,
    files: BTreeMap<String, String>,
    ui: String,
    state: State,
    bundle: JsRuntimeBundle,
    requested_resources: Vec<String>,
    allowed_resources: BTreeSet<String>,
    permission_status: String,
    decided_by: String,
    decision_reason: String,
}

#[derive(DeJson)]
struct PreviewFilesPayload {
    files: Vec<PreviewFile>,
}

#[derive(DeJson)]
struct PreviewManifest {
    #[nserde(default)]
    id: String,
    #[nserde(default)]
    name: String,
    #[nserde(default)]
    backend: String,
    #[nserde(default)]
    runtime: String,
    #[nserde(default)]
    ui: String,
    #[nserde(default)]
    resources: Vec<String>,
}

impl PreviewStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_preview_from_json(
        &mut self,
        files_json: &str,
        base_state: &State,
    ) -> Result<PreviewCreated, String> {
        self.create_preview(parse_files_payload(files_json)?, base_state)
    }

    pub fn create_preview_json_from_json(
        &mut self,
        files_json: &str,
        base_state: &State,
    ) -> Result<String, String> {
        Ok(self
            .create_preview_from_json(files_json, base_state)?
            .serialize_json())
    }

    pub fn create_preview(
        &mut self,
        files: Vec<PreviewFile>,
        base_state: &State,
    ) -> Result<PreviewCreated, String> {
        let files = validate_files(files)?;
        let manifest_text = files
            .get("manifest.json")
            .ok_or_else(|| "missing manifest.json".to_string())?;
        let manifest = parse_manifest(manifest_text)?;
        validate_manifest_id(&manifest.id)?;
        if manifest.runtime != "js" {
            return Err(format!(
                "preview runtime {:?} is not supported; use \"js\"",
                manifest.runtime
            ));
        }

        let ui =
            normalize_rel_path(&manifest.ui).map_err(|e| format!("manifest.ui is invalid: {e}"))?;
        let backend = normalize_rel_path(&manifest.backend)
            .map_err(|e| format!("manifest.backend is invalid: {e}"))?;
        if !files.contains_key(&ui) {
            return Err(format!("manifest.ui references missing file: {ui}"));
        }
        let backend_source = files
            .get(&backend)
            .cloned()
            .ok_or_else(|| format!("manifest.backend references missing file: {backend}"))?;

        self.counter += 1;
        let id = format!("preview-{}-{}", manifest.id.trim(), self.counter);
        let name = match manifest.name.trim() {
            "" => manifest.id.trim().to_string(),
            name => name.to_string(),
        };
        let mut state = base_state.clone();
        state.app.apps.insert(
            id.clone(),
            AppRecord {
                id: id.clone(),
                name: name.clone(),
                source: None,
                runtime: "js".to_string(),
            },
        );
        let bundle = JsRuntimeBundle {
            source: backend_source,
            name: manifest.name.clone(),
            resources: manifest.resources.clone(),
        };
        let requested_resources = grantable_preview_resources(&manifest.resources);
        let permission_status = if requested_resources.is_empty() {
            "approved".to_string()
        } else {
            "pending".to_string()
        };
        self.previews.insert(
            id.clone(),
            Preview {
                id: id.clone(),
                install_app_id: manifest.id.trim().to_string(),
                name,
                files,
                ui,
                state,
                bundle,
                requested_resources,
                allowed_resources: BTreeSet::new(),
                permission_status,
                decided_by: String::new(),
                decision_reason: String::new(),
            },
        );

        Ok(PreviewCreated {
            frame_url: format!("terrane-preview://{id}/frame/"),
            id,
        })
    }

    pub fn destroy_preview(&mut self, id: &str) -> Result<(), String> {
        self.previews
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| format!("no such preview: {id}"))
    }

    pub fn permission_required_with_admin_base(
        &self,
        id: &str,
        admin_base_url: &str,
    ) -> Result<Option<crate::permission::PermissionRequired>, String> {
        let preview = self.preview(id)?;
        let missing = preview
            .requested_resources
            .iter()
            .filter(|namespace| !preview.allowed_resources.contains(*namespace))
            .cloned()
            .collect::<Vec<_>>();
        if missing.is_empty() {
            return Ok(None);
        }
        let request_id = preview_request_id(preview, &missing);
        let admin_url = crate::permission::admin_url(admin_base_url, &request_id);
        let resume_token_hash = crate::permission::resume_token_hash(&request_id);
        let grant_commands = missing
            .iter()
            .map(|namespace| format!("approve preview {id} {namespace}"))
            .collect::<Vec<_>>();
        Ok(Some(crate::permission::PermissionRequired {
            kind: "permission_required".to_string(),
            status: "permission_required".to_string(),
            request_id,
            app: preview.id.clone(),
            app_name: preview.name.clone(),
            org: terrane_core::LOCAL_ORG.to_string(),
            subject: LOCAL_OWNER_SUBJECT.to_string(),
            source: "preview".to_string(),
            missing_resources: missing.clone(),
            admin_url: admin_url.clone(),
            grant_commands,
            request_status: preview.permission_status.clone(),
            resume_tool: "permission_check".to_string(),
            resume_token_hash,
            message: format!(
                "permission required for preview {}: grant {}; open {}",
                preview.id,
                missing.join(", "),
                admin_url
            ),
        }))
    }

    pub fn permission_requests(
        &self,
        admin_base_url: &str,
    ) -> Vec<crate::permission::PermissionRequestView> {
        let mut requests = self
            .previews
            .values()
            .filter_map(|preview| preview_request_view(preview, admin_base_url))
            .collect::<Vec<_>>();
        requests.sort_by(|a, b| a.request_id.cmp(&b.request_id));
        requests
    }

    pub fn permission_request(
        &self,
        request_id: &str,
        admin_base_url: &str,
    ) -> Option<crate::permission::PermissionRequestView> {
        self.previews
            .values()
            .filter_map(|preview| preview_request_view(preview, admin_base_url))
            .find(|view| view.request_id == request_id)
    }

    pub fn approve_permission_request(
        &mut self,
        request_id: &str,
        reason: &str,
        admin_base_url: &str,
    ) -> Result<Option<crate::permission::PermissionRequestView>, String> {
        self.decide_permission_request(request_id, "approved", reason, admin_base_url)
    }

    pub fn deny_permission_request(
        &mut self,
        request_id: &str,
        reason: &str,
        admin_base_url: &str,
    ) -> Result<Option<crate::permission::PermissionRequestView>, String> {
        self.decide_permission_request(request_id, "denied", reason, admin_base_url)
    }

    pub fn cancel_permission_request(
        &mut self,
        request_id: &str,
        reason: &str,
        admin_base_url: &str,
    ) -> Result<Option<crate::permission::PermissionRequestView>, String> {
        self.decide_permission_request(request_id, "cancelled", reason, admin_base_url)
    }

    pub fn promote_permission_request(
        &self,
        core: &mut crate::HostCore,
        request_id: &str,
        installed_app: &str,
        admin_base_url: &str,
    ) -> Result<Option<crate::permission::PermissionRequestView>, String> {
        let Some(preview) = self.previews.values().find(|preview| {
            preview_request_view(preview, admin_base_url)
                .is_some_and(|view| view.request_id == request_id)
        }) else {
            return Ok(None);
        };
        if preview.permission_status != "approved" {
            return Err(format!(
                "permission request {request_id} is {}",
                preview.permission_status
            ));
        }
        let requested_app = installed_app.trim();
        if !requested_app.is_empty() && requested_app != preview.install_app_id {
            return Err(format!(
                "preview promotion target {requested_app:?} does not match preview app {:?}",
                preview.install_app_id
            ));
        }
        let installed_app = preview.install_app_id.as_str();
        for namespace in &preview.requested_resources {
            crate::dispatch_on_core(
                core,
                "auth.grant",
                &[
                    LOCAL_OWNER_SUBJECT.to_string(),
                    installed_app.to_string(),
                    namespace.clone(),
                ],
            )?;
        }
        Ok(preview_request_view(preview, admin_base_url))
    }

    fn decide_permission_request(
        &mut self,
        request_id: &str,
        status: &str,
        reason: &str,
        admin_base_url: &str,
    ) -> Result<Option<crate::permission::PermissionRequestView>, String> {
        let Some(preview) = self.previews.values_mut().find(|preview| {
            preview_request_view(preview, admin_base_url)
                .is_some_and(|view| view.request_id == request_id)
        }) else {
            return Ok(None);
        };
        if preview.permission_status == status {
            return Ok(preview_request_view(preview, admin_base_url));
        }
        if preview.permission_status != "pending" {
            return Err(format!(
                "permission request {request_id} is {}",
                preview.permission_status
            ));
        }
        preview.permission_status = status.to_string();
        preview.decided_by = LOCAL_OWNER_SUBJECT.to_string();
        preview.decision_reason = reason.to_string();
        if status == "approved" {
            preview.allowed_resources = preview.requested_resources.iter().cloned().collect();
        }
        Ok(preview_request_view(preview, admin_base_url))
    }

    pub fn read_asset(&self, id: &str, rel_path: &str) -> Result<PreviewAsset, String> {
        let preview = self.preview(id)?;
        let path = preview.resolve_asset_path(rel_path)?;
        let content = preview
            .files
            .get(&path)
            .cloned()
            .ok_or_else(|| format!("preview asset not found: {path}"))?;
        Ok(PreviewAsset {
            content,
            content_type: content_type_for_path(&path).to_string(),
        })
    }

    pub fn read_asset_json(&self, id: &str, rel_path: &str) -> Result<String, String> {
        Ok(self.read_asset(id, rel_path)?.serialize_json())
    }

    pub fn invoke_backend(
        &mut self,
        id: &str,
        verb: &str,
        args: &[String],
    ) -> Result<String, String> {
        let preview = self.preview_mut(id)?;
        if verb.trim().is_empty() {
            return Err("preview verb must not be empty".to_string());
        }
        let mut input = Vec::with_capacity(args.len() + 1);
        input.push(verb.to_string());
        input.extend(args.iter().cloned());
        let host = RuntimeHostHandle::new(Box::new(
            RuntimeResourceHost::new_with_temporary_resource_grants(
                preview.id.clone(),
                preview.state.clone(),
                ExecutionPrincipal::local_owner(),
                preview.allowed_resources.iter().cloned(),
            ),
        ));
        let output = run_js_bundle(&preview.id, &input, &preview.bundle, host.clone())
            .map_err(|e| e.to_string())?;
        let records = host.take_records();
        fold_records_in_memory(&mut preview.state, &records).map_err(|e| e.to_string())?;
        Ok(output)
    }

    fn preview(&self, id: &str) -> Result<&Preview, String> {
        self.previews
            .get(id)
            .ok_or_else(|| format!("no such preview: {id}"))
    }

    fn preview_mut(&mut self, id: &str) -> Result<&mut Preview, String> {
        self.previews
            .get_mut(id)
            .ok_or_else(|| format!("no such preview: {id}"))
    }
}

fn grantable_preview_resources(resources: &[String]) -> Vec<String> {
    let grantable: BTreeSet<_> = terrane_core::grant_resource_namespaces()
        .into_iter()
        .collect();
    let mut resources = resources
        .iter()
        .filter(|namespace| grantable.contains(namespace.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    resources.sort();
    resources.dedup();
    resources
}

fn preview_request_view(
    preview: &Preview,
    admin_base_url: &str,
) -> Option<crate::permission::PermissionRequestView> {
    if preview.requested_resources.is_empty() {
        return None;
    }
    let request_id = preview_request_id(preview, &preview.requested_resources);
    Some(crate::permission::PermissionRequestView {
        admin_url: crate::permission::admin_url(admin_base_url, &request_id),
        request_id: request_id.clone(),
        org: terrane_core::LOCAL_ORG.to_string(),
        subject: LOCAL_OWNER_SUBJECT.to_string(),
        app: preview.id.clone(),
        app_name: preview.name.clone(),
        operation: "preview".to_string(),
        source: "preview".to_string(),
        resume_token_hash: crate::permission::resume_token_hash(&request_id),
        resources: preview
            .requested_resources
            .iter()
            .filter_map(|namespace| preview_resource_view(namespace))
            .collect(),
        status: preview.permission_status.clone(),
        decided_by: preview.decided_by.clone(),
        decision_reason: preview.decision_reason.clone(),
    })
}

fn preview_resource_view(
    namespace: &str,
) -> Option<crate::permission::PermissionRequestResourceView> {
    let spec = terrane_core::grant_resource_specs()
        .into_iter()
        .find(|spec| spec.namespace == namespace)?;
    Some(crate::permission::PermissionRequestResourceView {
        namespace: spec.namespace.to_string(),
        selector_schema_id: spec.selector_schema_id.to_string(),
        resource_id: spec.namespace.to_string(),
        verbs: spec.verbs.iter().map(|verb| (*verb).to_string()).collect(),
    })
}

fn preview_request_id(preview: &Preview, resources: &[String]) -> String {
    crate::permission::permission_request_id(&preview.id, LOCAL_OWNER_SUBJECT, resources)
}

impl Preview {
    fn resolve_asset_path(&self, rel_path: &str) -> Result<String, String> {
        if rel_path.is_empty() {
            return Ok(self.ui.clone());
        }
        let rel = normalize_rel_path(rel_path)?;
        Ok(match self.ui.rsplit_once('/') {
            Some((parent, _)) if !parent.is_empty() => format!("{parent}/{rel}"),
            _ => rel,
        })
    }
}

fn parse_files_payload(raw: &str) -> Result<Vec<PreviewFile>, String> {
    if raw.trim_start().starts_with('[') {
        Vec::<PreviewFile>::deserialize_json(raw).map_err(|e| format!("preview files JSON: {e}"))
    } else {
        PreviewFilesPayload::deserialize_json(raw)
            .map(|p| p.files)
            .map_err(|e| format!("preview files JSON: {e}"))
    }
}

fn validate_files(files: Vec<PreviewFile>) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for file in files {
        let path = normalize_rel_path(&file.path)?;
        let ext = extension(&path).ok_or_else(|| format!("unsupported file extension: {path}"))?;
        if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            return Err(format!("unsupported file extension: {path}"));
        }
        if out.insert(path.clone(), file.content).is_some() {
            return Err(format!("duplicate preview file: {path}"));
        }
    }
    Ok(out)
}

fn parse_manifest(text: &str) -> Result<PreviewManifest, String> {
    let mut m =
        PreviewManifest::deserialize_json(text).map_err(|e| format!("manifest.json: {e}"))?;
    if m.runtime.trim().is_empty() {
        m.runtime = "js".to_string();
    }
    if m.runtime != "js" {
        return Ok(m);
    }
    if m.backend.trim().is_empty() {
        return Err("missing manifest.backend".to_string());
    }
    if m.ui.trim().is_empty() {
        return Err("missing manifest.ui".to_string());
    }
    Ok(m)
}

fn validate_manifest_id(id: &str) -> Result<(), String> {
    let id = id.trim();
    if id.is_empty() {
        return Err("missing manifest.id".to_string());
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(format!(
            "manifest.id is unsafe: {id:?}; use ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

fn normalize_rel_path(input: &str) -> Result<String, String> {
    if input.trim().is_empty() {
        return Err("path must not be empty".to_string());
    }
    if input.contains('\\') {
        return Err(format!("path must use '/' separators: {input}"));
    }
    let mut parts = Vec::new();
    for component in Path::new(input).components() {
        match component {
            Component::Normal(part) => {
                let s = part
                    .to_str()
                    .ok_or_else(|| format!("path is not valid UTF-8: {input}"))?;
                if !s.is_empty() {
                    parts.push(s.to_string());
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("parent-dir components are not allowed: {input}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("absolute paths are not allowed: {input}"));
            }
        }
    }
    if parts.is_empty() {
        return Err(format!("path must name a file: {input}"));
    }
    Ok(parts.join("/"))
}

fn extension(path: &str) -> Option<String> {
    let file = path.rsplit('/').next()?;
    let (_, ext) = file.rsplit_once('.')?;
    Some(ext.to_ascii_lowercase())
}

fn content_type_for_path(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") | Some("mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[path = "preview_tests.rs"]
mod tests;
