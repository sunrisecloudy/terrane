//! `runtime.replay` — deterministic, byte-identical re-execution of a recorded run
//! (CR-A2, CR-9). Moved verbatim from `workspace.rs` (/simplify #11a): the handler,
//! the shared [`replay_run_by_id`](WorkspaceCore::replay_run_by_id) machinery (used
//! by the session-replay path too), and the version-pinned replay program store
//! (`program/<code_hash>` + `program/run/<run_id>`) live together.

use forge_domain::{CoreError, Result, RunRecord};
use forge_runtime::{replay, replay_dispatch, NullBridge, Program as RuntimeProgram};

use super::super::persistence::META_NS;
use super::super::{InstalledApplet, WorkspaceCore};

/// KV key for a pinned replay program within [`META_NS`], keyed by `code_hash`.
/// Content-addressed, so the same code reinstalled under a new applet version
/// still maps to the one program every run that hashed to it can replay against.
/// Kept as a fallback for runs recorded before per-run pinning (review 036
/// finding 2). It does NOT capture the manifest a specific run used, so the
/// write is **write-once** ([`store_program`](WorkspaceCore::store_program)):
/// the first run to hash to it pins the fallback and a later same-code reinstall
/// under a different manifest can no longer overwrite it (review 038 finding 3).
fn program_key(code_hash: &str) -> String {
    format!("program/{code_hash}")
}

/// KV key for the PER-RUN pinned replay program within [`META_NS`], keyed by the
/// unique `run_id` (review 036 finding 2). Unique per execution, so no reinstall
/// can overwrite the program + manifest an older run replays against.
fn run_program_key(run_id: &str) -> String {
    format!("program/run/{run_id}")
}

impl WorkspaceCore {
    /// `runtime.replay` — load the stored [`RunRecord`], replay it deterministically
    /// (the recorder serves recorded responses; the live bridge is a
    /// [`NullBridge`] that must never be consulted), and assert the replay is
    /// byte-identical to the original (CR-A2, CR-9). Divergence → `RuntimeError`.
    ///
    /// Payload: `{ run_id }`.
    pub(in crate::workspace) fn cmd_runtime_replay(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let run_id = cmd
            .payload
            .get("run_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::ValidationError("runtime.replay requires `run_id`".into()))?
            .to_string();

        let (original, replayed) = self.replay_run_by_id(&run_id, &cmd.actor)?;

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

    /// Load the stored [`RunRecord`] for `run_id`, reconstruct the version-pinned
    /// program + manifest this execution used, replay it deterministically over a
    /// [`NullBridge`], and assert the replay is byte-identical to the original
    /// (CR-9). Returns `(original, replayed)` so a caller can fingerprint either.
    /// Shared by [`cmd_runtime_replay`](Self::cmd_runtime_replay) and the
    /// session-replay path so the two never drift in how a single run is replayed.
    ///
    /// Version-pinned replay (review 031 finding 3, review 036 finding 2):
    /// reconstruct the program + manifest from the artifact recorded for THIS
    /// execution, not the currently installed applet. Resolution order:
    ///   1. the PER-RUN pin (`program/run/<run_id>`) — unique to this run, so a
    ///      reinstall under a different manifest cannot overwrite or alter it
    ///      (the review 036 finding 2 case);
    ///   2. the content-addressed `program/<code_hash>` pin — covers runs
    ///      recorded before per-run pinning existed;
    ///   3. the currently installed applet — last-resort legacy fallback, and
    ///      only if its code_hash still matches the recorded one.
    ///
    /// A run recorded by `ui.dispatch_event` carries a `ui.dispatch_event` envelope
    /// and was driven by re-entering a named handler (UI-4/CR-6), not `main` — so it
    /// is replayed via the dispatch path, which recovers the recorded `(action_ref,
    /// payload)` and re-runs that handler. A normal `runtime.run` record has no such
    /// envelope and replays via `main`.
    pub(in crate::workspace) fn replay_run_by_id(
        &self,
        run_id: &str,
        actor: &forge_domain::ActorContext,
    ) -> Result<(RunRecord, RunRecord)> {
        let original = self
            .store
            .load_run(run_id)?
            .ok_or_else(|| CoreError::ValidationError(format!("run {run_id} not found")))?;

        let installed = match self.load_run_program(run_id)? {
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

        let program = RuntimeProgram::new(original.applet_id.clone(), installed.js_code.clone());
        let mut null = NullBridge::new();
        let is_dispatch = original.calls.iter().any(|c| c.method == "ui.dispatch_event");
        let replayed = if is_dispatch {
            replay_dispatch(&original, &program, &installed.manifest, actor, &mut null)?
        } else {
            replay(&original, &program, &installed.manifest, actor, &mut null)?
        };

        // The strict replay check: canonical provenance on both records AND
        // byte-identical traces, surfaced as a RuntimeError on divergence.
        original.assert_replay_of(&replayed)?;
        Ok((original, replayed))
    }

    /// Persist the content-addressed replay fallback (`program/<code_hash>`),
    /// **write-once** (review 038 finding 3 / 036 finding 2).
    ///
    /// This artifact is the legacy fallback for runs recorded *before* per-run
    /// pinning (every modern run also gets a per-run pin via
    /// [`store_run_program`](Self::store_run_program), which is never overwritten).
    /// Because it is keyed by `code_hash` alone it does NOT capture the manifest a
    /// particular run used, so blindly overwriting it on every run let a later
    /// same-JS reinstall under a *different* manifest (e.g. tighter `limits`)
    /// replace the artifact a pre-per-run-pin run depends on — stranding that run,
    /// which would then replay under the wrong engine limits.
    ///
    /// Write-once fixes that: the first run to hash to a given `code_hash` pins the
    /// fallback (manifest + JS) and a later run with the **same** code_hash never
    /// overwrites it with a *different* manifest. Re-pinning identical content is an
    /// idempotent no-op (so a same-code, same-manifest re-run is unaffected); an
    /// identical-manifest re-pin is also a no-op. A legacy run keyed to this hash
    /// therefore always replays against the manifest first recorded for it.
    pub(in crate::workspace) fn store_program(&mut self, installed: &InstalledApplet) -> Result<()> {
        // Write-once: if a fallback already exists for this code_hash, keep it.
        // A different manifest must not clobber the original (the stranding bug);
        // an identical one is a no-op either way.
        if self.load_program(&installed.code_hash)?.is_some() {
            return Ok(());
        }
        let bytes = serde_json::to_vec(installed)
            .map_err(|e| CoreError::StorageError(format!("program serialize failed: {e}")))?;
        self.store
            .kv_set(META_NS, &program_key(&installed.code_hash), &bytes, "application/json")
    }

    /// Load the program recorded for a given `code_hash`, if one was pinned.
    pub(in crate::workspace) fn load_program(
        &self,
        code_hash: &str,
    ) -> Result<Option<InstalledApplet>> {
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
    pub(in crate::workspace) fn store_run_program(
        &mut self,
        run_id: &str,
        installed: &InstalledApplet,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(installed)
            .map_err(|e| CoreError::StorageError(format!("run program serialize failed: {e}")))?;
        self.store
            .kv_set(META_NS, &run_program_key(run_id), &bytes, "application/json")
    }

    /// Load the per-run pinned program for `run_id`, if one was recorded (runs
    /// recorded before per-run pinning have none → fall back to the code_hash pin).
    pub(in crate::workspace) fn load_run_program(
        &self,
        run_id: &str,
    ) -> Result<Option<InstalledApplet>> {
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
}
