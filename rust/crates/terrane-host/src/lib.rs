//! terrane-host — shared operational host spine.
//!
//! This crate owns the parts every host needs: opening a Terrane home, minting a
//! replica identity, installing bundles, running app backends, syncing replicas,
//! and exposing the public contract surface. Transport adapters such as the CLI,
//! HTTP server, MCP server, and FFI layer should stay thin and call into here.

use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};

use nanoserde::DeJson;
use terrane_api::{AppSummary, AppsResponse};
use terrane_cap_builder::draft_json;
use terrane_cap_crdt::crdt_export_hex;
use terrane_cap_js_runtime::{run_js_bundle, JsRuntimeBundle};
use terrane_cap_kv::app_bundle_source;
use terrane_core::Core;
pub use terrane_core::EventRecord;
use terrane_core::{
    Decision, Error, ExecutionPrincipal, QueryValue, Request, RuntimeHostHandle,
    RuntimeResourceHost,
};

pub mod app_log;
pub mod asr;
pub mod blob_store;
pub mod cap_doc;
pub mod cli;
pub mod edge;
pub mod ffi;
pub mod home;
pub mod i18n;
pub mod deep_links;
mod local_llm;
pub mod mcp;
mod metrics;
mod media_edge;
pub mod mcp_client;
pub mod native;
pub mod permission;
pub mod preview;
pub mod public_authz;
pub mod scheduler;
pub mod secret_store;
mod stt_edge;
pub mod stt_runner;
mod tts_edge;
pub mod sync;

pub use edge::{generate_app_records, EdgeRunner, HarnessStaging};
pub use home::{home_page, HomePageOptions};
pub use i18n::{import_i18n_dir, seed_public_i18n, I18nImportOutcome};
pub use preview::{PreviewAsset, PreviewCreated, PreviewFile, PreviewStore};

/// Release in-process local-model engines. Hosts (and the CLI) call this once
/// before a normal exit; safe to call when nothing is cached.
pub fn local_llm_shutdown() {
    local_llm::shutdown();
    asr::shutdown();
    stt_edge::shutdown();
}
pub use sync::{serve_conn, sync_conn};
pub use terrane_cap_auth::agent_subject;
pub use terrane_core::LOCAL_OWNER_SUBJECT;

pub type HostCore = Core<EdgeRunner>;

/// Default address `terrane serve` binds when none is given.
pub const DEFAULT_SERVE_ADDR: &str = "127.0.0.1:7777";

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub records: Vec<EventRecord>,
    pub output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDryRunOutcome {
    pub records: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookIngestOutcome {
    pub app: String,
    pub name: String,
    pub verb: String,
    pub delivery_json: String,
    pub body_kind: String,
    pub body_hash: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookIngestRequest {
    pub app: String,
    pub name: String,
    pub token: String,
    pub method: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub body_mime: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed { id: String, source: String },
    Refreshed { id: String },
}

impl InstallOutcome {
    pub fn message(&self) -> String {
        match self {
            InstallOutcome::Installed { id, source } => format!("installed {id} -> {source}"),
            InstallOutcome::Refreshed { id } => {
                format!(
                    "refreshed {id} (already installed; `terrane app remove {id}` to re-catalog)"
                )
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOutcome {
    NothingToSync { app: String, from_home: String },
    AlreadyUpToDate { from_home: String },
    Synced { app: String, from_home: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvokeFailure {
    PermissionRequired(Box<permission::PermissionRequired>),
    Other(String),
}

impl InvokeFailure {
    pub fn message(&self) -> String {
        match self {
            InvokeFailure::PermissionRequired(required) => required.message(),
            InvokeFailure::Other(message) => message.clone(),
        }
    }
}

impl SyncOutcome {
    pub fn message(&self) -> String {
        match self {
            SyncOutcome::NothingToSync { app, from_home } => {
                format!("(nothing to sync: {from_home} has no '{app}' data)")
            }
            SyncOutcome::AlreadyUpToDate { from_home } => {
                format!("(already up to date with {from_home})")
            }
            SyncOutcome::Synced { app, from_home } => format!("synced '{app}' from {from_home}"),
        }
    }
}

/// Open the workspace core at `$TERRANE_HOME/log.bin` with the real edge runner
/// and ensure the home has a stable replica identity.
pub fn open() -> Result<HostCore, String> {
    open_at_home(home_dir())
}

/// Open a specific Terrane home directory (`<home>/log.bin`).
pub fn open_at_home(home: impl AsRef<Path>) -> Result<HostCore, String> {
    open_at_log_path(log_path_for_home(home))
}

/// Open a specific event log path.
pub fn open_at_log_path(log_path: impl Into<PathBuf>) -> Result<HostCore, String> {
    open_at_log_path_with(log_path, EdgeRunner::default())
}

/// Open the default home with a [`HarnessStaging`] handle attached, so the
/// host can background `harness.generate-app` and commit staged results.
pub fn open_with_staging(staging: HarnessStaging) -> Result<HostCore, String> {
    open_at_home_with_staging(home_dir(), staging)
}

/// Open a specific home with a [`HarnessStaging`] handle attached.
pub fn open_at_home_with_staging(
    home: impl AsRef<Path>,
    staging: HarnessStaging,
) -> Result<HostCore, String> {
    open_at_log_path_with(log_path_for_home(home), EdgeRunner::with_staging(staging))
}

fn open_at_log_path_with(
    log_path: impl Into<PathBuf>,
    runner: EdgeRunner,
) -> Result<HostCore, String> {
    let log_path = log_path.into();
    let home = log_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let mut core = Core::open_with(log_path, runner.with_home(home)).map_err(|e| e.to_string())?;
    ensure_identity(&mut core)?;
    Ok(core)
}

/// The home directory: `$TERRANE_HOME` (default `./.terrane/`). Holds the event
/// log and the installed app bundles (`apps/<id>/`).
pub fn home_dir() -> PathBuf {
    env::var("TERRANE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".terrane"))
}

/// The on-disk log path: `$TERRANE_HOME/log.bin`.
pub fn log_path() -> PathBuf {
    log_path_for_home(home_dir())
}

pub fn log_path_for_home(home: impl AsRef<Path>) -> PathBuf {
    home.as_ref().join("log.bin")
}

/// Ensure this home has minted its replica identity before it authors anything.
/// Idempotent — `replica.init` is a no-op once the id exists.
pub fn ensure_identity(core: &mut HostCore) -> Result<(), String> {
    if core.state().replica.peer.is_none() {
        core.dispatch(Request::new("replica.init", Vec::new()))
            .map_err(|e| e.to_string())?;
    }
    if !terrane_cap_auth::local_owner_member_exists(core.state()).map_err(|e| e.to_string())? {
        core.dispatch(Request::trusted_host(
            "auth.member.ensure-local-owner",
            Vec::new(),
        ))
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Build a Request from a dotted command name + args and dispatch it.
pub fn dispatch(command: &str, args: &[String]) -> Result<CommandOutcome, String> {
    let mut core = open()?;
    dispatch_on_core(&mut core, command, args)
}

pub fn dispatch_on_core(
    core: &mut HostCore,
    command: &str,
    args: &[String],
) -> Result<CommandOutcome, String> {
    ensure_identity(core)?;
    dispatch_request_on_core(core, Request::trusted_host(command, args.to_vec()))
}

pub fn ingest_webhook_on_core(
    core: &mut HostCore,
    request: WebhookIngestRequest,
) -> Result<WebhookIngestOutcome, String> {
    ensure_identity(core)?;
    let WebhookIngestRequest {
        app,
        name,
        token,
        method,
        headers,
        body,
        body_mime,
    } = request;
    let meta = core
        .state()
        .webhook
        .routes
        .get(&app)
        .and_then(|routes| routes.get(&name))
        .cloned()
        .ok_or_else(|| "not found".to_string())?;
    if !terrane_cap_webhook::route_matches(&meta, &token) {
        return Err("not found".to_string());
    }
    let received_at = current_epoch_ms()?;
    let header_json = headers
        .into_iter()
        .map(|(key, value)| (key, serde_json::Value::String(value)))
        .collect::<serde_json::Map<_, _>>();
    let body_hash = terrane_cap_net::request::sha256_hex(&body);
    let mut envelope = serde_json::Map::new();
    envelope.insert("app".to_string(), serde_json::Value::String(app.clone()));
    envelope.insert("name".to_string(), serde_json::Value::String(name.clone()));
    envelope.insert("token".to_string(), serde_json::Value::String(token));
    envelope.insert("method".to_string(), serde_json::Value::String(method));
    envelope.insert("headers".to_string(), serde_json::Value::Object(header_json));
    envelope.insert(
        "received_at".to_string(),
        serde_json::Value::Number(serde_json::Number::from(received_at)),
    );
    if let Some(mime) = body_mime {
        envelope.insert("body_mime".to_string(), serde_json::Value::String(mime));
    }
    match std::str::from_utf8(&body) {
        Ok(text) => {
            envelope.insert("body".to_string(), serde_json::Value::String(text.to_string()));
        }
        Err(_) => {
            use base64::Engine as _;
            envelope.insert(
                "body_base64".to_string(),
                serde_json::Value::String(base64::engine::general_purpose::STANDARD.encode(&body)),
            );
        }
    }
    let raw = serde_json::Value::Object(envelope).to_string();
    let outcome = dispatch_request_on_core(
        core,
        Request::trusted_host("webhook.ingest", vec![raw]),
    )?;
    let received = outcome
        .records
        .iter()
        .find(|record| record.kind == "webhook.received")
        .ok_or_else(|| "webhook.ingest produced no webhook.received event".to_string())?;
    let delivery = terrane_cap_webhook::decode_delivery(received).map_err(|e| e.to_string())?;
    if delivery.body_kind == "blob" {
        blob_store::insert_if_absent(&home_dir(), &delivery.body_hash, &body)
            .map_err(|e| e.to_string())?;
        let args = vec![
            delivery.app.clone(),
            delivery.body.clone(),
            delivery.body_hash.clone(),
            delivery.body_size.to_string(),
            delivery.body_mime.clone(),
        ];
        dispatch_request_on_core(core, Request::trusted_host("blob.link", args))?;
    }
    let delivery_json = serde_json::to_string(&serde_json::json!({
        "app": delivery.app,
        "name": delivery.name,
        "method": delivery.method,
        "headers": delivery.headers,
        "body_kind": delivery.body_kind,
        "body": delivery.body,
        "body_is_base64": delivery.body_is_base64,
        "body_hash": delivery.body_hash,
        "body_size": delivery.body_size,
        "body_mime": delivery.body_mime,
        "received_at": delivery.received_at,
    }))
    .map_err(|e| e.to_string())?;
    Ok(WebhookIngestOutcome {
        app,
        name,
        verb: meta.verb,
        delivery_json,
        body_kind: delivery.body_kind,
        body_hash,
    })
}

fn current_epoch_ms() -> Result<u64, String> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "wall clock reads before the Unix epoch".to_string())
        .and_then(|duration| {
            u64::try_from(duration.as_millis())
                .map_err(|_| "wall clock epoch milliseconds overflow".to_string())
        })
}

pub fn dispatch_public_on_core(
    core: &mut HostCore,
    command: &str,
    args: &[String],
) -> Result<CommandOutcome, String> {
    ensure_identity(core)?;
    match public_authz::authorize_public_command(core, command, args)? {
        public_authz::PublicCommandAuthz::Allow => {}
        public_authz::PublicCommandAuthz::Refuse { reason } => return Err(reason),
        public_authz::PublicCommandAuthz::NeedsGrant { app, namespace } => {
            return Err(format!(
                "permission required for capability_command:{command}: grant {namespace} to {app}"
            ));
        }
    }
    dispatch_request_on_core(core, Request::new(command, args.to_vec()))
}

fn dispatch_request_on_core(
    core: &mut HostCore,
    request: Request,
) -> Result<CommandOutcome, String> {
    let records = core.dispatch(request).map_err(|e| e.to_string())?;
    Ok(CommandOutcome {
        records,
        output: core.take_last_output(),
    })
}

pub fn dry_run_on_core(
    core: &HostCore,
    command: &str,
    args: &[String],
) -> Result<CommandDryRunOutcome, String> {
    dry_run_request_on_core(core, Request::trusted_host(command, args.to_vec()), command)
}

pub fn dry_run_public_on_core(
    core: &HostCore,
    command: &str,
    args: &[String],
) -> Result<CommandDryRunOutcome, String> {
    match public_authz::authorize_public_command(core, command, args)? {
        public_authz::PublicCommandAuthz::Allow => {}
        public_authz::PublicCommandAuthz::Refuse { reason } => return Err(reason),
        public_authz::PublicCommandAuthz::NeedsGrant { app, namespace } => {
            return Err(format!(
                "permission required for capability_command:{command}: grant {namespace} to {app}"
            ));
        }
    }
    dry_run_request_on_core(core, Request::new(command, args.to_vec()), command)
}

fn dry_run_request_on_core(
    core: &HostCore,
    request: Request,
    command: &str,
) -> Result<CommandDryRunOutcome, String> {
    match core.decide(request).map_err(|e| e.to_string())? {
        Decision::Commit(records) => Ok(CommandDryRunOutcome {
            records: records.len(),
        }),
        Decision::Effect(_) | Decision::TransientEffect(_) => Err(format!(
            "dryRun unsupported for command '{command}': command requires an effect"
        )),
        Decision::Runtime(_) => Err(format!(
            "dryRun unsupported for command '{command}': command invokes a runtime"
        )),
    }
}

pub fn query_on_core(
    core: &HostCore,
    capability: &str,
    query: &str,
    args: &[String],
) -> Result<QueryValue, String> {
    core.query(capability, query, args)
        .map_err(|e| e.to_string())
}

/// Run an app backend and return the backend output string.
pub fn invoke_app(
    core: &mut HostCore,
    app: &str,
    verb: &str,
    args: &[String],
) -> Result<String, String> {
    invoke_app_checked(core, app, verb, args).map_err(|e| e.message())
}

pub fn invoke_app_checked(
    core: &mut HostCore,
    app: &str,
    verb: &str,
    args: &[String],
) -> Result<String, InvokeFailure> {
    invoke_app_checked_with_admin_base(core, app, verb, args, permission::DEFAULT_ADMIN_BASE_URL)
}

pub fn invoke_app_checked_with_admin_base(
    core: &mut HostCore,
    app: &str,
    verb: &str,
    args: &[String],
    admin_base_url: &str,
) -> Result<String, InvokeFailure> {
    invoke_app_checked_with_admin_base_and_source(core, app, verb, args, admin_base_url, "host")
}

pub fn invoke_app_checked_with_admin_base_and_source(
    core: &mut HostCore,
    app: &str,
    verb: &str,
    args: &[String],
    admin_base_url: &str,
    source: &str,
) -> Result<String, InvokeFailure> {
    let mut input = Vec::with_capacity(args.len() + 1);
    input.push(verb.to_string());
    input.extend(args.iter().cloned());
    invoke_app_input_checked_with_admin_base_and_source(core, app, &input, admin_base_url, source)
}

/// Run an app backend with the exact runtime input vector.
pub fn invoke_app_input(
    core: &mut HostCore,
    app: &str,
    input: &[String],
) -> Result<String, String> {
    invoke_app_input_checked(core, app, input).map_err(|e| e.message())
}

pub fn invoke_app_input_checked(
    core: &mut HostCore,
    app: &str,
    input: &[String],
) -> Result<String, InvokeFailure> {
    invoke_app_input_checked_with_admin_base(core, app, input, permission::DEFAULT_ADMIN_BASE_URL)
}

pub fn invoke_app_input_checked_with_admin_base(
    core: &mut HostCore,
    app: &str,
    input: &[String],
    admin_base_url: &str,
) -> Result<String, InvokeFailure> {
    invoke_app_input_checked_with_admin_base_and_source(core, app, input, admin_base_url, "host")
}

pub fn invoke_app_input_checked_with_admin_base_and_source(
    core: &mut HostCore,
    app: &str,
    input: &[String],
    admin_base_url: &str,
    source: &str,
) -> Result<String, InvokeFailure> {
    if !core.state().app.apps.contains_key(app) {
        return Err(InvokeFailure::Other(format!("no such app: {app}")));
    }
    let operation = input.first().map(String::as_str).unwrap_or("invoke");
    if let Some(required) = permission::request_permission_for_app_with_admin_base(
        core,
        app,
        operation,
        source,
        admin_base_url,
    )
    .map_err(InvokeFailure::Other)?
    {
        return Err(InvokeFailure::PermissionRequired(Box::new(required)));
    }
    let runtime = core.state().app.apps[app].runtime.as_str();
    let command = match runtime {
        "js" => "js-runtime.run",
        "wasm" => "wasm-runtime.run",
        other => {
            return Err(InvokeFailure::Other(format!(
                "unknown app runtime for {app}: {other}"
            )))
        }
    };
    let mut argv = Vec::with_capacity(input.len() + 1);
    argv.push(app.to_string());
    argv.extend(input.iter().cloned());
    Ok(dispatch_on_core(core, command, &argv)
        .map_err(InvokeFailure::Other)?
        .output
        .unwrap_or_default())
}

/// Return an app's self-declared action metadata by invoking its reserved
/// `__actions__` verb.
pub fn app_actions(core: &mut HostCore, app: &str) -> Result<String, String> {
    app_actions_checked(core, app).map_err(|e| e.message())
}

pub fn app_actions_checked(core: &mut HostCore, app: &str) -> Result<String, InvokeFailure> {
    app_actions_checked_with_admin_base(core, app, permission::DEFAULT_ADMIN_BASE_URL)
}

pub fn app_actions_checked_with_admin_base(
    core: &mut HostCore,
    app: &str,
    admin_base_url: &str,
) -> Result<String, InvokeFailure> {
    app_actions_checked_with_admin_base_and_source(core, app, admin_base_url, "host")
}

pub fn app_actions_checked_with_admin_base_and_source(
    core: &mut HostCore,
    app: &str,
    admin_base_url: &str,
    source: &str,
) -> Result<String, InvokeFailure> {
    invoke_app_checked_with_admin_base_and_source(
        core,
        app,
        terrane_api::ACTIONS_VERB,
        &[],
        admin_base_url,
        source,
    )
}

/// Ask the core builder capability to generate a draft app and return the
/// latest draft as JSON for host bridges.
pub fn generate_app_json(
    core: &mut HostCore,
    app_id: &str,
    name: &str,
    prompt: &str,
    harness: Option<&str>,
) -> Result<String, String> {
    let draft_id = app_id.trim();
    let harness = harness
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("codex");
    dispatch_on_core(
        core,
        "harness.generate-app",
        &[
            "--harness".to_string(),
            harness.to_string(),
            draft_id.to_string(),
            app_id.to_string(),
            name.to_string(),
            prompt.to_string(),
        ],
    )?;
    let draft = core
        .state()
        .builder
        .drafts
        .get(draft_id)
        .ok_or_else(|| format!("builder draft missing after generation: {draft_id}"))?;
    Ok(draft_json(draft))
}

/// The JSON view of a builder draft, if it exists in state.
pub fn builder_draft_json(core: &HostCore, draft_id: &str) -> Option<String> {
    core.state().builder.drafts.get(draft_id).map(draft_json)
}

pub fn list_apps(core: &HostCore) -> AppsResponse {
    let apps = core
        .state()
        .app
        .apps
        .values()
        .map(|app| AppSummary {
            id: app.id.clone(),
            name: app.name.clone(),
            icon: app_icon(app.source.as_deref()).unwrap_or_default(),
            has_ui: app_has_ui(app.source.as_deref()),
        })
        .collect();
    AppsResponse { apps }
}

pub fn app_has_ui(source: Option<&str>) -> bool {
    source.and_then(read_manifest_ui).is_some()
}

pub fn app_icon(source: Option<&str>) -> Option<String> {
    source.and_then(read_manifest_icon)
}

/// The app's declared UI entry file (`manifest.ui`), if any.
pub fn read_manifest_ui(source: &str) -> Option<String> {
    read_manifest(Path::new(source))
        .ok()
        .map(|m| m.ui)
        .filter(|ui| !ui.is_empty())
}

/// The app's declared icon asset (`manifest.icon`), if any.
pub fn read_manifest_icon(source: &str) -> Option<String> {
    read_manifest(Path::new(source))
        .ok()
        .map(|m| m.icon)
        .filter(|icon| !icon.is_empty())
}

/// `app install <path>`: copy a bundle into this home's `apps/<id>/` and catalog
/// it from there, so the home owns the app and no longer depends on the install
/// command's working directory.
pub fn install_app(path: &str) -> Result<InstallOutcome, String> {
    let src = Path::new(path);
    let manifest = read_manifest(src).map_err(|e| e.to_string())?;
    let id = manifest.id.trim().to_string();
    validate_bundle_id(path, &id)?;
    validate_runtime(path, &manifest.runtime)?;
    validate_common_api_bundle(src)?;
    let name = match manifest.name.trim() {
        "" => id.clone(),
        name => name.to_string(),
    };
    let runtime = manifest.runtime.trim().to_string();

    let dest = home_dir().join("apps").join(&id);
    let in_place = matches!(
        (src.canonicalize(), dest.canonicalize()),
        (Ok(s), Ok(d)) if s == d
    );
    if !in_place {
        copy_dir(src, &dest).map_err(|e| format!("copy bundle into home: {e}"))?;
    }
    let dest_abs = dest
        .canonicalize()
        .map_err(|e| format!("resolve {}: {e}", dest.display()))?;
    let source = dest_abs
        .to_str()
        .ok_or("install path is not valid UTF-8")?
        .to_string();

    let mut core = open()?;
    let mut args = vec![
        id.clone(),
        name,
        "--source".into(),
        source.clone(),
        "--runtime".into(),
        runtime,
        "--interfaces".into(),
        terrane_cap_app::normalize_interfaces(manifest.interfaces).join(","),
    ];
    let file_types = manifest_file_types_arg(&manifest.file_types)?;
    if !file_types.is_empty() {
        args.push("--file-types".into());
        args.push(file_types);
    }
    match core.dispatch(Request::new("app.add", args)) {
        Ok(_) => Ok(InstallOutcome::Installed { id, source }),
        Err(Error::AppExists(_)) => Ok(InstallOutcome::Refreshed { id }),
        Err(e) => Err(e.to_string()),
    }
}

/// `app install-kv <path>`: import a JS bundle into reserved cap-kv keys and
/// catalog it with a `kv://app-bundle/<id>` source. If a storage backend is
/// provided, the same commit configures that app's kv projection first.
pub fn install_app_to_kv(
    path: &str,
    storage_backend: Option<String>,
    storage_path: Option<String>,
) -> Result<InstallOutcome, String> {
    let src = Path::new(path);
    let manifest = read_manifest(src).map_err(|e| e.to_string())?;
    let id = manifest.id.trim().to_string();
    validate_bundle_id(path, &id)?;
    validate_runtime(path, &manifest.runtime)?;
    validate_common_api_bundle(src)?;
    if manifest.runtime != "js" {
        return Err("app install-kv currently supports js text bundles only".into());
    }
    let source = app_bundle_source(&id);

    let mut args = vec![path.to_string()];
    if let Some(backend) = storage_backend {
        args.push("--storage".into());
        args.push(backend);
    }
    if let Some(path) = storage_path {
        args.push("--path".into());
        args.push(path);
    }

    let mut core = open()?;
    match core.dispatch(Request::new("app.import", args)) {
        Ok(_) => Ok(InstallOutcome::Installed { id, source }),
        Err(Error::AppExists(_)) => Ok(InstallOutcome::Refreshed { id }),
        Err(e) => Err(e.to_string()),
    }
}

/// `sync <app> --from <home>`: pull another replica's edits for one app and
/// merge them into this one.
pub fn sync_from_home(app: &str, from_home: &str) -> Result<SyncOutcome, String> {
    let source_home = PathBuf::from(from_home);
    let src_log = source_home.join("log.bin");
    let source = Core::open(&src_log).map_err(|e| format!("open --from {from_home}: {e}"))?;

    let mut local = open()?;
    let hex = crdt_export_hex(source.state(), app, local.state()).map_err(|e| e.to_string())?;
    let had_crdt = hex.is_some();
    let mut changed = false;
    if let Some(hex) = hex {
        let records = local
            .dispatch(Request::new("crdt.merge", vec![app.to_string(), hex]))
            .map_err(|e| e.to_string())?;
        changed |= !records.is_empty();
    }
    changed |= sync_blob_metadata(app, source.state(), &mut local)?;
    let hashes = terrane_cap_blob::live_hashes_for_app(&source.state().blob, app);
    crate::blob_store::copy_hashes_from_home(&home_dir(), &source_home, &hashes)
        .map_err(|e| e.to_string())?;

    if hashes.is_empty() && !changed && !had_crdt {
        return Ok(SyncOutcome::NothingToSync {
            app: app.to_string(),
            from_home: from_home.to_string(),
        });
    }
    if !changed {
        Ok(SyncOutcome::AlreadyUpToDate {
            from_home: from_home.to_string(),
        })
    } else {
        Ok(SyncOutcome::Synced {
            app: app.to_string(),
            from_home: from_home.to_string(),
        })
    }
}

fn sync_blob_metadata(
    app: &str,
    source: &terrane_core::State,
    local: &mut HostCore,
) -> Result<bool, String> {
    let Some(source_names) = source.blob.blobs.get(app) else {
        return Ok(false);
    };
    let mut changed = false;
    for (name, meta) in source_names {
        let same = local
            .state()
            .blob
            .blobs
            .get(app)
            .and_then(|names| names.get(name))
            .map(|local_meta| local_meta == meta)
            .unwrap_or(false);
        if same {
            continue;
        }
        let records = local
            .dispatch(Request::new(
                "blob.link",
                vec![
                    app.to_string(),
                    name.clone(),
                    meta.hash.clone(),
                    meta.size.to_string(),
                    meta.mime.clone(),
                ],
            ))
            .map_err(|e| e.to_string())?;
        changed |= !records.is_empty();
    }
    Ok(changed)
}

fn manifest_file_types_arg(file_types: &[FileTypeSpec]) -> Result<String, String> {
    let mut specs = Vec::new();
    for file_type in file_types {
        let ext = file_type.ext.trim().trim_start_matches('.');
        let mime = file_type.mime.trim();
        if ext.is_empty() && mime.is_empty() {
            continue;
        }
        let spec = format!("{ext}:{mime}");
        terrane_cap_app::validate_link_registration("filetype", &spec)
            .map_err(|e| e.to_string())?;
        specs.push(spec);
    }
    specs.sort();
    specs.dedup();
    Ok(specs.join(","))
}

/// The public API surface assembled from `terrane-api` and `terrane-core`
/// declarations, so it can't drift from the running system.
pub fn contract_surface() -> terrane_api::PublicSurface {
    let capability_docs = terrane_core::capability_docs(false)
        .into_iter()
        .map(|doc| cap_doc::capability_info(&doc.namespace, false))
        .collect::<Result<Vec<_>, _>>()
        .expect("capability docs should be exported from known namespaces");
    let mut grant_specs_by_namespace =
        BTreeMap::<String, Vec<terrane_api::GrantResourceSpecInfo>>::new();
    for spec in terrane_core::grant_resource_specs() {
        grant_specs_by_namespace
            .entry(spec.namespace.to_string())
            .or_default()
            .push(cap_doc::grant_spec_info(spec));
    }
    let resources = terrane_core::resource_surface()
        .into_iter()
        .map(|ns| terrane_api::ResourceNamespace {
            namespace: ns.namespace.to_string(),
            methods: ns
                .methods
                .into_iter()
                .map(|m| terrane_api::ResourceMethodInfo {
                    name: m.name.to_string(),
                    kind: m.kind.to_string(),
                    params: m.params.iter().map(|p| p.to_string()).collect(),
                })
                .collect(),
            grant_specs: grant_specs_by_namespace
                .remove(ns.namespace)
                .unwrap_or_default(),
        })
        .collect();
    let capabilities = terrane_core::capability_namespaces()
        .into_iter()
        .map(str::to_string)
        .collect();
    terrane_api::public_surface(capabilities, resources, capability_docs)
}

fn copy_dir(src: &Path, dest: &Path) -> std::io::Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

#[derive(Debug, Clone, DeJson)]
pub struct BundleManifest {
    #[nserde(default)]
    pub id: String,
    #[nserde(default)]
    pub name: String,
    #[nserde(default)]
    pub runtime: String,
    #[nserde(default)]
    pub backend: String,
    #[nserde(default)]
    pub module: String,
    #[nserde(default)]
    pub entry: String,
    #[nserde(default)]
    pub ui: String,
    #[nserde(default)]
    pub icon: String,
    #[nserde(default)]
    pub resources: Vec<String>,
    #[nserde(default)]
    pub interfaces: Vec<String>,
    #[nserde(default, rename = "fileTypes")]
    pub file_types: Vec<FileTypeSpec>,
    #[nserde(default)]
    pub browser_permissions: Vec<String>,
}

#[derive(Debug, Clone, DeJson)]
pub struct FileTypeSpec {
    #[nserde(default)]
    pub ext: String,
    #[nserde(default)]
    pub mime: String,
}

pub fn read_manifest(bundle_dir: &Path) -> Result<BundleManifest, Error> {
    let text = std::fs::read_to_string(bundle_dir.join("manifest.json"))
        .map_err(|e| Error::Runtime(format!("read manifest.json: {e}")))?;
    let manifest = BundleManifest::deserialize_json(&text)
        .map_err(|e| Error::Runtime(format!("manifest.json: {e}")))?;
    Ok(BundleManifest {
        runtime: non_empty_or(manifest.runtime, "js"),
        ..manifest
    })
}

pub fn validate_common_api_bundle(path: &Path) -> Result<(), String> {
    let manifest = read_manifest(path).map_err(|e| e.to_string())?;
    if manifest.runtime != "js" {
        return Ok(());
    }
    let id = manifest.id.trim();
    validate_bundle_id(&path.display().to_string(), id)?;
    let backend = path.join(&manifest.backend);
    let source = std::fs::read_to_string(&backend)
        .map_err(|e| format!("read backend {}: {e}", backend.display()))?;
    validate_common_api_bundle_source(
        id,
        non_empty_or(manifest.name.clone(), id),
        source,
        manifest.resources.clone(),
    )
}

pub fn validate_common_api_bundle_source(
    id: &str,
    name: String,
    source: String,
    resources: Vec<String>,
) -> Result<(), String> {
    let bundle = JsRuntimeBundle {
        source,
        name: name.clone(),
        resources: resources.clone(),
    };
    let mut state = terrane_core::State::default();
    state.app.apps.insert(
        id.to_string(),
            terrane_cap_app::AppRecord {
                id: id.to_string(),
                name,
                source: None,
                runtime: "js".to_string(),
                interfaces: terrane_cap_app::mandatory_interfaces(),
                links: Vec::new(),
            },
        );
    let host = RuntimeHostHandle::new(Box::new(
        RuntimeResourceHost::new_with_temporary_resource_grants(
            id.to_string(),
            state,
            ExecutionPrincipal::local_owner(),
            resources,
        ),
    ));
    let actions = run_js_bundle(id, &["__actions__".to_string()], &bundle, host.clone())
        .map_err(|e| format!("common API __actions__ probe failed: {e}"))?;
    require_common_actions(&actions)?;
    let list = run_js_bundle(id, &["common.list".to_string()], &bundle, host.clone())
        .map_err(|e| format!("common.list probe failed: {e}"))?;
    let list_value: serde_json::Value =
        serde_json::from_str(&list).map_err(|e| format!("common.list must return JSON: {e}"))?;
    if !list_value.is_array() {
        return Err("common.list must return a JSON array".to_string());
    }
    let bogus = "__terrane_validation_missing_item__";
    let get = run_js_bundle(
        id,
        &["common.get".to_string(), bogus.to_string()],
        &bundle,
        host.clone(),
    )
    .map_err(|e| format!("common.get bogus-id probe failed: {e}"))?;
    let get_value: serde_json::Value =
        serde_json::from_str(&get).map_err(|e| format!("common.get not-found must be JSON: {e}"))?;
    let code = get_value
        .pointer("/error/code")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if code != "NotFound" {
        return Err("common.get bogus id must return typed NotFound JSON".to_string());
    }
    run_js_bundle(
        id,
        &[
            "common.receive".to_string(),
            "json".to_string(),
            "{}".to_string(),
        ],
        &bundle,
        host,
    )
    .map_err(|e| format!("common.receive probe failed: {e}"))?;
    Ok(())
}

fn require_common_actions(actions: &str) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(actions).map_err(|e| format!("__actions__ must return JSON: {e}"))?;
    let verbs = value
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "__actions__ must return an actions array".to_string())?;
    for required in ["common.receive", "common.list", "common.get"] {
        if !verbs
            .iter()
            .any(|entry| entry.get("verb").and_then(serde_json::Value::as_str) == Some(required))
        {
            return Err(format!("bundle must declare required verb {required}"));
        }
    }
    Ok(())
}

/// App ids become directory names under `$TERRANE_HOME/apps`, so keep them as
/// one portable path segment.
pub(crate) fn validate_bundle_id(path: &str, id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err(format!("{path}/manifest.json has no \"id\""));
    }
    if id == "." || id == ".." {
        return Err(format!("{path}/manifest.json has unsafe \"id\": {id:?}"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(format!(
            "{path}/manifest.json has unsafe \"id\": {id:?}; use ASCII letters, digits, '-' or '_'"
        ));
    }
    Ok(())
}

pub(crate) fn validate_runtime(path: &str, runtime: &str) -> Result<(), String> {
    match runtime {
        "js" | "wasm" => Ok(()),
        other => Err(format!(
            "{path}/manifest.json has unsupported runtime {other:?}; use \"js\" or \"wasm\""
        )),
    }
}

fn non_empty_or(value: String, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value
    }
}
