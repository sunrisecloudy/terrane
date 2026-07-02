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
use terrane_cap_kv::app_bundle_source;
use terrane_core::Core;
use terrane_core::{Decision, Error, EventRecord, QueryValue, Request};

pub mod cap_doc;
pub mod cli;
pub mod edge;
pub mod ffi;
pub mod home;
mod local_llm;
pub mod mcp;
pub mod native;
pub mod permission;
pub mod preview;
pub mod public_authz;
pub mod sync;

pub use edge::EdgeRunner;
pub use home::{home_page, HomePageOptions};
pub use preview::{PreviewAsset, PreviewCreated, PreviewFile, PreviewStore};

/// Release in-process local-model engines. Hosts (and the CLI) call this once
/// before a normal exit; safe to call when nothing is cached.
pub fn local_llm_shutdown() {
    local_llm::shutdown();
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
    let mut core = Core::open_with(log_path.into(), EdgeRunner).map_err(|e| e.to_string())?;
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
        Decision::Effect(_) => Err(format!(
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

pub fn list_apps(core: &HostCore) -> AppsResponse {
    let apps = core
        .state()
        .app
        .apps
        .values()
        .map(|app| AppSummary {
            id: app.id.clone(),
            name: app.name.clone(),
            has_ui: app_has_ui(app.source.as_deref()),
        })
        .collect();
    AppsResponse { apps }
}

pub fn app_has_ui(source: Option<&str>) -> bool {
    source.and_then(read_manifest_ui).is_some()
}

/// The app's declared UI entry file (`manifest.ui`), if any.
pub fn read_manifest_ui(source: &str) -> Option<String> {
    read_manifest(Path::new(source))
        .ok()
        .map(|m| m.ui)
        .filter(|ui| !ui.is_empty())
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
    match core.dispatch(Request::new(
        "app.add",
        vec![
            id.clone(),
            name,
            "--source".into(),
            source.clone(),
            "--runtime".into(),
            runtime,
        ],
    )) {
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
    let src_log = PathBuf::from(from_home).join("log.bin");
    let source = Core::open(&src_log).map_err(|e| format!("open --from {from_home}: {e}"))?;

    let mut local = open()?;
    let hex = crdt_export_hex(source.state(), app, local.state()).map_err(|e| e.to_string())?;
    let Some(hex) = hex else {
        return Ok(SyncOutcome::NothingToSync {
            app: app.to_string(),
            from_home: from_home.to_string(),
        });
    };

    let records = local
        .dispatch(Request::new("crdt.merge", vec![app.to_string(), hex]))
        .map_err(|e| e.to_string())?;
    if records.is_empty() {
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
    pub resources: Vec<String>,
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
