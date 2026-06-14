//! `quota.status` + `quota.set` — the DL-22 user-facing quota commands
//! (prd-merged/02 DL-22, `forge/spec/quotas.md`).
//!
//! These are the COMMAND boundary over the `forge-storage` quota substrate
//! (`Store::quota_usage` / `quota_policy` / `set_quota_policy`):
//!
//!   - [`cmd_quota_status`](super::super::WorkspaceCore::cmd_quota_status) REPORTS the
//!     deterministic usage vs. the trusted limits, plus the non-blocking
//!     APPROACHING-LIMIT warnings (a budget at/above its threshold) carrying the DL-22
//!     remedy suggestion (compaction / cleanup / export — never deletion). It is a
//!     pure read of persisted state, so it replays byte-identically.
//!   - [`cmd_quota_set`](super::super::WorkspaceCore::cmd_quota_set) CONFIGURES the
//!     trusted [`QuotaPolicy`] override (quotas are user-configurable). This is a
//!     PRIVILEGED, trust-gated admin op: the command-level RBAC gate
//!     ([`authorize`](super::super::auth::authorize)) restricts it to the Owner, and
//!     the override is persisted in the LOCAL-ONLY KV namespace, so it stays per-install
//!     config that enforcement reads from durable state — never from the write being
//!     checked. A write can therefore never widen its own quota.
//!
//! CONFIG IS TRUSTED STATE (the DL-22 lesson): the *enforcement* read of the policy
//! (`enforce_records_write_tx` / `put_attachment`) always comes from persisted state,
//! and the only way to change it is this Owner-gated command — not an applet's `ctx.*`
//! request payload. `quota.status` likewise reads the trusted policy + the persisted
//! usage, with no wall clock on the path, so the report is deterministic.

use forge_domain::{CoreError, Result};
use forge_storage::{QuotaCategory, QuotaPolicy, QuotaScope, QuotaUsage};

use super::super::WorkspaceCore;
use crate::bridge::QUOTA_APPROACHING_SUGGESTION;

impl WorkspaceCore {
    /// `quota.status` — report DL-22 storage usage vs. the trusted limits plus the
    /// non-blocking approaching-limit warnings (`forge/spec/quotas.md` §1/§3). Returns:
    ///
    /// ```jsonc
    /// {
    ///   "usage":  { workspace_total_bytes, per_applet:[…], per_category:[…] },
    ///   "policy": { workspace_limit, per_applet_limit, …caps…, approaching_threshold },
    ///   "approaching": [ { scope, projected, limit, suggestion } ]   // ≥ threshold
    /// }
    /// ```
    ///
    /// The `approaching` list names every budget (workspace, each applet, each
    /// category) whose CURRENT usage already sits at/above the policy's approaching
    /// threshold (default ≥ 80%) — surfaced with the DL-22 remedy suggestion
    /// (compaction/cleanup/export), NEVER a deletion. Empty when every budget has
    /// headroom to spare. The usage and policy are read PURELY from the trusted
    /// persisted state (no request input, no wall clock), so two `quota.status` reads
    /// of an unchanged workspace are byte-equal and a replay reproduces them.
    ///
    /// Authorization: scope is the whole workspace, read from trusted state — the
    /// payload names nothing. The command-level role gate
    /// ([`authorize`](super::super::auth::authorize)) admits the read-membership roles
    /// (a storage-usage report is an oversight/read operation).
    pub(in crate::workspace) fn cmd_quota_status(
        &mut self,
        _cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let usage = self.quota_usage()?;
        let policy = self.quota_policy()?;
        let approaching = approaching_warnings(&usage, &policy);
        Ok(serde_json::json!({
            "usage": usage,
            "policy": policy,
            "approaching": approaching,
        }))
    }

    /// `quota.set` — configure the trusted DL-22 [`QuotaPolicy`] override (quotas are
    /// user-configurable; `forge/spec/quotas.md` §2). Payload `{ policy: { …fields… } }`
    /// where `policy` names ONLY the limits to change; every field is OPTIONAL and
    /// overlays onto the CURRENT effective policy (a partial set leaves the rest
    /// untouched). The accepted fields:
    ///
    ///   `workspace_limit`, `per_applet_limit`, `attachments_cap`, `run_logs_cap`,
    ///   `retained_chunks_cap`, `snapshots_cap`, `cache_cap` (byte counts), and
    ///   `approaching_threshold` (a fraction in `(0, 1]`).
    ///
    /// The merged policy is VALIDATED before it lands ([`Store::set_quota_policy`]):
    /// a zero limit or a threshold outside `(0, 1]` is rejected as a `ValidationError`
    /// rather than silently disabling enforcement. On success the override is persisted
    /// in the LOCAL-ONLY KV namespace (per-install config, out of synced/exported
    /// bundles) and the effective policy is returned.
    ///
    /// PRIVILEGED + TRUST-GATED: the command-level role gate
    /// ([`authorize`](super::super::auth::authorize)) restricts `quota.set` to the
    /// Owner — configuring quotas is workspace administration. Because enforcement
    /// reads the policy from THIS persisted state (never the write's payload), this
    /// Owner-gated command is the only way the trusted config changes; an applet's
    /// `ctx.*` write can never reach it, so a write cannot widen its own quota.
    pub(in crate::workspace) fn cmd_quota_set(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let overrides = cmd.payload.get("policy").ok_or_else(|| {
            CoreError::ValidationError("quota.set requires a `policy` object".into())
        })?;
        // Overlay the named fields onto the CURRENT effective trusted policy (a partial
        // set changes only what it names), then validate + persist through the trusted
        // seam. Reading the base from the effective policy — not the const default —
        // means two successive partial sets compose, matching "user-configurable".
        let merged = merge_policy_override(self.quota_policy()?, overrides)?;
        self.set_quota_policy(&merged)?;
        Ok(serde_json::json!({ "policy": merged }))
    }
}

/// Build the DL-22 approaching-limit warnings for a `quota.status` report: a warning
/// for EVERY budget (the workspace total, each per-applet collection budget, and each
/// category cap) whose CURRENT usage sits at/above the policy's approaching threshold,
/// each compared DIRECTLY against its OWN limit (the usage is the already-persisted
/// state, so `write_bytes = 0`). A budget that is OVER its limit (which an in-flight
/// write would have been rejected for, but a tightened policy can produce against prior
/// data) is also surfaced — `quota.status` is a non-blocking REPORT, not a write gate,
/// so it never errors; it just flags the budget so the user can act.
///
/// Review 177 P2: this enumerates EVERY bucket independently rather than asking
/// `decide_quota` per group — `decide_quota` returns only the FIRST over-cap scope or
/// the single strongest approaching scope, so simultaneous workspace + category (or
/// multiple-applet) warnings masked each other. The spec
/// (`spec/quotas.md` §6) and the command doc promise the report lists ALL of them.
///
/// Deterministic: a pure function of the persisted usage + the trusted policy with no
/// wall clock. The emit order is stable — workspace, then each applet in `per_applet`
/// order, then each category in [`QuotaCategory::ALL`] order — so two reads of an
/// unchanged workspace are byte-equal.
fn approaching_warnings(usage: &QuotaUsage, policy: &QuotaPolicy) -> Vec<serde_json::Value> {
    let mut out = Vec::new();
    // Workspace total against the workspace limit.
    push_if_approaching(
        &mut out,
        policy,
        QuotaScope::Workspace,
        usage.workspace_total_bytes,
        policy.workspace_limit,
    );
    // Each per-applet collection budget against the per-applet limit.
    for applet in &usage.per_applet {
        push_if_approaching(
            &mut out,
            policy,
            QuotaScope::Applet { applet: applet.applet.clone() },
            applet.collections_bytes,
            policy.per_applet_limit,
        );
    }
    // Each category against its own cap.
    for category in QuotaCategory::ALL {
        push_if_approaching(
            &mut out,
            policy,
            QuotaScope::Category { category },
            usage.category_bytes(category),
            policy.category_cap(category),
        );
    }
    out
}

/// Append a warning row for `scope` iff `usage` is at/above the policy's approaching
/// threshold of `limit` (or already over it). Below the threshold ⇒ the budget has
/// headroom and nothing is appended. The threshold comparison matches the live write
/// path (`decide_quota`): at/above `limit * approaching_threshold`.
fn push_if_approaching(
    out: &mut Vec<serde_json::Value>,
    policy: &QuotaPolicy,
    scope: QuotaScope,
    usage: u64,
    limit: u64,
) {
    if (usage as f64) < (limit as f64) * policy.approaching_threshold {
        return;
    }
    out.push(serde_json::json!({
        "scope": scope_token(&scope),
        "projected": usage,
        "limit": limit,
        "suggestion": QUOTA_APPROACHING_SUGGESTION,
    }));
}

/// The stable machine token a `quota.status` warning names its budget by (mirrors the
/// `ctx.db` write-path warning's `scope` token): `workspace`, `applet:<name>`, or
/// `category:<name>`.
fn scope_token(scope: &QuotaScope) -> String {
    match scope {
        QuotaScope::Workspace => "workspace".to_string(),
        QuotaScope::Applet { applet } => format!("applet:{applet}"),
        QuotaScope::Category { category } => format!("category:{}", category.as_str()),
    }
}

/// Overlay the OPTIONAL `quota.set` payload fields onto `base`, returning the merged
/// [`QuotaPolicy`]. Each field is independently optional (a partial set), a present
/// field must be the right JSON type, and an unknown field is rejected so a typo never
/// silently no-ops. Validation of the MERGED policy (non-zero limits, threshold in
/// `(0, 1]`) happens when it is persisted ([`Store::set_quota_policy`]).
fn merge_policy_override(mut base: QuotaPolicy, overrides: &serde_json::Value) -> Result<QuotaPolicy> {
    let obj = overrides.as_object().ok_or_else(|| {
        CoreError::ValidationError("quota.set `policy` must be an object".into())
    })?;
    for (key, value) in obj {
        match key.as_str() {
            "workspace_limit" => base.workspace_limit = take_u64(key, value)?,
            "per_applet_limit" => base.per_applet_limit = take_u64(key, value)?,
            "attachments_cap" => base.attachments_cap = take_u64(key, value)?,
            "run_logs_cap" => base.run_logs_cap = take_u64(key, value)?,
            "retained_chunks_cap" => base.retained_chunks_cap = take_u64(key, value)?,
            "snapshots_cap" => base.snapshots_cap = take_u64(key, value)?,
            "cache_cap" => base.cache_cap = take_u64(key, value)?,
            "approaching_threshold" => {
                base.approaching_threshold = value.as_f64().ok_or_else(|| {
                    CoreError::ValidationError(format!(
                        "quota.set `policy.{key}` must be a number, got {value}"
                    ))
                })?
            }
            other => {
                return Err(CoreError::ValidationError(format!(
                    "quota.set `policy` has unknown field `{other}`"
                )))
            }
        }
    }
    Ok(base)
}

/// Read a required unsigned-integer byte-count policy field, rejecting a
/// non-integer / negative value (a byte limit cannot be either).
fn take_u64(key: &str, value: &serde_json::Value) -> Result<u64> {
    value.as_u64().ok_or_else(|| {
        CoreError::ValidationError(format!(
            "quota.set `policy.{key}` must be a non-negative integer byte count, got {value}"
        ))
    })
}
