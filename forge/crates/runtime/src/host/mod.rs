//! The shared host context: the single mutable hub the `ctx.*` forwarders call.
//!
//! Every `ctx.*` host call funnels through [`HostContext::call`], which is the
//! one place that enforces the full chain for a host effect:
//!   1. policy/capability check (forge-policy [`PolicyEngine`], prd-merged/01
//!      CR-4 call-time checks);
//!   2. the deterministic record/replay recorder (prd-merged/01 CR-8/CR-11);
//!   3. log/storage byte budgets + per-namespace host-call counters, all routed
//!      through the single [`HostBudgets`] source of truth (prd-merged/01 CR-5,
//!      prd-merged/07 SC-2).
//!
//! Keeping this target-independent (no QuickJS) means the policy + record/replay
//! seam is testable and `wasm32`-clean; the engine only marshals JS values to
//! and from `serde_json::Value` and calls in here.

use crate::bridge::HostBridge;
use crate::recorder::RunRecorder;
use forge_domain::{
    ActorContext, FilesGrant, Limits, Manifest, NetGrant, PermissionSnapshot, Result,
};
use forge_policy::PolicyEngine;

// Low-coupling host-call handlers split into focused submodules. Each adds
// `impl HostContext` methods so `HostContext`'s public surface is reachable at
// the same paths regardless of which file the handler body lives in:
//   * `policy`  — the `check_or_record_denial` denial-recording chokepoint;
//   * `time`    — the `ctx.time.now` / `ctx.random.next` deterministic seams;
//   * `log`     — the `ctx.log` sink + its byte/call budgets;
//   * `ui`      — `ctx.ui.render` + the UI event-dispatch envelope;
//   * `storage` — the `ctx.storage.*` key/value effects;
//   * `db`      — the `ctx.db.*` collection effects (incl. query `from` pinning);
//   * `net`     — `ctx.net.fetch` + the SC-5/SC-13 egress projections/redaction;
//   * `files`   — `ctx.files.read`/`write` + the CR-3 confinement/cap helpers.
mod budget;
mod db;
mod files;
mod log;
mod net;
mod policy;
mod storage;
mod time;
mod ui;

use budget::HostBudgets;

/// The hub shared (via interior mutability in the engine) by all `ctx.*`
/// forwarders for the duration of a single run.
pub struct HostContext<'b> {
    policy: PolicyEngine,
    recorder: RunRecorder,
    bridge: &'b mut dyn HostBridge,
    /// The single source of truth for the CR-5 byte budgets and the SC-2
    /// per-namespace host-call counters: every effect that consumes a budget
    /// routes its `saturating_add` + limit comparison through a `check_*` method
    /// here, so the budget arithmetic lives in exactly one place.
    budgets: HostBudgets,
    /// The full network egress allowlist for `ctx.net.fetch` (prd-merged/07
    /// SC-5/SC-8), with **every** SC-5 constraint intact (request + response).
    /// Derived from the policy's permission snapshot at construction so it tracks
    /// the *recorded* grants on replay (review 009 P1 CR-9), not the live manifest.
    /// Empty ⇒ no network (the default for every applet). The **response-leg**
    /// check (`net_fetch` step 5) runs against this full allowlist.
    net_allowlist: NetGrant,
    /// The **request-phase** view of [`net_allowlist`](Self::net_allowlist): the
    /// same rules with their *response* constraints (`max_response_bytes`,
    /// `response_content_types`) cleared. The call gate (`net_fetch` step 2) must
    /// decide *before* a request is sent, when the response is unknown — so it
    /// evaluates only the request-side gates against this view. A rule that
    /// constrains the response would otherwise spuriously deny at the call gate
    /// (the policy denies an unknown response content-type); stripping the
    /// response constraints here defers them, intact, to the response leg where
    /// the real response is in hand. Built once at construction so each fetch is
    /// allocation-free on this path.
    net_allowlist_request_phase: NetGrant,
    /// The full handle-scoped filesystem grant for `ctx.files` (prd-merged/01
    /// CR-3, `forge/spec/files.md`). Like [`net_allowlist`](Self::net_allowlist)
    /// it is derived from the policy's permission **snapshot** at construction, so
    /// on replay it is the *recorded* grant (built via `PolicyEngine::from_snapshot`),
    /// not whatever the live manifest grants now — keeping a files allow/deny
    /// decision deterministic across replay (review 009 P1 CR-9). Empty ⇒ no file
    /// access (the default for every applet).
    files_grant: FilesGrant,
    /// Captured log lines (mirrored into the RunRecord).
    logs: Vec<String>,
}

impl<'b> HostContext<'b> {
    pub fn new(
        manifest: &Manifest,
        actor: &ActorContext,
        recorder: RunRecorder,
        bridge: &'b mut dyn HostBridge,
    ) -> Result<Self> {
        // `PolicyEngine::new` validates the manifest's storage glob grants
        // (forge-policy review 006 P2), so it can now fail closed; propagate that
        // instead of constructing a hub around invalid grants.
        Ok(Self::with_policy(
            PolicyEngine::new(manifest, actor)?,
            manifest.limits.clone(),
            recorder,
            bridge,
        ))
    }

    /// Build a hub around a pre-constructed [`PolicyEngine`]. Replay uses this
    /// with a policy built from the run's recorded [`PermissionSnapshot`]
    /// (review 009 P1 CR-9), so the replay re-derives the *recorded* permission
    /// decision rather than whatever the live manifest grants now.
    pub fn with_policy(
        policy: PolicyEngine,
        limits: Limits,
        recorder: RunRecorder,
        bridge: &'b mut dyn HostBridge,
    ) -> Self {
        // The net allowlist rides on the evaluated permission snapshot's
        // capabilities, so on replay it is the *recorded* grant (built via
        // `PolicyEngine::from_snapshot`), not whatever the live manifest grants
        // now — keeping a net allow/deny decision deterministic across replay
        // exactly like the storage/db scopes (review 009 P1 CR-9).
        let snapshot = policy.snapshot();
        let net_allowlist = snapshot.capabilities.net;
        let net_allowlist_request_phase = net::request_phase_allowlist(&net_allowlist);
        // The files grant likewise rides on the recorded snapshot's capabilities,
        // so a files allow/deny is deterministic across replay (review 009 P1 CR-9).
        let files_grant = snapshot.capabilities.files;
        HostContext {
            policy,
            recorder,
            bridge,
            budgets: HostBudgets::new(limits),
            net_allowlist,
            net_allowlist_request_phase,
            files_grant,
            logs: Vec::new(),
        }
    }

    /// The evaluated permission snapshot for this run (review 009 P1 CR-9), to
    /// be recorded on the [`RunRecord`] so a later replay is governed by the
    /// permissions in effect *now*, not the live manifest then.
    pub fn permission_snapshot(&self) -> PermissionSnapshot {
        self.policy.snapshot()
    }

    /// Consume the context after a run, yielding the recorder (for the trace)
    /// and the captured logs.
    pub fn finish(self) -> (RunRecorder, Vec<String>) {
        (self.recorder, self.logs)
    }

    /// In replay mode, fail the run if not every recorded call was consumed
    /// (review 009 P2). Delegates to the recorder; no-op in record mode.
    pub fn assert_replay_consumed(&self) -> Result<()> {
        self.recorder.assert_fully_consumed()
    }
}
