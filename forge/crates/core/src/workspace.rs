//! [`WorkspaceCore`]: the command/event facade that wires the entire M0a spine.
//!
//! prd-merged/01 CR-A1..A5 (Command / Event / Response; every command carries an
//! [`ActorContext`] and passes policy before touching state) + prd-merged/04
//! P-04 command catalog (the string command names in `forge/spec/commands.md`).
//!
//! A single [`handle`](WorkspaceCore::handle) turns a [`CoreCommand`] into a
//! [`CoreResponse`], driving the jewel end-to-end:
//!
//!   `applet.install` (TS → SWC transpile + policy scan → store) and
//!   `runtime.run` (QuickJS → capability-checked `ctx` → SQLite write → UI patch
//!   → recorded [`RunRecord`]) and `runtime.replay` (deterministic re-execution,
//!   asserted byte-identical) and `query.execute` (read the records projection).
//!
//! The code-hash that flows through this facade is the single canonical
//! `forge_domain::code_hash` (`sha256:`): the pipeline computes it over the
//! transpiled JS, and the runtime records exactly that hash on the
//! [`RunRecord`] (it hashes the same `js_code` bytes the pipeline produced), so a
//! stored run's `code_hash` is provably the pipeline's hash — the TS → SWC → run
//! provenance chain (reviews 010/012/013/014).

use crate::bridge::StorageHostBridge;
use crate::event::EventSink;
use forge_domain::{
    AppletId, CoreCommand, CoreError, CoreResponse, Manifest, Result, RunRecord,
};
use forge_runtime::{record_run, replay, NullBridge, Program as RuntimeProgram};
use forge_schema::SchemaRegistry;
use forge_storage::Store;

/// Reserved KV namespace prefix for core-owned metadata (applet manifests +
/// compiled programs + workspace meta). Applet `ctx.storage` namespaces are
/// `applet/<id>` (see [`StorageHostBridge`]), which never collide with this
/// `__forge/...` prefix.
const META_NS: &str = "__forge/meta";

/// The deterministic seeds the spine uses for a run's time/random seams in M0a.
/// Fixed so a fresh run is itself reproducible; a later milestone threads these
/// from the command/run profile.
const DEFAULT_RANDOM_SEED: u64 = 0x5EED;
const DEFAULT_TIME_START: u64 = 0;

/// The compiled, installed form of an applet: its manifest plus the transpiled
/// JS the runtime executes and the canonical `code_hash` the pipeline produced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct InstalledApplet {
    manifest: Manifest,
    /// Transpiled ES-module JavaScript (the runtime's `Program.source`).
    js_code: String,
    /// `forge_domain::code_hash(js_code)` — the provenance + replay key.
    code_hash: String,
    /// Monotone install version (bumps on re-install/upgrade).
    version: u32,
}

/// The workspace facade. Owns the SQLite [`Store`], a [`SchemaRegistry`], and an
/// [`EventSink`]; a [`forge_runtime::QuickJsEngine`] is constructed per run
/// inside the runtime's `record_run`/`replay`.
pub struct WorkspaceCore {
    store: Store,
    registry: SchemaRegistry,
    events: EventSink,
    workspace_id: String,
}

impl WorkspaceCore {
    /// Open (or create) a file-backed workspace at `path` (`workspace.open`
    /// semantics; the single portable SQLite file, DECISIONS E1).
    pub fn open(path: impl AsRef<std::path::Path>, workspace_id: impl Into<String>) -> Result<Self> {
        Ok(WorkspaceCore {
            store: Store::open(path)?,
            registry: SchemaRegistry::new(),
            events: EventSink::new(),
            workspace_id: workspace_id.into(),
        })
    }

    /// Open an in-memory workspace (tests/scratch).
    pub fn in_memory(workspace_id: impl Into<String>) -> Result<Self> {
        Ok(WorkspaceCore {
            store: Store::open_in_memory()?,
            registry: SchemaRegistry::new(),
            events: EventSink::new(),
            workspace_id: workspace_id.into(),
        })
    }

    /// The event sink (observability): all `run.*` / `ui.patch` events land here.
    pub fn events(&self) -> &EventSink {
        &self.events
    }

    /// Mutable access to the event sink (e.g. to drain it between turns).
    pub fn events_mut(&mut self) -> &mut EventSink {
        &mut self.events
    }

    /// The schema registry backing `schema.*` / record validation.
    pub fn registry(&self) -> &SchemaRegistry {
        &self.registry
    }

    /// Borrow the underlying store (read-only access for tests / queries).
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Mutable access to the underlying store, for in-process tooling and tests
    /// (e.g. the testkit injecting/​tampering a record to exercise replay
    /// divergence). This is **not** a shell-facing path: the CR-A1 boundary
    /// forbids *shells* from mutating SQLite directly, which they do not get via
    /// the binding layer; in-process callers that hold a `WorkspaceCore` already
    /// own the workspace.
    pub fn store_mut(&mut self) -> &mut Store {
        &mut self.store
    }

    // ---------------------------------------------------------------- dispatch

    /// Handle one [`CoreCommand`], returning a [`CoreResponse`]
    /// (prd-merged/01 CR-A1/CR-A2). Unknown command names return a
    /// `ValidationError` response rather than panicking (CR-A5 graceful reject).
    ///
    /// Every command carries an [`ActorContext`]; the policy gate is enforced at
    /// host-call time inside `runtime.run`/`replay` (CR-A3 / CR-4), where the
    /// applet's manifest capabilities are checked per `ctx.*` call.
    pub fn handle(&mut self, cmd: CoreCommand) -> CoreResponse {
        let request_id = cmd.request_id.clone();
        let result = match cmd.name.as_str() {
            "workspace.create" => self.cmd_workspace_create(&cmd),
            "workspace.open" => self.cmd_workspace_open(&cmd),
            "applet.install" => self.cmd_applet_install(&cmd),
            "runtime.run" => self.cmd_runtime_run(&cmd),
            "runtime.replay" => self.cmd_runtime_replay(&cmd),
            "query.execute" => self.cmd_query_execute(&cmd),
            other => Err(CoreError::ValidationError(format!(
                "unknown command {other:?} (CR-A5: client should negotiate capability)"
            ))),
        };
        match result {
            Ok(payload) => CoreResponse::ok(request_id, payload),
            Err(error) => CoreResponse::err(request_id, error),
        }
    }

    // ---------------------------------------------------------------- commands

    /// `workspace.create` — in M0a the store is created on open, so this reports
    /// the workspace identity + the base logical version (CR-A2; M0b adds
    /// templates/owner wiring).
    fn cmd_workspace_create(&mut self, _cmd: &CoreCommand) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "root_version": 0,
        }))
    }

    /// `workspace.open` — report workspace metadata + the current logical clock
    /// (CR-A2). The file is already open (this core wraps one workspace file).
    fn cmd_workspace_open(&mut self, _cmd: &CoreCommand) -> Result<serde_json::Value> {
        Ok(serde_json::json!({
            "workspace_id": self.workspace_id,
            "logical_clock": self.events.len(),
        }))
    }

    /// `applet.install` — compile each source (static policy scan + SWC
    /// transpile; reject forbidden constructs), validate the manifest, and store
    /// the manifest + transpiled program (CR-A2, CR-13/CR-14, SC-15).
    ///
    /// Payload: `{ applet_id, manifest, sources: { "<path>": "<ts>" } }`. The
    /// manifest's `entrypoint` selects which source is the runnable program.
    fn cmd_applet_install(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let manifest: Manifest = take_field(cmd, "manifest")?;
        manifest.validate()?;

        let sources = cmd
            .payload
            .get("sources")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                CoreError::ValidationError("applet.install requires a `sources` object".into())
            })?;
        if sources.is_empty() {
            return Err(CoreError::ValidationError(
                "applet.install `sources` must not be empty".into(),
            ));
        }

        // Compile every source so a forbidden construct in ANY file rejects the
        // whole install (CR-13: the static policy scan is layer one). Capture
        // each compiled program; the entrypoint's program is the runnable one.
        let mut warnings = Vec::new();
        let mut entry_program: Option<forge_pipeline::Program> = None;
        for (path, src) in sources {
            let ts = src.as_str().ok_or_else(|| {
                CoreError::ValidationError(format!("source {path:?} must be a string"))
            })?;
            // compile() runs enforce_policy (PermissionDenied on eval/Function/…)
            // THEN transpiles; a forbidden construct never reaches transpile.
            let program = forge_pipeline::compile(ts)?;
            if path == &manifest.entrypoint {
                entry_program = Some(program);
            }
        }
        let entry_program = entry_program.ok_or_else(|| {
            CoreError::ValidationError(format!(
                "manifest.entrypoint {:?} is not among the provided sources",
                manifest.entrypoint
            ))
        })?;

        // Bump version if re-installing.
        let version = self
            .load_applet(applet_id.as_str())
            .ok()
            .flatten()
            .map(|a| a.version + 1)
            .unwrap_or(1);

        let installed = InstalledApplet {
            manifest,
            js_code: entry_program.js_code,
            code_hash: entry_program.code_hash,
            version,
        };
        self.store_applet(applet_id.as_str(), &installed)?;

        if sources.len() > 1 {
            warnings.push(format!(
                "{} non-entrypoint source(s) compiled but only the entrypoint is runnable in M0a",
                sources.len() - 1
            ));
        }

        self.events.emit(
            Some(applet_id.clone()),
            "applet.installed",
            serde_json::json!({ "applet_id": applet_id, "version": version }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "version": version,
            "code_hash": installed.code_hash,
            "warnings": warnings,
        }))
    }

    /// `runtime.run` — load the compiled program, run it via the QuickJS engine
    /// in record mode with a [`StorageHostBridge`] + the applet's policy
    /// (manifest capabilities), save the [`RunRecord`], emit
    /// `run.started`/`ui.patch`/`run.completed`, and return the run summary +
    /// `AppResult` (CR-A2, CR-8, CR-9).
    ///
    /// Payload: `{ applet_id, input }`.
    fn cmd_runtime_run(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let input = cmd.payload.get("input").cloned().unwrap_or(serde_json::Value::Null);
        let installed = self
            .load_applet(applet_id.as_str())?
            .ok_or_else(|| {
                CoreError::ValidationError(format!("applet {applet_id} is not installed"))
            })?;

        self.events.emit(
            Some(applet_id.clone()),
            "run.started",
            serde_json::json!({ "applet_id": applet_id, "code_hash": installed.code_hash }),
        );

        let program = RuntimeProgram::new(applet_id.clone(), installed.js_code.clone());
        // Sanity: the runtime hashes the SAME js bytes the pipeline did, so its
        // recorded code_hash equals the pipeline's. Asserted in the integration
        // test; here we surface a clear error if that invariant ever breaks.
        if program.code_hash() != installed.code_hash {
            return Err(CoreError::RuntimeError(format!(
                "code_hash provenance broken: runtime {} != pipeline {}",
                program.code_hash(),
                installed.code_hash
            )));
        }

        // Run in record mode against the live Store-backed bridge. The bridge
        // performs the SQLite writes / UI diffs; the runtime's HostContext gates
        // each ctx.* call against the manifest policy BEFORE the bridge runs.
        let mut bridge = StorageHostBridge::new(&mut self.store, applet_id.as_str());
        let run = record_run(
            &program,
            &installed.manifest,
            &cmd.actor,
            &input,
            DEFAULT_RANDOM_SEED,
            DEFAULT_TIME_START,
            &mut bridge,
        )?;
        // Drain the captured UI renders + logs before dropping the bridge (which
        // releases the &mut Store borrow so we can save the run).
        let ui_renders = std::mem::take(&mut bridge.ui_renders);
        drop(bridge);

        // Persist the deterministic run record (replay source, CR-9). save_run
        // re-validates the canonical code_hash.
        self.store.save_run(&run)?;

        // Emit a ui.patch event per render (the UI tree patch link).
        for (i, render) in ui_renders.iter().enumerate() {
            self.events.emit(
                Some(applet_id.clone()),
                "ui.patch",
                serde_json::json!({
                    "applet_id": applet_id,
                    "render_index": i,
                    "tree": render.tree,
                    "patches": render.patches,
                }),
            );
        }

        let summary = run_summary(&run);
        let (ok, app_result) = outcome_fields(&run);
        let event_kind = if ok { "run.completed" } else { "run.failed" };
        self.events.emit(
            Some(applet_id.clone()),
            event_kind,
            serde_json::json!({ "run_id": run.run_id, "ok": ok }),
        );

        Ok(serde_json::json!({
            "run_id": run.run_id,
            "code_hash": run.code_hash,
            "ok": ok,
            "result": app_result,
            "summary": summary,
            "ui_renders": ui_renders.iter().map(|r| r.tree.clone()).collect::<Vec<_>>(),
        }))
    }

    /// `runtime.replay` — load the stored [`RunRecord`], replay it deterministically
    /// (the recorder serves recorded responses; the live bridge is a
    /// [`NullBridge`] that must never be consulted), and assert the replay is
    /// byte-identical to the original (CR-A2, CR-9). Divergence → `RuntimeError`.
    ///
    /// Payload: `{ run_id }`.
    fn cmd_runtime_replay(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let run_id = cmd
            .payload
            .get("run_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::ValidationError("runtime.replay requires `run_id`".into()))?
            .to_string();

        let original = self
            .store
            .load_run(&run_id)?
            .ok_or_else(|| CoreError::ValidationError(format!("run {run_id} not found")))?;

        let installed = self
            .load_applet(original.applet_id.as_str())?
            .ok_or_else(|| {
                CoreError::ValidationError(format!(
                    "applet {} for run {run_id} is not installed; cannot replay",
                    original.applet_id
                ))
            })?;

        let program = RuntimeProgram::new(original.applet_id.clone(), installed.js_code.clone());
        let mut null = NullBridge::new();
        let replayed = replay(&original, &program, &installed.manifest, &cmd.actor, &mut null)?;

        // The strict replay check: canonical provenance on both records AND
        // byte-identical traces, surfaced as a RuntimeError on divergence.
        original.assert_replay_of(&replayed)?;

        self.events.emit(
            Some(original.applet_id.clone()),
            "run.replayed",
            serde_json::json!({ "run_id": run_id, "ok": true }),
        );

        Ok(serde_json::json!({
            "ok": true,
            "run_id": run_id,
            "fingerprint": replayed.replay_fingerprint(),
            "replays_identically": original.replays_identically(&replayed),
        }))
    }

    /// `query.execute` — list every record in `collection` from the projection
    /// (CR-A2, DL-15 subset). Payload: `{ collection }`.
    fn cmd_query_execute(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let collection = cmd
            .payload
            .get("collection")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError("query.execute requires `collection`".into())
            })?;
        let records = self.store.list_records(collection)?;
        let rows: Vec<serde_json::Value> = records
            .into_iter()
            .map(|env| {
                serde_json::json!({
                    "id": env.entity_id,
                    "fields": env.fields,
                })
            })
            .collect();
        Ok(serde_json::json!({ "collection": collection, "rows": rows }))
    }

    // ----------------------------------------------------------- applet store

    /// Persist an installed applet (manifest + compiled program) in the reserved
    /// meta KV namespace.
    fn store_applet(&mut self, applet_id: &str, installed: &InstalledApplet) -> Result<()> {
        let bytes = serde_json::to_vec(installed).map_err(|e| {
            CoreError::StorageError(format!("applet serialize failed: {e}"))
        })?;
        self.store
            .kv_set(META_NS, &applet_key(applet_id), &bytes, "application/json")
    }

    /// Load an installed applet by id, if present.
    fn load_applet(&self, applet_id: &str) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &applet_key(applet_id))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("applet deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }
}

/// KV key for an applet's installed record within [`META_NS`].
fn applet_key(applet_id: &str) -> String {
    format!("applet/{applet_id}")
}

/// Extract and require the command's `applet_id` (from the envelope, or the
/// payload as a fallback).
fn require_applet_id(cmd: &CoreCommand) -> Result<AppletId> {
    if let Some(id) = &cmd.applet_id {
        return Ok(id.clone());
    }
    cmd.payload
        .get("applet_id")
        .and_then(|v| v.as_str())
        .map(AppletId::new)
        .ok_or_else(|| CoreError::ValidationError(format!("{} requires an applet_id", cmd.name)))
}

/// Deserialize a required object field from the command payload.
fn take_field<T: serde::de::DeserializeOwned>(cmd: &CoreCommand, field: &str) -> Result<T> {
    let value = cmd.payload.get(field).ok_or_else(|| {
        CoreError::ValidationError(format!("{} requires a `{field}` field", cmd.name))
    })?;
    serde_json::from_value(value.clone()).map_err(|e| {
        CoreError::ValidationError(format!("{} `{field}` is malformed: {e}", cmd.name))
    })
}

/// `(ok, app_result_json)` for a run's outcome.
fn outcome_fields(run: &RunRecord) -> (bool, serde_json::Value) {
    use forge_domain::RunOutcome;
    match &run.outcome {
        RunOutcome::Completed { result } => {
            (result.ok, serde_json::to_value(result).unwrap_or(serde_json::Value::Null))
        }
        RunOutcome::Failed { error } => {
            (false, serde_json::json!({ "error": error }))
        }
    }
}

/// A compact summary of a run for the response payload + observability.
fn run_summary(run: &RunRecord) -> serde_json::Value {
    serde_json::json!({
        "run_id": run.run_id,
        "applet_id": run.applet_id,
        "code_hash": run.code_hash,
        "calls": run.calls.len(),
        "logs": run.logs.len(),
        "completed": run.is_completed(),
    })
}
