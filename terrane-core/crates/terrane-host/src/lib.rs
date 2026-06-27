//! terrane-host — shared operational host spine.
//!
//! This crate owns the parts every host needs: opening a Terrane home, minting a
//! replica identity, installing bundles, running app backends, syncing replicas,
//! and exposing the public contract surface. Transport adapters such as the CLI,
//! HTTP server, MCP server, and FFI layer should stay thin and call into here.

use std::env;
use std::path::{Path, PathBuf};

use terrane_api::{AppSummary, AppsResponse};
use terrane_core::cap::builder;
use terrane_core::Core;
use terrane_domain::{Error, EventRecord, Request};

pub mod edge;
pub mod mcp;
pub mod preview;
pub mod sync;

pub use edge::EdgeRunner;
pub use preview::{PreviewAsset, PreviewCreated, PreviewFile, PreviewStore};
pub use sync::{serve_conn, sync_conn};

pub type HostCore = Core<EdgeRunner>;

/// Default address `terrane serve` binds when none is given.
pub const DEFAULT_SERVE_ADDR: &str = "127.0.0.1:7777";

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub records: Vec<EventRecord>,
    pub output: Option<String>,
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
    let records = core
        .dispatch(Request::new(command, args.to_vec()))
        .map_err(|e| e.to_string())?;
    Ok(CommandOutcome {
        records,
        output: core.take_last_output(),
    })
}

/// Run an app backend and return the backend output string.
pub fn invoke_app(
    core: &mut HostCore,
    app: &str,
    verb: &str,
    args: &[String],
) -> Result<String, String> {
    if !core.state().app.apps.contains_key(app) {
        return Err(format!("no such app: {app}"));
    }
    let mut argv = Vec::with_capacity(args.len() + 2);
    argv.push(app.to_string());
    argv.push(verb.to_string());
    argv.extend(args.iter().cloned());
    Ok(dispatch_on_core(core, "host.run", &argv)?
        .output
        .unwrap_or_default())
}

/// Return an app's self-declared action metadata by invoking its reserved
/// `__actions__` verb.
pub fn app_actions(core: &mut HostCore, app: &str) -> Result<String, String> {
    invoke_app(core, app, terrane_api::ACTIONS_VERB, &[])
}

/// Ask the core builder capability to generate a draft app and return the
/// latest draft as JSON for host bridges.
pub fn generate_app_json(
    core: &mut HostCore,
    app_id: &str,
    name: &str,
    prompt: &str,
    agent: Option<&str>,
) -> Result<String, String> {
    let draft_id = app_id.trim();
    let agent = agent
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(builder::DEFAULT_AGENT);
    dispatch_on_core(
        core,
        "builder.generate",
        &[
            draft_id.to_string(),
            app_id.to_string(),
            name.to_string(),
            agent.to_string(),
            prompt.to_string(),
        ],
    )?;
    let draft = core
        .state()
        .builder
        .drafts
        .get(draft_id)
        .ok_or_else(|| format!("builder draft missing after generation: {draft_id}"))?;
    Ok(builder::draft_json(draft))
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
    terrane_core::cap::host::read_manifest(Path::new(source))
        .ok()
        .map(|m| m.ui)
        .filter(|ui| !ui.is_empty())
}

/// `app install <path>`: copy a bundle into this home's `apps/<id>/` and catalog
/// it from there, so the home owns the app and no longer depends on the install
/// command's working directory.
pub fn install_app(path: &str) -> Result<InstallOutcome, String> {
    let src = Path::new(path);
    let manifest = terrane_core::cap::host::read_manifest(src).map_err(|e| e.to_string())?;
    let id = manifest.id.trim().to_string();
    validate_bundle_id(path, &id)?;
    let name = match manifest.name.trim() {
        "" => id.clone(),
        name => name.to_string(),
    };

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
        vec![id.clone(), name, "--source".into(), source.clone()],
    )) {
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
    let hex = terrane_core::cap::crdt::crdt_export_hex(source.state(), app, local.state())
        .map_err(|e| e.to_string())?;
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
        })
        .collect();
    let capabilities = terrane_core::capability_namespaces()
        .into_iter()
        .map(str::to_string)
        .collect();
    terrane_api::public_surface(capabilities, resources)
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

/// App ids become directory names under `$TERRANE_HOME/apps`, so keep them as
/// one portable path segment.
fn validate_bundle_id(path: &str, id: &str) -> Result<(), String> {
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
