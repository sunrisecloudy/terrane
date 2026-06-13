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
    AppletId, CoreCommand, CoreError, CoreResponse, Manifest, Result, Role, RunId, RunRecord,
};
use forge_runtime::{record_run, replay, NullBridge, Program as RuntimeProgram};
use forge_schema::SchemaRegistry;
use forge_storage::Store;

/// Reserved KV namespace prefix for core-owned metadata (applet manifests +
/// compiled programs + workspace meta). Applet `ctx.storage` namespaces are
/// `applet/<id>` (see [`StorageHostBridge`]), which never collide with this
/// `__forge/...` prefix.
const META_NS: &str = "__forge/meta";

/// The KV key (within [`META_NS`]) holding the workspace's monotone run counter.
/// Bumped once per `runtime.run` to mint a unique per-execution `run_id` while
/// the replay *seeds* stay a deterministic function of `(code_hash, input)`
/// (review 031 finding 2 / CR-9 "every execution persists").
const RUN_COUNTER_KEY: &str = "run_counter";

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
    /// Every command carries an [`ActorContext`]; CR-A3 requires that context to
    /// pass policy *before* the command touches state. This happens in two
    /// layers: a **command-level RBAC gate** ([`authorize`]) runs here, before
    /// dispatch, rejecting an actor whose role is not permitted to issue the
    /// command per `forge/spec/commands.md` (e.g. a Viewer cannot
    /// `applet.install`, a Runner cannot `runtime.replay`); then the
    /// **capability gate** is enforced at host-call time inside
    /// `runtime.run`/`replay` (CR-A3 / CR-4), where the applet's manifest
    /// capabilities are checked per `ctx.*` call.
    pub fn handle(&mut self, cmd: CoreCommand) -> CoreResponse {
        let request_id = cmd.request_id.clone();
        let result = authorize(&cmd).and_then(|()| match cmd.name.as_str() {
            "workspace.create" => self.cmd_workspace_create(&cmd),
            "workspace.open" => self.cmd_workspace_open(&cmd),
            "applet.install" => self.cmd_applet_install(&cmd),
            "runtime.run" => self.cmd_runtime_run(&cmd),
            "runtime.replay" => self.cmd_runtime_replay(&cmd),
            "query.execute" => self.cmd_query_execute(&cmd),
            other => Err(CoreError::ValidationError(format!(
                "unknown command {other:?} (CR-A5: client should negotiate capability)"
            ))),
        });
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
    /// Payload: `{ applet_id, input, random_seed?, time_start? }`.
    ///
    /// `random_seed`/`time_start` are **optional** deterministic-seam overrides
    /// (review 032 finding 1). When present they pin the run's RNG/clock seeds to
    /// exact values — the conformance corpus uses this to drive a scenario
    /// recorded under specific seeds (e.g. `seeded_random` under seed 7) through
    /// this facade rather than a parallel direct path. When absent the seeds are a
    /// deterministic function of `(code_hash, input)` (the default that makes
    /// independent re-runs of the demo replay identically; review 031 finding 2).
    fn cmd_runtime_run(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let input = cmd.payload.get("input").cloned().unwrap_or(serde_json::Value::Null);
        let seed_override = run_seed_override(cmd)?;
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

        // Replay seeds are a deterministic function of (code_hash, input): a
        // re-run with the SAME applet code and input reproduces the SAME seeded
        // time/random seams, so the two records replay byte-identically. A
        // different input yields different seeds (a genuinely different run).
        // An explicit `(random_seed, time_start)` in the payload overrides this
        // default so a conformance scenario recorded under fixed seeds can be
        // reproduced through the facade (review 032 finding 1). Either way the
        // chosen seeds are recorded on the run, so replay stays byte-identical.
        let (random_seed, time_start) =
            seed_override.unwrap_or_else(|| derive_seeds(&installed.code_hash, &input));

        // Mint a UNIQUE per-execution run id (review 031 finding 2 / CR-9). The
        // counter is persisted in workspace meta so each execution is saved and
        // loadable separately, even when code+input (and therefore the seeds)
        // match a prior run. This bump happens before the run so the id is
        // reserved even if the run itself fails.
        let invocation = self.next_run_counter()?;

        // Run in record mode against the live Store-backed bridge. The bridge
        // performs the SQLite writes / UI diffs; the runtime's HostContext gates
        // each ctx.* call against the manifest policy BEFORE the bridge runs.
        let mut bridge = StorageHostBridge::new(&mut self.store, applet_id.as_str());
        let mut run = record_run(
            &program,
            &installed.manifest,
            &cmd.actor,
            &input,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // Drain the captured UI renders + logs before dropping the bridge (which
        // releases the &mut Store borrow so we can save the run).
        let ui_renders = std::mem::take(&mut bridge.ui_renders);
        drop(bridge);

        // Override the runtime's deterministic run_id with the unique per-core
        // identity so two executions of the same applet+input persist as two
        // distinct, independently loadable records (the seeds/trace are
        // unchanged, so each still replays identically to itself).
        run.run_id = unique_run_id(&run.code_hash, invocation);

        // Pin the replay artifact PER RUN: persist the EXACT compiled program +
        // manifest this run executed, keyed by `run_id`, so replay reconstructs
        // the program AND the manifest (engine limits/capabilities) from what this
        // run actually used — never from whatever is installed now (review 031
        // finding 3 / review 036 finding 2; CR-9 version-pinned replay).
        //
        // Review 036 finding 2: the prior pin was keyed by `code_hash` alone, so
        // reinstalling the SAME JS under a different manifest (tighter `limits`,
        // changed legacy caps) overwrote `program/<code_hash>` and stranded older
        // runs' context — replay then used the new manifest's engine limits. The
        // per-run key is unique to this execution, so no reinstall (same code or
        // not) can overwrite it. The content-addressed `program/<code_hash>` pin
        // is kept as a fallback for legacy runs recorded before per-run pinning.
        self.store_run_program(run.run_id.as_str(), &installed)?;
        self.store_program(&installed)?;

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
            // The ordered host-call method trace the run issued (`db.insert`,
            // `ui.render`, …). Surfaced so a shell / conformance harness can
            // assert the exact effect sequence through the facade rather than
            // re-reading the persisted RunRecord (review 032 finding 1).
            "host_call_methods": run.calls.iter().map(|c| c.method.clone()).collect::<Vec<_>>(),
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

        // Version-pinned replay (review 031 finding 3, review 036 finding 2):
        // reconstruct the program + manifest from the artifact recorded for THIS
        // execution, not the currently installed applet. Resolution order:
        //   1. the PER-RUN pin (`program/run/<run_id>`) — unique to this run, so a
        //      reinstall under a different manifest cannot overwrite or alter it
        //      (the review 036 finding 2 case);
        //   2. the content-addressed `program/<code_hash>` pin — covers runs
        //      recorded before per-run pinning existed;
        //   3. the currently installed applet — last-resort legacy fallback, and
        //      only if its code_hash still matches the recorded one.
        let replay_artifact = match self.load_run_program(&run_id)? {
            Some(p) => p,
            None => match self.load_program(&original.code_hash)? {
                Some(p) => p,
                None => {
                    let installed =
                        self.load_applet(original.applet_id.as_str())?.ok_or_else(|| {
                            CoreError::ValidationError(format!(
                                "no recorded program for run {run_id} (code_hash {}) and applet {} is not installed; cannot replay",
                                original.code_hash, original.applet_id
                            ))
                        })?;
                    if installed.code_hash != original.code_hash {
                        return Err(CoreError::ValidationError(format!(
                            "no recorded program for run {run_id}; installed applet {} is a different version (code_hash {} != recorded {}); cannot replay",
                            original.applet_id, installed.code_hash, original.code_hash
                        )));
                    }
                    installed
                }
            },
        };
        let installed = replay_artifact;

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
    ///
    /// `forge/spec/commands.md:21` requires **"Role plus db.read capability"**:
    /// the command-level [`authorize`] role gate is necessary but not sufficient
    /// (review 036 finding 1). Before any records are listed, the actor must
    /// actually hold the `db.read` capability — modeled in M0a as a role-derived
    /// capability (see [`role_has_db_read`]). This is a distinct layer from the
    /// role allowlist: a role reachable at the command level but lacking `db.read`
    /// (e.g. a `Runner`, which is execution-only) is denied here before
    /// `list_records` touches state, mirroring the per-`ctx.*` `db.read` gate the
    /// runtime enforces for applet reads.
    fn cmd_query_execute(&mut self, cmd: &CoreCommand) -> Result<serde_json::Value> {
        let collection = cmd
            .payload
            .get("collection")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError("query.execute requires `collection`".into())
            })?;
        // Capability gate (CR-A3 / DL-15): the actor must hold `db.read` before
        // any projection is read. Denied with PermissionDenied, not silently
        // listing the collection.
        require_db_read(cmd, collection)?;
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

    // -------------------------------------------------- replay program pinning

    /// Persist the exact compiled program a run executed, keyed by its
    /// `code_hash`, so `runtime.replay` can reconstruct it even after the
    /// applet is reinstalled/upgraded (review 031 finding 3 / CR-9). Keyed by
    /// content hash, so re-running the same code is idempotent (same bytes).
    fn store_program(&mut self, installed: &InstalledApplet) -> Result<()> {
        let bytes = serde_json::to_vec(installed).map_err(|e| {
            CoreError::StorageError(format!("program serialize failed: {e}"))
        })?;
        self.store
            .kv_set(META_NS, &program_key(&installed.code_hash), &bytes, "application/json")
    }

    /// Load the program recorded for a given `code_hash`, if one was pinned.
    fn load_program(&self, code_hash: &str) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &program_key(code_hash))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("program deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }

    /// Persist the exact compiled program + manifest a run executed, keyed by the
    /// run's unique `run_id` (review 036 finding 2). Unlike the content-addressed
    /// [`store_program`], this key is unique to the execution, so reinstalling the
    /// same JS under a different manifest (tighter limits / changed caps) cannot
    /// overwrite an older run's pinned context.
    fn store_run_program(&mut self, run_id: &str, installed: &InstalledApplet) -> Result<()> {
        let bytes = serde_json::to_vec(installed).map_err(|e| {
            CoreError::StorageError(format!("run program serialize failed: {e}"))
        })?;
        self.store
            .kv_set(META_NS, &run_program_key(run_id), &bytes, "application/json")
    }

    /// Load the per-run pinned program for `run_id`, if one was recorded (runs
    /// recorded before per-run pinning have none → fall back to the code_hash pin).
    fn load_run_program(&self, run_id: &str) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &run_program_key(run_id))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("run program deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }

    // -------------------------------------------------- per-execution counter

    /// Atomically read-bump-write the persisted workspace run counter, returning
    /// the value assigned to this invocation. Monotone across the workspace's
    /// lifetime (persisted in meta), so each `runtime.run` mints a distinct
    /// `run_id` even for an identical applet+input pair (review 031 finding 2).
    ///
    /// Review 036 finding 3: the read+bump+write run inside ONE SQLite transaction
    /// ([`Store::next_counter`]), so the reservation is atomic. Two `WorkspaceCore`
    /// instances over the same file can no longer reserve the same invocation
    /// number — the second transaction observes the first's committed value — so no
    /// audit record is silently replaced via a `run_id` collision.
    fn next_run_counter(&mut self) -> Result<u64> {
        self.store.next_counter(META_NS, RUN_COUNTER_KEY)
    }
}

/// KV key for an applet's installed record within [`META_NS`].
fn applet_key(applet_id: &str) -> String {
    format!("applet/{applet_id}")
}

/// KV key for a pinned replay program within [`META_NS`], keyed by `code_hash`.
/// Content-addressed, so the same code reinstalled under a new applet version
/// still maps to the one program every run that hashed to it can replay against.
/// Kept as a fallback for runs recorded before per-run pinning (review 036
/// finding 2): it does NOT capture the manifest a specific run used, so a
/// same-code reinstall under a different manifest overwrites it.
fn program_key(code_hash: &str) -> String {
    format!("program/{code_hash}")
}

/// KV key for the PER-RUN pinned replay program within [`META_NS`], keyed by the
/// unique `run_id` (review 036 finding 2). Unique per execution, so no reinstall
/// can overwrite the program + manifest an older run replays against.
fn run_program_key(run_id: &str) -> String {
    format!("program/run/{run_id}")
}

/// Read the optional explicit `(random_seed, time_start)` seam override from a
/// `runtime.run` payload (review 032 finding 1).
///
/// Returns `Ok(None)` when neither field is present (the default
/// `(code_hash, input)`-derived seeds apply). If *either* is present, *both*
/// must be: a half-specified override is a malformed command (a scenario that
/// pins one seam but lets the other drift is not reproducible), rejected with
/// `ValidationError` rather than silently defaulting the missing seam. Each
/// field must be a non-negative integer that fits `u64`.
fn run_seed_override(cmd: &CoreCommand) -> Result<Option<(u64, u64)>> {
    let random_seed = cmd.payload.get("random_seed");
    let time_start = cmd.payload.get("time_start");
    match (random_seed, time_start) {
        (None, None) => Ok(None),
        (Some(r), Some(t)) => Ok(Some((seed_field("random_seed", r)?, seed_field("time_start", t)?))),
        (Some(_), None) | (None, Some(_)) => Err(CoreError::ValidationError(
            "runtime.run seed override must set BOTH `random_seed` and `time_start` or neither"
                .into(),
        )),
    }
}

/// Parse a `u64` seed field from the command payload, rejecting non-integer /
/// out-of-range values with a precise `ValidationError`.
fn seed_field(name: &str, value: &serde_json::Value) -> Result<u64> {
    value.as_u64().ok_or_else(|| {
        CoreError::ValidationError(format!(
            "runtime.run `{name}` must be a non-negative integer that fits u64, got {value}"
        ))
    })
}

/// Derive the deterministic replay seeds `(random_seed, time_start)` from the
/// run's `(code_hash, input)` (review 031 finding 2). The same code + input
/// always produces the same seeds, so re-runs replay byte-identically and the
/// "deterministic across independent runs" property holds; a different input
/// produces different seeds (a genuinely different deterministic run).
///
/// This is a stable, non-cryptographic split of the canonical `code_hash`
/// (which already digests the program) mixed with a digest of the input. It is
/// not security-sensitive — only determinism matters — so a fixed FNV-style
/// fold over the canonical inputs is sufficient and dependency-free.
fn derive_seeds(code_hash: &str, input: &serde_json::Value) -> (u64, u64) {
    // Canonical JSON for the input (serde_json sorts object keys), so equal
    // inputs fold to the same digest regardless of construction order.
    let input_repr = input.to_string();
    let random_seed = fnv1a64(code_hash.as_bytes()) ^ fnv1a64(input_repr.as_bytes());
    // A second, independent fold (salted) for the time seam so the two seeds are
    // not trivially correlated.
    let time_start = fnv1a64(input_repr.as_bytes()).wrapping_mul(0x100000001b3)
        ^ fnv1a64(code_hash.as_bytes());
    (random_seed, time_start)
}

/// A small FNV-1a 64-bit fold. Deterministic and dependency-free; used only to
/// derive replay seeds (not for security or collision resistance).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Mint a unique, inspectable per-execution `run_id` from the run's `code_hash`
/// and the workspace's monotone invocation counter (review 031 finding 2). The
/// counter guarantees uniqueness even when two executions share code+input (and
/// therefore seeds); the short hash prefix keeps the id self-describing.
fn unique_run_id(code_hash: &str, invocation: u64) -> RunId {
    // Strip the `alg:` tag, then take a short prefix of the digest body.
    let digest = code_hash.split_once(':').map(|(_, body)| body).unwrap_or(code_hash);
    let short = &digest[..8.min(digest.len())];
    RunId::new(format!("run_{short}_{invocation:06}"))
}

/// Command-level RBAC gate (prd-merged/01 CR-A3): reject a command whose
/// actor role is not permitted to issue it *before* any handler touches state.
///
/// The role matrix is the "Roles" column of `forge/spec/commands.md` for the
/// M0a command set. An unauthorized actor returns [`CoreError::PermissionDenied`];
/// an unknown command is left for the dispatcher to reject with a
/// `ValidationError` (so capability negotiation, not authorization, owns that
/// message). This is the first of the two CR-A3 layers; the per-`ctx.*`
/// capability/policy gate still runs at host-call time inside the runtime.
fn authorize(cmd: &CoreCommand) -> Result<()> {
    let role = cmd.actor.role;
    // `None` ⇒ no command-level role restriction (the command is reachable by
    // any authenticated actor, or it is an unknown name the dispatcher rejects).
    let allowed: Option<&[Role]> = match cmd.name.as_str() {
        // Owner-only workspace lifecycle (commands.md: workspace.create → Owner).
        "workspace.create" => Some(&[Role::Owner]),
        // Read-level metadata: every member role may open/inspect the workspace.
        "workspace.open" => Some(&[
            Role::Owner,
            Role::Maintainer,
            Role::Editor,
            Role::Viewer,
            Role::Auditor,
        ]),
        // Installing/compiling an applet is a maintainer+ operation (SC-15):
        // Viewer/Auditor/Runner/Editor cannot install.
        "applet.install" => Some(&[Role::Owner, Role::Maintainer]),
        // Triggering execution: the run-capable roles (CR-8). Viewer/Auditor are
        // read-only/oversight and cannot run code.
        "runtime.run" => Some(&[Role::Owner, Role::Maintainer, Role::Editor, Role::Runner]),
        // Replay is an audit/oversight operation (CR-9): Auditor/Maintainer/Owner.
        // A bare Runner can run but not replay (per commands.md).
        "runtime.replay" => Some(&[Role::Owner, Role::Maintainer, Role::Auditor]),
        // Reading the records projection requires a read-capable role (db.read).
        "query.execute" => Some(&[
            Role::Owner,
            Role::Maintainer,
            Role::Editor,
            Role::Viewer,
            Role::Auditor,
        ]),
        _ => None,
    };
    match allowed {
        Some(roles) if !roles.contains(&role) => Err(CoreError::PermissionDenied(format!(
            "actor role {role:?} is not permitted to issue {:?} (see forge/spec/commands.md)",
            cmd.name
        ))),
        _ => Ok(()),
    }
}

/// True iff `role` carries the `db.read` capability at the command level.
///
/// `forge/spec/commands.md` lists the data-read membership roles (the same set
/// that may `workspace.open` / `file.history` / read projections): Owner,
/// Maintainer, Editor, Viewer, Auditor. The execution-only `Runner` and the
/// code-review `Reviewer` are NOT data readers, so they lack `db.read` even
/// though `Runner` may `runtime.run`. This mirrors the manifest `db.read` grant
/// the runtime enforces per `ctx.db.*` call, lifted to the workspace command.
fn role_has_db_read(role: Role) -> bool {
    matches!(
        role,
        Role::Owner | Role::Maintainer | Role::Editor | Role::Viewer | Role::Auditor
    )
}

/// Command-level `db.read` capability gate for `query.execute` (review 036
/// finding 1, `forge/spec/commands.md:21` "Role plus db.read capability"). The
/// role allowlist in [`authorize`] is a separate, necessary layer; this enforces
/// the *capability* before the projection is read, so an actor whose role lacks
/// `db.read` is denied here rather than handed the records.
fn require_db_read(cmd: &CoreCommand, collection: &str) -> Result<()> {
    if role_has_db_read(cmd.actor.role) {
        Ok(())
    } else {
        Err(CoreError::PermissionDenied(format!(
            "actor role {:?} lacks the db.read capability required to query {collection:?} (forge/spec/commands.md: query.execute = Role plus db.read)",
            cmd.actor.role
        )))
    }
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
