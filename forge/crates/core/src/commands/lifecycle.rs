//! Applet lifecycle transitions (CR-7 / `forge/spec/applet-lifecycle.md`):
//! `applet.enable` (suspended → enabled), `applet.suspend` (enabled → suspended,
//! idempotent), `applet.upgrade` (atomically install a new version over an active
//! applet), and `applet.uninstall` (remove the active applet with a retention
//! policy). These are the durable-state transitions over the existing
//! [`AppletLifecycle`](super::super::AppletLifecycle) flag + the installed-applet
//! record; `install` (the enabled-v1 creator) and the suspended-dispatch gate live
//! with their commands (`applet.rs`, `runtime_run.rs`, `ui.rs`).
//!
//! Durable lifecycle states (the spec's three):
//!   - `enabled`   — an active applet record exists AND its lifecycle is `Active`.
//!   - `suspended` — an active applet record exists AND its lifecycle is `Suspended`.
//!   - `uninstalled` — NO active applet record (retention policy decided whether
//!     its records survived).
//!
//! Every rejection is a TYPED [`CoreError::ValidationError`] carrying a stable
//! lifecycle-error MARKER (`lifecycle.applet_not_installed` / `lifecycle.invalid_
//! transition` / `lifecycle.upgrade_failed`), mirroring how `ui.dispatch_event`
//! carries `ui.applet_not_dispatchable`. An illegal transition (enable/suspend/
//! uninstall/upgrade an uninstalled applet) is fail-closed: no state changes, no
//! user code runs. An `applet.upgrade` whose staged work fails is fail-closed too:
//! the active version, lifecycle, records, and schema registry remain as they were
//! BEFORE the command (the staged work is applied to a COPY and only committed on
//! success).

use forge_domain::{CoreError, Manifest, Result};

use super::super::persistence::META_NS;
use super::super::signing::verify_install_signature;
use super::super::{AppletLifecycle, InstalledApplet, WorkspaceCore, SCHEMA_REGISTRY_KEY};
use super::{require_applet_id, take_field};

/// Lifecycle error marker (spec "Lifecycle Errors"): no active applet record
/// exists for the `applet_id`. The command requires an installed applet. Embedded
/// as a prefix on a [`CoreError::ValidationError`] so a shell/conformance harness
/// can key off the stable code without parsing the English message — the same
/// marker convention the UI dispatch gate uses for `ui.applet_not_dispatchable`.
pub(in crate::workspace) const LIFECYCLE_NOT_INSTALLED: &str = "lifecycle.applet_not_installed";

/// Lifecycle error marker (spec "Lifecycle Errors"): the command requires an
/// enabled applet but the applet is suspended (e.g. `runtime.run` on a suspended
/// applet).
pub(in crate::workspace) const LIFECYCLE_SUSPENDED: &str = "lifecycle.applet_suspended";

/// Lifecycle error marker (`forge/spec/applet-lifecycle.md` `applet.upgrade`): a
/// staged upgrade FAILED before the active pointer switched, so the whole upgrade
/// rolled back (active version, lifecycle, records, schema registry unchanged). The
/// `upgrade_failure_rolls_back` vector keys off this code. Distinct from
/// `applet_not_installed` (there was an active applet to upgrade) — the upgrade
/// itself could not commit.
pub(in crate::workspace) const LIFECYCLE_UPGRADE_FAILED: &str = "lifecycle.upgrade_failed";

/// A typed rejection for a command issued against an applet with no active record
/// (`uninstalled`): `lifecycle.applet_not_installed`. Fail-closed — the caller
/// returns before touching any state.
pub(in crate::workspace) fn not_installed(applet_id: &str) -> CoreError {
    CoreError::ValidationError(format!(
        "{LIFECYCLE_NOT_INSTALLED}: applet {applet_id} is not installed; install it before this command"
    ))
}

/// A typed rejection for an `applet.upgrade` whose staged work failed before the
/// active pointer switched (`lifecycle.upgrade_failed`). `stage` names the failing
/// stage so a shell/conformance harness can confirm WHERE the rollback occurred
/// (the `upgrade_failure_rolls_back` vector asserts `message_contains` the stage).
/// Fail-closed: the caller has applied every staged change to a COPY, so returning
/// this error leaves the live + persisted workspace exactly as it was.
fn upgrade_failed(applet_id: &str, stage: &str, detail: &str) -> CoreError {
    CoreError::ValidationError(format!(
        "{LIFECYCLE_UPGRADE_FAILED}: applet {applet_id} upgrade failed at stage {stage}: {detail}; the active version was rolled back unchanged"
    ))
}

impl WorkspaceCore {
    /// `applet.upgrade` — install a NEW version over an active applet, ATOMICALLY
    /// (CR-7 / `forge/spec/applet-lifecycle.md` `applet.upgrade`).
    ///
    /// Payload: `{ applet_id, manifest, sources, signature?, schema_additions?,
    /// simulate_failure_stage? }`. `manifest` + `sources` describe the new version
    /// (a different canonical payload than the active one — same-payload reinstall
    /// is the `applet.install` idempotent no-op, not an upgrade). `schema_additions`
    /// is an optional list of additive `schema.apply_change` operations the new
    /// version needs; `simulate_failure_stage` is a TEST-ONLY seam that injects a
    /// staged failure (e.g. `"schema.apply_change"`) so the rollback path is
    /// exercised end-to-end. That field is read ONLY through the `test-hooks`-gated
    /// [`super::test_hooks`] seam, so it is INERT in the release build (review 157):
    /// a production caller cannot inject it to force a rollback.
    ///
    /// Atomicity (spec lines 92-94): every step — manifest validation, signature
    /// verification, source compilation, and schema additions — is STAGED first,
    /// applied only to LOCAL values / a COPY of the schema registry, and the active
    /// applet pointer / persisted registry are switched ONLY after all staged work
    /// succeeds. A failure at ANY stage returns a typed `lifecycle.upgrade_failed`
    /// and leaves the active version, lifecycle state, records, schema registry,
    /// indexes, and replay pins exactly as they were before the command. The prior
    /// version's per-run + content-addressed program pins are NOT touched, so an
    /// already-recorded run still replays against its OWN `code_hash` after the
    /// upgrade (the `recorded_run_replays_old_code_hash_after_upgrade` vector).
    ///
    /// Upgrade does NOT resume a suspended applet (spec line 97): the new active
    /// version inherits the prior lifecycle flag, so a suspended-then-upgraded
    /// applet stays suspended.
    pub(in crate::workspace) fn cmd_applet_upgrade(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;

        // STAGE 0 — preconditions. There must be an ACTIVE applet to upgrade; an
        // uninstalled applet is a typed `applet_not_installed` rejection (spec line
        // 108), not an upgrade-failed. Read the prior version/generation/state so the
        // response + rollback baseline are anchored to the pre-command active applet.
        let active = self
            .load_applet(applet_id.as_str())?
            .ok_or_else(|| not_installed(applet_id.as_str()))?;
        let from_version = active.version;
        let from_code_hash = active.code_hash.clone();
        let install_generation = active.install_generation;
        // The upgrade does NOT change the lifecycle flag, so the new version inherits
        // it (a suspended applet stays suspended after a successful upgrade).
        let lifecycle = self.applet_lifecycle(applet_id.as_str())?;

        // Run the staged pipeline; on the first failure emit the rejection audit and
        // return the typed error with EVERYTHING rolled back. Nothing below the
        // COMMIT block is persisted, so a staged failure leaves the active applet,
        // lifecycle, records, and schema registry exactly as they were.
        match self.stage_upgrade(cmd, &applet_id, &active) {
            Ok(staged) => {
                // COMMIT phase — all staged work succeeded. Switch the active pointer
                // to the new version and persist the evolved registry together, in ONE
                // SQLite transaction (CR-7 commit atomicity, lifecycle review P1): the
                // schema-registry persist + the active-pointer switch + the program pin
                // commit-or-roll-back as a unit, so a crash or write error mid-commit
                // can NEVER leave the workspace with v2's schema committed but v1 still
                // the active pointer (the split spec lines 92-94 forbid). The version
                // bumps within the SAME install generation (spec Identity); the prior
                // version is retained for audit/replay via its existing per-run +
                // content-addressed program pins (never overwritten).
                let to_version = from_version + 1;
                let installed = InstalledApplet {
                    manifest: staged.manifest,
                    js_code: staged.js_code,
                    code_hash: staged.code_hash.clone(),
                    version: to_version,
                    install_generation,
                    trust: staged.trust.clone(),
                };
                // Whether to inject a TEST-ONLY failure BETWEEN the schema-registry
                // persist and the active-pointer switch INSIDE the commit transaction,
                // so the doc'd `simulate_failure_stage == "commit"` rolls the whole
                // commit back (registry persist included), proving the commit is
                // crash-atomic and not merely "all writes happened to succeed".
                let simulate_commit = super::test_hooks::simulate_failure_at(cmd, "commit");
                // Serialize the evolved registry (if the upgrade added fields) OUTSIDE
                // the transaction; the in-memory `self.registry` is swapped only AFTER
                // the transaction commits, so a rollback leaves it untouched too.
                let next_registry = staged.next_registry;
                let registry_bytes = match &next_registry {
                    Some(reg) => Some(serde_json::to_vec(reg).map_err(|e| {
                        CoreError::StorageError(format!("serialize schema registry: {e}"))
                    })?),
                    None => None,
                };
                let applet_id_str = applet_id.as_str();
                let commit = self.store.transact(|tx| {
                    // 1. Persist the evolved schema registry FIRST (if any), mirroring
                    //    `schema.apply_change`, so the durable registry leads the active
                    //    pointer — but now inside the same transaction as the switch.
                    if let Some(bytes) = &registry_bytes {
                        forge_storage::kv_set_tx(
                            tx,
                            META_NS,
                            SCHEMA_REGISTRY_KEY,
                            bytes,
                            "application/json",
                        )?;
                    }
                    // Injected mid-commit failure: AFTER the registry persisted but
                    // BEFORE the active pointer switches. Returning `Err` here rolls the
                    // whole transaction back — including the registry persist above — so
                    // the workspace stays exactly v1 (the P1 the review found: this is
                    // the window a non-transactional commit could leave half-applied).
                    if simulate_commit {
                        return Err(CoreError::StorageError(
                            "simulated mid-commit failure between schema persist and active-pointer switch".into(),
                        ));
                    }
                    // 2. Switch the active pointer (after the schema persist), so the
                    //    version is observable as v2 only once the whole commit lands.
                    Self::store_applet_tx(tx, applet_id_str, &installed)?;
                    // 3. Pin the new version's program (content-addressed, write-once)
                    //    so a run recorded against v2 replays even if v2 is upgraded
                    //    again — in the SAME transaction so it never half-commits.
                    Self::store_program_tx(tx, &installed)?;
                    Ok(())
                });
                if let Err(e) = commit {
                    // A commit failure (real or simulated) rolled the WHOLE transaction
                    // back: the active version, schema registry, records, and pins are
                    // unchanged (the in-memory registry is NOT swapped below — we return
                    // here). Route through the same rejection-audit path a staged
                    // failure uses, naming the `commit` stage, so the rollback is
                    // observable and the typed `lifecycle.upgrade_failed` is surfaced.
                    let error = upgrade_failed(applet_id_str, "commit", &e.to_string());
                    self.events.emit(
                        Some(applet_id.clone()),
                        "applet.upgrade.rejected",
                        serde_json::json!({
                            "applet_id": applet_id,
                            "active_version": from_version,
                            "active_code_hash": from_code_hash,
                            "failed_stage": "commit",
                            "error_code": LIFECYCLE_UPGRADE_FAILED,
                            "message": error.to_string(),
                        }),
                    );
                    return Err(error);
                }
                // The transaction committed: swap the in-memory registry to match the
                // durable one (skipped above on rollback, where we returned early).
                if let Some(reg) = next_registry {
                    self.registry = reg;
                }
                // The lifecycle flag is deliberately left as-is (no implicit resume).

                self.events.emit(
                    Some(applet_id.clone()),
                    "applet.upgraded",
                    serde_json::json!({
                        "applet_id": applet_id,
                        "install_generation": install_generation,
                        "from_version": from_version,
                        "to_version": to_version,
                        "from_code_hash": from_code_hash,
                        "to_code_hash": staged.code_hash,
                        "state_after": lifecycle_str(lifecycle),
                    }),
                );

                Ok(serde_json::json!({
                    "applet_id": applet_id,
                    "install_generation": install_generation,
                    "previous_version": from_version,
                    "version": to_version,
                    "code_hash": staged.code_hash,
                    "state": lifecycle_str(lifecycle),
                    "warnings": staged.warnings,
                    "trust": staged.trust.to_json(),
                }))
            }
            Err((stage, error)) => {
                // Fail-closed rollback: NOTHING was committed (every staged change
                // landed only on locals / a registry COPY). Emit the rejection audit
                // naming the active (unchanged) version + the failing stage so the
                // rollback is observable, then surface the typed error.
                self.events.emit(
                    Some(applet_id.clone()),
                    "applet.upgrade.rejected",
                    serde_json::json!({
                        "applet_id": applet_id,
                        "active_version": from_version,
                        "active_code_hash": from_code_hash,
                        "failed_stage": stage,
                        "error_code": LIFECYCLE_UPGRADE_FAILED,
                        "message": error.to_string(),
                    }),
                );
                Err(error)
            }
        }
    }

    /// Stage every part of an `applet.upgrade` WITHOUT committing: validate the new
    /// manifest, verify the signature (bound to the new sources), compile the new
    /// entrypoint to a fresh `code_hash`, and apply any `schema_additions` to a COPY
    /// of the registry. Returns the staged artifacts on success, or `(stage, error)`
    /// on the first failure so the caller can roll back with everything untouched.
    ///
    /// The optional `simulate_failure_stage` payload injects a failure at a named
    /// stage (`"manifest.validate"`, `"compile"`, `"schema.apply_change"`) so the
    /// rollback path is exercised end-to-end by the conformance vectors. It is
    /// checked at the corresponding stage boundary so the failure is observed AFTER
    /// the prior stages ran (proving they too rolled back). The `"commit"` stage is
    /// distinct: it injects a failure INSIDE the commit transaction (between the
    /// schema-registry persist and the active-pointer switch), exercised by
    /// `cmd_applet_upgrade`'s commit block — NOT here, since by then staging is
    /// done and the failure must roll back a real SQLite transaction.
    fn stage_upgrade(
        &self,
        cmd: &forge_domain::CoreCommand,
        applet_id: &forge_domain::AppletId,
        active: &InstalledApplet,
    ) -> std::result::Result<StagedUpgrade, (String, CoreError)> {
        let fail = |stage: &str, detail: &str| {
            (stage.to_string(), upgrade_failed(applet_id.as_str(), stage, detail))
        };
        let simulate = super::test_hooks::simulate_failure_stage(cmd);

        // STAGE 1 — manifest validation.
        let manifest: Manifest =
            take_field(cmd, "manifest").map_err(|e| fail("manifest.validate", &e.to_string()))?;
        manifest
            .validate()
            .map_err(|e| fail("manifest.validate", &e.to_string()))?;
        if simulate == Some("manifest.validate") {
            return Err(fail("manifest.validate", "simulated failure"));
        }

        // STAGE 2 — sources present + non-empty.
        let sources = cmd
            .payload
            .get("sources")
            .and_then(|v| v.as_object())
            .ok_or_else(|| fail("compile", "applet.upgrade requires a `sources` object"))?;
        if sources.is_empty() {
            return Err(fail("compile", "applet.upgrade `sources` must not be empty"));
        }

        // STAGE 3 — signature verification (bound to the new sources, SC-15), so a
        // signed upgrade can only bless the exact code being compiled/stored. An
        // unsigned upgrade proceeds `Unsigned`.
        let trust = verify_install_signature(cmd, applet_id, &manifest, sources)
            .map_err(|e| fail("signature.verify", &e.to_string()))?;

        // STAGE 4 — compile every source (static policy scan + transpile); the
        // entrypoint's program is the runnable one and yields the new `code_hash`.
        let mut warnings = Vec::new();
        let mut entry_program: Option<forge_pipeline::Program> = None;
        for (path, src) in sources {
            let ts = src
                .as_str()
                .ok_or_else(|| fail("compile", &format!("source {path:?} must be a string")))?;
            let program =
                forge_pipeline::compile(ts).map_err(|e| fail("compile", &e.to_string()))?;
            if path == &manifest.entrypoint {
                entry_program = Some(program);
            }
        }
        let entry_program = entry_program.ok_or_else(|| {
            fail(
                "compile",
                &format!(
                    "manifest.entrypoint {:?} is not among the provided sources",
                    manifest.entrypoint
                ),
            )
        })?;
        if sources.len() > 1 {
            warnings.push(format!(
                "{} non-entrypoint source(s) compiled but only the entrypoint is runnable in M0a",
                sources.len() - 1
            ));
        }
        if simulate == Some("compile") {
            return Err(fail("compile", "simulated failure"));
        }

        // An upgrade must carry a DIFFERENT canonical payload than the active version
        // (spec line 40: same payload is the `applet.install` idempotent no-op, not an
        // upgrade). Reject a no-op upgrade so a caller does not mint a spurious v2 with
        // identical code + manifest.
        if entry_program.code_hash == active.code_hash && manifest == active.manifest {
            return Err(fail(
                "compile",
                "upgrade payload is identical to the active version (same code_hash and manifest); reinstall is an applet.install no-op, not an upgrade",
            ));
        }

        // STAGE 5 — schema additions, applied to a COPY of the registry so a rejected
        // change leaves the live + persisted registry untouched. The schema crate is
        // the authority (additive-only; rejects a destructive/incompatible change).
        let next_registry = self.stage_schema_additions(cmd, simulate)?;

        Ok(StagedUpgrade {
            manifest,
            js_code: entry_program.js_code,
            code_hash: entry_program.code_hash,
            trust,
            warnings,
            next_registry,
        })
    }

    /// Apply an upgrade's optional `schema_additions` to a COPY of the registry,
    /// returning `Some(next)` when at least one field was added (so the caller
    /// persists it on commit) or `None` when the upgrade carried no schema work.
    /// The candidate registry is never persisted here — atomicity lives in the
    /// caller's commit block. `simulate == "schema.apply_change"` injects a staged
    /// failure AFTER the additions are applied to the copy, proving the copy (and
    /// therefore the live registry) is discarded on rollback.
    fn stage_schema_additions(
        &self,
        cmd: &forge_domain::CoreCommand,
        simulate: Option<&str>,
    ) -> std::result::Result<Option<forge_schema::SchemaRegistry>, (String, CoreError)> {
        let stage = "schema.apply_change";
        let fail = |detail: &str| {
            (
                stage.to_string(),
                CoreError::ValidationError(format!(
                    "{LIFECYCLE_UPGRADE_FAILED}: upgrade failed at stage {stage}: {detail}; the active version was rolled back unchanged"
                )),
            )
        };

        let additions = match cmd.payload.get("schema_additions") {
            None | Some(serde_json::Value::Null) => Vec::new(),
            Some(serde_json::Value::Array(arr)) => arr.clone(),
            Some(_) => {
                return Err(fail("`schema_additions` must be an array"));
            }
        };

        // A simulated failure at the schema stage is observed even when there are no
        // real additions, so the vector can exercise the rollback without a concrete
        // schema change. It fails AFTER staging (below) when there ARE additions.
        if additions.is_empty() {
            if simulate == Some(stage) {
                return Err(fail("simulated failure"));
            }
            return Ok(None);
        }

        // Apply each addition to a COPY (mirrors `cmd_schema_apply_change`): the
        // schema crate mints stable field ids + enforces additive-only evolution and
        // rejects a destructive/incompatible change, leaving the live registry intact.
        let actor = cmd.actor.actor.clone();
        let mut next = self.registry.clone();
        for addition in &additions {
            // An `add_field` targets an EXISTING collection (DL-7). If the upgrade's
            // collection is not yet registered (the M0a spine defines schema lazily on
            // first use), define it first with an additive `add_collection` so the
            // field addition lands — both steps are additive, so the whole staged
            // change is still additive-only and rolls back together on any failure.
            let collection = addition
                .get("collection")
                .and_then(|v| v.as_str())
                .ok_or_else(|| fail("schema addition requires a `collection` string"))?;
            if next.collection(collection).is_none() {
                next.apply_change(forge_schema::SchemaChange::AddCollection {
                    name: collection.to_string(),
                })
                .map_err(|e| fail(&e.to_string()))?;
            }
            let change = schema_addition_to_change(addition, &actor).map_err(|e| fail(&e))?;
            next.apply_change(change).map_err(|e| fail(&e.to_string()))?;
        }
        if simulate == Some(stage) {
            return Err(fail("simulated failure"));
        }
        Ok(Some(next))
    }

    /// `applet.enable` — make a suspended applet dispatchable again (CR-7:
    /// `suspended -> enabled`). Rules (`forge/spec/applet-lifecycle.md`):
    ///   - `suspended -> enabled` succeeds and flips the trusted lifecycle to
    ///     `Active`, so the next `runtime.run` / `ui.dispatch_event` is accepted;
    ///   - `enabled -> enabled` is an idempotent no-op (`changed: false`);
    ///   - `uninstalled -> enabled` is a typed `lifecycle.applet_not_installed`
    ///     rejection (no active record to enable).
    ///
    /// Payload: `{ applet_id }`.
    pub(in crate::workspace) fn cmd_applet_enable(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        // Fail-closed: an uninstalled applet cannot be enabled (illegal transition).
        if self.load_applet(applet_id.as_str())?.is_none() {
            return Err(not_installed(applet_id.as_str()));
        }

        let before = self.applet_lifecycle(applet_id.as_str())?;
        let changed = before == AppletLifecycle::Suspended;
        if changed {
            self.set_applet_lifecycle(applet_id.as_str(), AppletLifecycle::Active)?;
        }

        // `enabled` either way (the post-state); `changed` distinguishes a real
        // resume from the idempotent re-enable of an already-enabled applet.
        let kind = if changed { "applet.enabled" } else { "applet.enable.noop" };
        self.events.emit(
            Some(applet_id.clone()),
            kind,
            serde_json::json!({
                "applet_id": applet_id,
                "state_before": lifecycle_str(before),
                "state_after": "enabled",
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "state": "enabled",
            "changed": changed,
            "idempotent": !changed,
        }))
    }

    /// `applet.suspend` — prevent new runs/event dispatch while retaining the
    /// active version + data (CR-7: `enabled -> suspended`). Rules:
    ///   - `enabled -> suspended` succeeds and flips the trusted lifecycle to
    ///     `Suspended`, so the next `runtime.run` / `ui.dispatch_event` is rejected
    ///     BEFORE any handler runs;
    ///   - `suspended -> suspended` is idempotent (`changed: false`); in-flight runs
    ///     keep their recorded version and may finish (they are already past this
    ///     gate);
    ///   - `uninstalled -> suspended` is a typed `lifecycle.applet_not_installed`
    ///     rejection.
    ///
    /// Payload: `{ applet_id, reason? }` — `reason` is an optional audit note.
    pub(in crate::workspace) fn cmd_applet_suspend(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        if self.load_applet(applet_id.as_str())?.is_none() {
            return Err(not_installed(applet_id.as_str()));
        }
        let reason = cmd.payload.get("reason").and_then(|v| v.as_str());

        let before = self.applet_lifecycle(applet_id.as_str())?;
        let changed = before == AppletLifecycle::Active;
        if changed {
            self.set_applet_lifecycle(applet_id.as_str(), AppletLifecycle::Suspended)?;
        }

        // A real suspend emits `applet.suspended`; an idempotent re-suspend of an
        // already-suspended applet emits `applet.suspend.noop` (the `suspend_already_
        // suspended_idempotent` vector) — no user code ran either way.
        let kind = if changed { "applet.suspended" } else { "applet.suspend.noop" };
        self.events.emit(
            Some(applet_id.clone()),
            kind,
            serde_json::json!({
                "applet_id": applet_id,
                "state_before": lifecycle_str(before),
                "state_after": "suspended",
                "reason": reason,
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "state": "suspended",
            "changed": changed,
            "idempotent": !changed,
        }))
    }

    /// `applet.uninstall` — remove the active applet, requiring a retention policy
    /// (CR-7). `payload.retention_policy` is `"keep_data"` or `"purge_data"`:
    ///
    ///   - `keep_data`: remove the active applet record, RETAIN applet-owned
    ///     records (and run records + replay artifacts for audit/replay);
    ///   - `purge_data`: remove the active applet record AND tombstone applet-owned
    ///     records with the `applet.uninstall:purge_data` reason. Run records and
    ///     replay artifacts remain as audit evidence.
    ///
    /// "Applet-owned records" are the records in the collections the uninstalled
    /// applet's manifest grants WRITE access to (`capabilities.db.write`) — the
    /// collections the applet could have written. The generation counter and pinned
    /// replay programs are NOT removed, so a recorded run still replays against its
    /// own `code_hash` and a reinstall mints a fresh generation.
    ///
    /// Rules:
    ///   - `uninstalled -> uninstall` (no active record) is a typed
    ///     `lifecycle.applet_not_installed` rejection;
    ///   - a missing/invalid `retention_policy` is a `ValidationError` (the policy is
    ///     mandatory, so retention is never implicit);
    ///   - the active-record removal + any tombstone work are reported together.
    ///
    /// Payload: `{ applet_id, retention_policy }`.
    pub(in crate::workspace) fn cmd_applet_uninstall(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let installed = self
            .load_applet(applet_id.as_str())?
            .ok_or_else(|| not_installed(applet_id.as_str()))?;

        // The retention policy is MANDATORY (CR-7): retention is never implicit.
        let policy = match cmd.payload.get("retention_policy").and_then(|v| v.as_str()) {
            Some("keep_data") => RetentionPolicy::KeepData,
            Some("purge_data") => RetentionPolicy::PurgeData,
            Some(other) => {
                return Err(CoreError::ValidationError(format!(
                    "applet.uninstall `retention_policy` must be \"keep_data\" or \"purge_data\", got {other:?}"
                )))
            }
            None => {
                return Err(CoreError::ValidationError(
                    "applet.uninstall requires a `retention_policy` of \"keep_data\" or \"purge_data\""
                        .into(),
                ))
            }
        };

        let install_generation = installed.install_generation;

        // The collections this applet owned = the collections its manifest granted
        // WRITE access to. `purge_data` tombstones the live records in those
        // collections; `keep_data` leaves them. Either way, removing the active
        // record + (for purge) the tombstones is the atomic unit of the uninstall.
        let owned_collections: Vec<String> = installed.manifest.capabilities.db.write.clone();
        let mut records_retained = 0usize;
        let mut records_tombstoned = 0usize;

        // SC-12 live wiring (`forge/spec/audit-log.md` §2): an uninstall is a
        // security-relevant lifecycle decision — for `purge_data` it hard-tombstones
        // applet-owned records — so it lands a durable, queryable `applet.uninstalled`
        // audit row through this real command path. The row MUST commit in the SAME
        // `Store::transact` as the durable mutation it records (the tombstone writes +
        // active-pointer removal for purge; the active-pointer removal for keep), so a
        // crash (or a transient SQLite error on a separate append) can NEVER leave the
        // applet's records hard-tombstoned / the applet uninstalled with NO audit row
        // of the purge — they land or roll back as one unit, exactly like the sync-RBAC
        // path (`crdt_write/remote.rs`).
        //
        // The uninstall transaction can roll back as a NORMAL outcome (a failed
        // tombstone write), so we use the DEFERRED-EMIT seam: PEEK the next
        // `logical_time`, build the row at it WITHOUT emitting, append it inside the
        // mutation's transaction, and emit the transient `applet.uninstalled` event
        // ONLY after that transaction commits. A rolled-back uninstall therefore
        // persists no row AND emits no spurious event (a shell must not observe an
        // uninstall that didn't happen); a committed one keeps the event and the row
        // under one clock. The metadata mirrors the `audit-log-e2e`
        // `uninstall_purge_data_audit_row` shape (retention policy, tombstone count,
        // tombstone reason, run records + replay artifacts retained); no secret value /
        // body is present, so redaction is a no-op.
        let uninstall_logical_time = self.events.peek_next_logical_time().0;
        let uninstall_event_payload = serde_json::json!({
            "applet_id": applet_id,
            "install_generation": install_generation,
            "retention_policy": policy.as_str(),
            "state_after": "uninstalled",
        });
        let build_uninstall_audit = |this: &Self, records_tombstoned: usize| {
            let mut audit_metadata = serde_json::Map::new();
            audit_metadata
                .insert("retention_policy".into(), serde_json::json!(policy.as_str()));
            audit_metadata
                .insert("records_tombstoned".into(), serde_json::json!(records_tombstoned));
            // The tombstone reason is meaningful only for a purge (keep_data tombstones
            // nothing), so it is recorded only on the purge path — the same string the
            // staged tombstones carry in their `extensions`.
            if matches!(policy, RetentionPolicy::PurgeData) {
                audit_metadata.insert(
                    "tombstone_reason".into(),
                    serde_json::json!("applet.uninstall:purge_data"),
                );
            }
            audit_metadata.insert("run_records_retained".into(), serde_json::json!(true));
            audit_metadata.insert("replay_artifacts_retained".into(), serde_json::json!(true));
            this.build_producer_audit_record_at(
                uninstall_logical_time,
                "lifecycle",
                "applet.uninstalled",
                "allow",
                cmd.actor.actor.as_str(),
                "applet",
                Some(applet_id.as_str().to_string()),
                None,
                match policy {
                    RetentionPolicy::PurgeData => {
                        "uninstall purge_data tombstoned applet-owned records"
                    }
                    RetentionPolicy::KeepData => {
                        "uninstall keep_data retained applet-owned records"
                    }
                },
                serde_json::Value::Object(audit_metadata),
            )
        };

        match policy {
            RetentionPolicy::KeepData => {
                for collection in &owned_collections {
                    records_retained += self
                        .store
                        .list_records(collection)?
                        .iter()
                        .filter(|r| !r.deleted)
                        .count();
                }
                // No record data is purged, but the active-record removal IS the
                // durable uninstall decision, so its `applet.uninstalled` audit row
                // commits in the SAME transaction as the removal (spec §2).
                let audit = build_uninstall_audit(self, 0);
                let applet_id_str = applet_id.as_str();
                self.store.transact(|tx| {
                    Self::delete_applet_tx(tx, applet_id_str)?;
                    forge_storage::Store::append_audit_tx(tx, &audit)?;
                    Ok(())
                })?;
            }
            RetentionPolicy::PurgeData => {
                // Stage the tombstones (read live records OUTSIDE the transaction),
                // then commit the tombstone writes, the active-pointer removal, AND the
                // `applet.uninstalled` audit row in ONE `Store::transact` (CR-7,
                // lifecycle review P2 + SC-12 §2): a crash mid-uninstall can NEVER leave
                // some records purged, others live, the applet still installed, OR the
                // purge committed without its audit row — they land or roll back as a
                // unit.
                let tombstones = self.stage_owned_tombstones(&owned_collections)?;
                records_tombstoned = tombstones.len();
                let audit = build_uninstall_audit(self, records_tombstoned);
                // TEST-ONLY hook: inject a failure BETWEEN the tombstone writes and the
                // active-pointer removal so the whole purge-uninstall rolls back
                // (records stay live, applet stays installed, NO audit row lands),
                // proving the atomicity.
                let simulate_uninstall =
                    super::test_hooks::simulate_failure_at(cmd, "uninstall.tombstone");
                let applet_id_str = applet_id.as_str();
                self.store.transact(|tx| {
                    for env in &tombstones {
                        forge_storage::put_record_tx(tx, env)?;
                    }
                    if simulate_uninstall {
                        return Err(CoreError::StorageError(
                            "simulated mid-uninstall failure between tombstone writes and active-pointer removal".into(),
                        ));
                    }
                    // Remove the active applet record, in the SAME transaction as the
                    // tombstones. The applet is now durably `uninstalled` — the ABSENCE
                    // of an active record — so every lifecycle/dispatch gate
                    // (`runtime.run`, `ui.dispatch_event`, `applet.enable`,
                    // `applet.suspend`) rejects with `lifecycle.applet_not_installed`
                    // BEFORE it ever consults the leftover lifecycle flag. We leave that
                    // dormant flag untouched: a reinstall under the same id explicitly
                    // sets `enabled` (`applet.install`), so it never inherits a stale
                    // `suspended` state. The generation counter is likewise retained (a
                    // reinstall mints a fresh generation past it).
                    Self::delete_applet_tx(tx, applet_id_str)?;
                    // The audit row lands LAST in the same txn: append-only, redacted at
                    // `append_audit_tx`, with the peeked deterministic `logical_time`.
                    forge_storage::Store::append_audit_tx(tx, &audit)?;
                    Ok(())
                })?;
            }
        }

        // The uninstall transaction COMMITTED — only now emit the transient
        // `applet.uninstalled` observability event, stamped with the SAME
        // `logical_time` the durable row carries (the peek above matches this emit
        // because nothing emitted in between). A rolled-back uninstall returned `?`
        // above, so this line is never reached — no spurious event for a non-uninstall.
        let emitted = self.events.emit(
            Some(applet_id.clone()),
            "applet.uninstalled",
            uninstall_event_payload,
        );
        debug_assert_eq!(
            self.events
                .events()
                .iter()
                .rev()
                .find(|e| e.event_id == emitted)
                .map(|e| e.created_at_logical.0),
            Some(uninstall_logical_time),
            "the peeked logical_time must match the committed event's stamp"
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "install_generation": install_generation,
            "state": "uninstalled",
            "retention": {
                "policy": policy.as_str(),
                "records_retained": records_retained,
                "records_tombstoned": records_tombstoned,
                // Run records + replay artifacts are never removed by uninstall, so
                // recorded runs stay auditable + replayable (CR-9).
                "run_records_retained": true,
                "replay_artifacts_retained": true,
            },
        }))
    }

    /// STAGE the tombstones for every live (non-deleted) record in `collections`,
    /// returning the modified envelopes WITHOUT writing them. The caller commits
    /// them inside one `Store::transact` together with the active-pointer removal
    /// (CR-7 purge-uninstall atomicity, lifecycle review P2), so a crash
    /// mid-uninstall rolls the whole purge back. A tombstone is a soft delete
    /// (DL-21): the row is retained with `deleted = true` (so the delete syncs) and
    /// carries the purge reason in its `extensions`, so an auditor / a peer can see
    /// WHY the record was tombstoned. Already-deleted records are skipped
    /// (idempotent), so the staged set holds exactly the records this purge flips.
    fn stage_owned_tombstones(
        &self,
        collections: &[String],
    ) -> Result<Vec<forge_domain::RecordEnvelope>> {
        let mut staged = Vec::new();
        for collection in collections {
            let live: Vec<String> = self
                .store
                .list_records(collection)?
                .into_iter()
                .filter(|r| !r.deleted)
                .map(|r| r.entity_id.as_str().to_string())
                .collect();
            for id in live {
                if let Some(mut env) = self.store.get_record(collection, &id)? {
                    env.deleted = true;
                    env.extensions.insert(
                        "tombstone_reason".into(),
                        serde_json::json!("applet.uninstall:purge_data"),
                    );
                    staged.push(env);
                }
            }
        }
        Ok(staged)
    }
}

/// The mandatory uninstall retention policy (CR-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetentionPolicy {
    /// Remove the active applet; RETAIN applet-owned records + storage.
    KeepData,
    /// Remove the active applet; TOMBSTONE applet-owned records with a purge reason.
    PurgeData,
}

impl RetentionPolicy {
    /// The wire/audit token for this policy.
    fn as_str(self) -> &'static str {
        match self {
            RetentionPolicy::KeepData => "keep_data",
            RetentionPolicy::PurgeData => "purge_data",
        }
    }
}

/// The durable lifecycle token for the active-record's [`AppletLifecycle`] flag —
/// the spec's `enabled` / `suspended` strings (an applet with no active record is
/// `uninstalled`, which is the absence of a record, not this flag).
fn lifecycle_str(lifecycle: AppletLifecycle) -> &'static str {
    match lifecycle {
        AppletLifecycle::Active => "enabled",
        AppletLifecycle::Suspended => "suspended",
    }
}

/// The fully-staged (but NOT yet committed) artifacts of an `applet.upgrade`: the
/// validated new manifest, the compiled new program + `code_hash`, the recorded
/// signing trust, install warnings, and the evolved schema registry (when the
/// upgrade added fields). The caller commits these together — switching the active
/// pointer and persisting the registry — only after EVERY stage succeeded, so a
/// staged failure discards this whole bundle and the workspace is untouched.
struct StagedUpgrade {
    manifest: Manifest,
    js_code: String,
    code_hash: String,
    trust: super::super::InstallTrust,
    warnings: Vec<String>,
    /// `Some` when the upgrade added schema fields (the evolved registry COPY to
    /// persist on commit); `None` when it carried no schema work.
    next_registry: Option<forge_schema::SchemaRegistry>,
}

/// Translate one upgrade `schema_additions` entry into the additive
/// [`SchemaChange`](forge_schema::SchemaChange) the registry applies. The vector
/// shape is `{ collection, field, type, default? }` (a new field on an existing
/// collection); the minting `actor` is the command's actor so the field gets a
/// stable actor-scoped id (DL-7). The `default` is advisory metadata in M0a (the
/// additive field is nullable until an `enforce_required` step), so it is accepted
/// but not stored as a constraint. An unknown `type` is a typed error so a malformed
/// addition fails the upgrade at the schema stage (rolled back) rather than silently.
fn schema_addition_to_change(
    addition: &serde_json::Value,
    actor: &forge_domain::ActorId,
) -> std::result::Result<forge_schema::SchemaChange, String> {
    use forge_schema::{FieldType, SchemaChange};

    let collection = addition
        .get("collection")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "schema addition requires a `collection` string".to_string())?;
    let name = addition
        .get("field")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "schema addition requires a `field` string".to_string())?;
    let ty_str = addition
        .get("type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "schema addition requires a `type` string".to_string())?;
    let ty = match ty_str {
        "text" | "string" => FieldType::Text,
        "integer" | "int" => FieldType::IntNum,
        "float" | "number" => FieldType::FloatNum,
        "boolean" | "bool" => FieldType::Bool,
        "scalar" | "json" => FieldType::Scalar,
        other => return Err(format!("schema addition has an unknown field type {other:?}")),
    };
    Ok(SchemaChange::AddField {
        collection: collection.to_string(),
        actor: actor.clone(),
        name: name.to_string(),
        ty,
        indexed: false,
        required: false,
    })
}
