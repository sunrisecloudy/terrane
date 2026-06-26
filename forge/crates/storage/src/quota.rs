//! DL-22 workspace quotas: deterministic size accounting, a trusted policy
//! (const defaults + a persisted override), and a reject-not-delete enforcement
//! check, plus the content-addressed attachment store the `attachments` category
//! accounts.
//!
//! Normative spec: `forge/spec/quotas.md`. The contract, in one place:
//!
//! - **Size accounting is a PURE function of persisted state.** [`Store::quota_usage`]
//!   sums the bytes already on disk (records, CRDT chunks, oplog, run logs, audit
//!   log, attachments) with NO wall clock and NO request input, so the same file
//!   always reports the same usage and a replay reproduces the same numbers.
//! - **The policy is TRUSTED config, not request payload.** [`QuotaPolicy`] is a
//!   `const` default (1 GiB/workspace, 100 MiB/applet, per-category caps) overlaid
//!   with a persisted override read from a LOCAL-ONLY KV namespace — never from the
//!   write being checked.
//! - **Enforcement REJECTS, it NEVER deletes.** [`Store::check_quota`] returns
//!   [`QuotaDecision::Ok`], [`QuotaDecision::ApproachingLimit`] (a non-blocking
//!   warning at/above the threshold), or a typed `ResourceLimitExceeded` error
//!   ([`QuotaDecision::over_quota_error`]) suggesting compaction/cleanup/export. An
//!   over-quota write is blocked at the write boundary; existing data is left
//!   byte-for-byte intact.
//! - **Attachments are deduplicated by content hash.** [`Store::put_attachment`]
//!   stores the bytes once per `sha256:` content hash and refcounts re-puts, so
//!   identical bytes occupy one blob and are accounted once.

use forge_domain::{content_hash, CoreError, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::errors::{map_json, map_sql};
use crate::kv::kv_set_tx;
use crate::store::{now_ms, Store};

/// The LOCAL-ONLY KV namespace the trusted [`QuotaPolicy`] override is persisted
/// in. It begins with `__local` so [`is_local_only_namespace`](crate::is_local_only_namespace)
/// keeps it OUT of an exported/synced bundle: the policy is per-install trusted
/// config, not workspace content that should travel to a peer.
pub const QUOTA_META_NS: &str = "__local/quota";

/// The KV key (within [`QUOTA_META_NS`]) holding the persisted [`QuotaPolicy`]
/// override as canonical JSON. Absent ⇒ the [`QuotaPolicy::DEFAULT`] applies.
pub const QUOTA_POLICY_KEY: &str = "policy";

/// One mebibyte and one gibibyte, named so the default limits read in the units
/// DL-22 states them in (100 MiB / 1 GiB).
pub const MIB: u64 = 1024 * 1024;
/// One gibibyte (see [`MIB`]).
pub const GIB: u64 = 1024 * MIB;

/// A storage category the workspace caps independently (DL-22: "caps for
/// attachments, run logs, retained chunks/snapshots, cache"). Each maps to a
/// concrete slice of the persisted substrate that [`Store::quota_usage`] sums.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaCategory {
    /// Content-addressed attachment blobs (the `attachments` table), counted ONCE
    /// per content hash (dedup) — never multiplied by the number of references.
    Attachments,
    /// Run logs (`run_logs` + the `runs` records they belong to).
    RunLogs,
    /// Retained CRDT op chunks (`crdt_chunks`) — what compaction folds away.
    RetainedChunks,
    /// CRDT snapshots (`crdt_snapshots`).
    Snapshots,
    /// Local cache / scratch (the oplog + audit-log substrate). Bounded so an
    /// unbounded change feed cannot crowd out user data.
    Cache,
}

impl QuotaCategory {
    /// Every category, in a stable order (the order [`Store::quota_usage`] reports
    /// them and the order [`QuotaPolicy`] caps them).
    pub const ALL: [QuotaCategory; 5] = [
        QuotaCategory::Attachments,
        QuotaCategory::RunLogs,
        QuotaCategory::RetainedChunks,
        QuotaCategory::Snapshots,
        QuotaCategory::Cache,
    ];

    /// The stable machine token for this category (for reports/fixtures/errors).
    pub fn as_str(self) -> &'static str {
        match self {
            QuotaCategory::Attachments => "attachments",
            QuotaCategory::RunLogs => "run_logs",
            QuotaCategory::RetainedChunks => "retained_chunks",
            QuotaCategory::Snapshots => "snapshots",
            QuotaCategory::Cache => "cache",
        }
    }
}

/// Per-applet collection-storage usage: the bytes of every record whose
/// collection belongs to `applet`. `applet` is the collection-name prefix before
/// the first `/` (`tasks/inbox` ⇒ applet `tasks`), or the whole collection name
/// when it carries no prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppletUsage {
    pub applet: String,
    pub collections_bytes: u64,
}

/// One category's accounted bytes (a row of the per-category report).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategoryUsage {
    pub category: QuotaCategory,
    pub bytes: u64,
}

/// A deterministic snapshot of how much storage the workspace is using, summed
/// PURELY from the persisted SQLite tables (no wall clock, no request input).
///
/// `workspace_total_bytes` is the whole accounted footprint; `per_applet` breaks
/// the records projection down by owning applet; `per_category` reports the
/// independently-capped slices. Two reads of an unchanged store are byte-equal,
/// and a replay reproduces the exact numbers (the determinism lesson).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QuotaUsage {
    pub workspace_total_bytes: u64,
    pub per_applet: Vec<AppletUsage>,
    pub per_category: Vec<CategoryUsage>,
}

impl QuotaUsage {
    /// The accounted bytes for one category (0 if the category has no rows).
    pub fn category_bytes(&self, category: QuotaCategory) -> u64 {
        self.per_category
            .iter()
            .find(|c| c.category == category)
            .map(|c| c.bytes)
            .unwrap_or(0)
    }

    /// The collection-storage bytes attributed to `applet` (0 if none).
    pub fn applet_bytes(&self, applet: &str) -> u64 {
        self.per_applet
            .iter()
            .find(|a| a.applet == applet)
            .map(|a| a.collections_bytes)
            .unwrap_or(0)
    }
}

/// The trusted DL-22 quota configuration: the workspace + per-applet limits, the
/// per-category caps, and the approaching-limit warning threshold.
///
/// This is TRUSTED state. [`QuotaPolicy::DEFAULT`] is the `const` floor; a
/// per-install override is read from the local-only [`QUOTA_META_NS`] (never from
/// the request payload being checked), so a write can never widen its own quota.
///
/// `approaching_threshold` is an `f64`, so this derives `PartialEq` but not `Eq`
/// (a fraction has no total equality). Comparisons in tests/decisions only need
/// `PartialEq`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct QuotaPolicy {
    /// Total workspace local budget (DL-22 default 1 GiB).
    pub workspace_limit: u64,
    /// Per-applet collection-storage budget (DL-22 default 100 MiB).
    pub per_applet_limit: u64,
    /// Cap for attachment blobs (counted once per content hash).
    pub attachments_cap: u64,
    /// Cap for run logs.
    pub run_logs_cap: u64,
    /// Cap for retained CRDT op chunks.
    pub retained_chunks_cap: u64,
    /// Cap for CRDT snapshots.
    pub snapshots_cap: u64,
    /// Cap for local cache/scratch (oplog + audit substrate).
    pub cache_cap: u64,
    /// Fraction of a limit at/above which a write is still ALLOWED but surfaces a
    /// non-blocking [`QuotaDecision::ApproachingLimit`] warning (DL-22 "approaching
    /// limits → suggest compaction/cleanup/export"). DL-22 default 0.8 (80%).
    pub approaching_threshold: f64,
    /// Budget-error count threshold for auto-quarantine (D12/Q2 default 3 / 60s).
    #[serde(default = "default_auto_quarantine_error_threshold")]
    pub auto_quarantine_error_threshold: u32,
    /// Rolling window in seconds for auto-quarantine budget errors (D12 default 60).
    #[serde(default = "default_auto_quarantine_window_seconds")]
    pub auto_quarantine_window_seconds: u32,
}

fn default_auto_quarantine_error_threshold() -> u32 {
    3
}

fn default_auto_quarantine_window_seconds() -> u32 {
    60
}

impl QuotaPolicy {
    /// The DL-22 default policy: 1 GiB workspace, 100 MiB per applet, per-category
    /// caps, and an 80% approaching-limit threshold. The `const` trusted floor a
    /// fresh workspace runs under until an override is persisted.
    pub const DEFAULT: QuotaPolicy = QuotaPolicy {
        workspace_limit: GIB,
        per_applet_limit: 100 * MIB,
        // Category caps are generous relative to the per-applet/workspace limits so
        // the workspace/applet budgets bite first in normal use; the caps are the
        // backstop DL-22 calls for on attachments/run logs/retained chunks/snapshots/
        // cache so no single category can consume the whole workspace budget.
        attachments_cap: 512 * MIB,
        run_logs_cap: 256 * MIB,
        retained_chunks_cap: 256 * MIB,
        snapshots_cap: 256 * MIB,
        cache_cap: 128 * MIB,
        approaching_threshold: 0.8,
        auto_quarantine_error_threshold: 3,
        auto_quarantine_window_seconds: 60,
    };

    /// This policy's cap for `category`.
    pub fn category_cap(&self, category: QuotaCategory) -> u64 {
        match category {
            QuotaCategory::Attachments => self.attachments_cap,
            QuotaCategory::RunLogs => self.run_logs_cap,
            QuotaCategory::RetainedChunks => self.retained_chunks_cap,
            QuotaCategory::Snapshots => self.snapshots_cap,
            QuotaCategory::Cache => self.cache_cap,
        }
    }

    /// Validate the trusted invariants: every limit/cap must be non-zero and the
    /// approaching threshold must be in `(0, 1]`. A persisted override that fails
    /// this is rejected rather than silently disabling enforcement (a `0` limit
    /// would reject every write; a threshold outside `(0,1]` is meaningless).
    fn validate(&self) -> Result<()> {
        let limits = [
            ("workspace_limit", self.workspace_limit),
            ("per_applet_limit", self.per_applet_limit),
            ("attachments_cap", self.attachments_cap),
            ("run_logs_cap", self.run_logs_cap),
            ("retained_chunks_cap", self.retained_chunks_cap),
            ("snapshots_cap", self.snapshots_cap),
            ("cache_cap", self.cache_cap),
        ];
        for (name, value) in limits {
            if value == 0 {
                return Err(CoreError::ValidationError(format!(
                    "quota policy {name} must be greater than zero"
                )));
            }
        }
        if !(self.approaching_threshold > 0.0 && self.approaching_threshold <= 1.0) {
            return Err(CoreError::ValidationError(format!(
                "quota approaching_threshold must be in (0, 1], got {}",
                self.approaching_threshold
            )));
        }
        Ok(())
    }
}

impl Default for QuotaPolicy {
    fn default() -> Self {
        QuotaPolicy::DEFAULT
    }
}

/// What kind of limit a [`QuotaDecision`] is reported against, so a warning or a
/// rejection can name the exact budget it touched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum QuotaScope {
    /// The whole-workspace budget.
    Workspace,
    /// A named applet's per-applet collection budget.
    Applet { applet: String },
    /// A storage category's cap.
    Category { category: QuotaCategory },
}

impl QuotaScope {
    fn describe(&self) -> String {
        match self {
            QuotaScope::Workspace => "workspace".to_string(),
            QuotaScope::Applet { applet } => format!("applet {applet:?}"),
            QuotaScope::Category { category } => format!("category {}", category.as_str()),
        }
    }
}

/// The outcome of a [`Store::check_quota`] decision: a PURE function of the
/// persisted usage, the trusted policy, and the incoming write size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum QuotaDecision {
    /// The write fits with headroom to spare (below the approaching threshold of
    /// every relevant limit). Allowed.
    Ok,
    /// The write fits but lands at/above the approaching threshold of `scope`'s
    /// limit. ALLOWED (non-blocking) — the caller surfaces a warning suggesting
    /// compaction/cleanup/export. `projected`/`limit` are the post-write bytes and
    /// the limit it is approaching.
    ApproachingLimit {
        scope: QuotaScope,
        projected: u64,
        limit: u64,
    },
    /// The write would exceed `scope`'s limit. REJECTED — the caller must block the
    /// write and surface [`over_quota_error`](QuotaDecision::over_quota_error);
    /// existing data is never deleted or evicted to make room.
    OverQuota {
        scope: QuotaScope,
        projected: u64,
        limit: u64,
    },
}

impl QuotaDecision {
    /// True iff the write must be BLOCKED (an over-quota rejection). `Ok` and
    /// `ApproachingLimit` are both allowed.
    pub fn is_over_quota(&self) -> bool {
        matches!(self, QuotaDecision::OverQuota { .. })
    }

    /// True iff this is the non-blocking approaching-limit warning.
    pub fn is_approaching(&self) -> bool {
        matches!(self, QuotaDecision::ApproachingLimit { .. })
    }

    /// The typed `ResourceLimitExceeded` error a rejected write surfaces, naming the
    /// scope and suggesting the DL-22 remedies (compaction/cleanup/export) — NEVER a
    /// silent deletion. Returns `None` for an allowed decision.
    pub fn over_quota_error(&self) -> Option<CoreError> {
        match self {
            QuotaDecision::OverQuota {
                scope,
                projected,
                limit,
            } => Some(CoreError::ResourceLimitExceeded(format!(
                "quota exceeded for {}: {projected} bytes would exceed the {limit}-byte limit. \
                 Free space by compacting history, cleaning up run logs/old attachments, or \
                 exporting and archiving data — no data was deleted.",
                scope.describe()
            ))),
            _ => None,
        }
    }
}

/// The applet that owns `collection`: the prefix before the first `/`, or the whole
/// name when there is no `/`. So `tasks/inbox` ⇒ `tasks` and `notes` ⇒ `notes`.
/// Pure string work — the same accounting key a record's collection maps to.
pub fn applet_of_collection(collection: &str) -> &str {
    match collection.split_once('/') {
        Some((applet, _)) => applet,
        None => collection,
    }
}

/// Compute the deterministic [`QuotaUsage`] report from the persisted tables on
/// `conn` (a `Store` connection OR an open transaction, since `Transaction` derefs
/// to `Connection`). The shared accounting engine behind both
/// [`Store::quota_usage`] and the tx-scoped write-boundary check, so the usage a
/// write is enforced against is the SAME usage a `quota.status` reports — no skew.
///
/// PURE: every number is summed from bytes already on disk with NO wall clock and
/// NO request input.
pub(crate) fn usage_from_conn(conn: &Connection) -> Result<QuotaUsage> {
    let per_applet = per_applet_usage(conn)?;
    let per_category = QuotaCategory::ALL
        .iter()
        .map(|&category| {
            Ok(CategoryUsage {
                category,
                bytes: category_bytes(conn, category)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    // The workspace total is the sum of every accounted slice: per-applet collection
    // storage PLUS every category. (Records live under per_applet; the categories
    // cover the substrate that backs them and the side stores.)
    let collections_total: u64 = per_applet.iter().map(|a| a.collections_bytes).sum();
    let categories_total: u64 = per_category.iter().map(|c| c.bytes).sum();
    Ok(QuotaUsage {
        workspace_total_bytes: collections_total + categories_total,
        per_applet,
        per_category,
    })
}

/// Per-applet collection-storage bytes: the summed `length(data)` of every
/// `records` row, grouped by the owning applet (the collection-name prefix).
fn per_applet_usage(conn: &Connection) -> Result<Vec<AppletUsage>> {
    let mut stmt = conn
        .prepare(
            "SELECT collection, SUM(length(data)) FROM records
              GROUP BY collection ORDER BY collection",
        )
        .map_err(map_sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(map_sql)?;
    // Group collections by owning applet, accumulating bytes. A BTreeMap keeps the
    // report in a stable (sorted) applet order so it is byte-deterministic.
    let mut by_applet: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    for row in rows {
        let (collection, bytes) = row.map_err(map_sql)?;
        let applet = applet_of_collection(&collection).to_string();
        *by_applet.entry(applet).or_insert(0) += bytes.max(0) as u64;
    }
    Ok(by_applet
        .into_iter()
        .map(|(applet, collections_bytes)| AppletUsage {
            applet,
            collections_bytes,
        })
        .collect())
}

/// The accounted bytes for one category, summed from its backing table(s). A pure
/// `SUM(length(...))` (or `SUM(byte_len)` for attachments) over persisted rows —
/// `coalesce` so an empty table reports `0`, not `NULL`.
fn category_bytes(conn: &Connection, category: QuotaCategory) -> Result<u64> {
    let sql = match category {
        // Attachments are accounted ONCE per content hash (dedup): summing `byte_len`
        // over the unique rows counts each stored blob a single time regardless of
        // how many records reference it.
        QuotaCategory::Attachments => "SELECT coalesce(SUM(byte_len), 0) FROM attachments",
        QuotaCategory::RunLogs => {
            "SELECT (SELECT coalesce(SUM(length(payload)), 0) FROM run_logs)
                  + (SELECT coalesce(SUM(length(record_json)), 0) FROM runs)"
        }
        QuotaCategory::RetainedChunks => "SELECT coalesce(SUM(length(payload)), 0) FROM crdt_chunks",
        QuotaCategory::Snapshots => {
            "SELECT coalesce(SUM(length(payload)) + SUM(length(frontier)), 0) FROM crdt_snapshots"
        }
        QuotaCategory::Cache => {
            "SELECT (SELECT coalesce(SUM(length(payload)), 0) FROM oplog)
                  + (SELECT coalesce(SUM(length(metadata)), 0) FROM audit_log)"
        }
    };
    let bytes: i64 = conn.query_row(sql, [], |row| row.get(0)).map_err(map_sql)?;
    Ok(bytes.max(0) as u64)
}

/// The effective trusted [`QuotaPolicy`] read off `conn`: the persisted override
/// from the local-only [`QUOTA_META_NS`], or [`QuotaPolicy::DEFAULT`] when none is
/// set. Shared by [`Store::quota_policy`] and the tx-scoped check so both read the
/// SAME trusted config. A persisted override that fails [`QuotaPolicy::validate`]
/// surfaces an error rather than silently disabling enforcement.
pub(crate) fn policy_from_conn(conn: &Connection) -> Result<QuotaPolicy> {
    let bytes: Option<Vec<u8>> = conn
        .query_row(
            "SELECT value FROM kv WHERE namespace = ?1 AND key = ?2 AND tombstone = 0",
            params![QUOTA_META_NS, QUOTA_POLICY_KEY],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        )
        .optional()
        .map_err(map_sql)?
        .flatten();
    match bytes {
        Some(bytes) => {
            let policy: QuotaPolicy =
                serde_json::from_slice(&bytes).map_err(|e| map_json("quota policy decode", e))?;
            policy.validate()?;
            Ok(policy)
        }
        None => Ok(QuotaPolicy::DEFAULT),
    }
}

/// Enforce the DL-22 quota for a records write into `collection`, AFTER the chunk +
/// oplog row + projection have been STAGED inside the caller's open transaction `tx`
/// but BEFORE it commits (review 176 P1).
///
/// This is the LIVE write-boundary check, and it charges EXACTLY the slices
/// [`quota_usage`] counts — not just the chunk payload. A records write grows several
/// accounted slices at once: the per-applet `records.data` projection, the
/// `retained_chunks` (the new CRDT chunk), and the `cache` (the new oplog row's
/// payload). The earlier pre-write check looked only at `chunk_payload.len()`, so a
/// write could pass while the post-commit `quota_usage` it is enforced against landed
/// MORE accounted bytes (records + oplog), leaving the workspace over quota
/// immediately after an "accepted" write. To make the gate consistent with the report,
/// the caller STAGES the full write first; this then recomputes `usage_from_conn(tx)`
/// — which now reflects every staged slice exactly as a post-commit `quota_usage`
/// would — and compares the REAL post-write totals against the trusted limits.
///
/// The touched budgets are the workspace total, the per-applet collection budget
/// (charged against the projected `records.data` total, not the chunk bytes), and
/// every category cap the write grew (`retained_chunks` and `cache`). An over-quota
/// breach of ANY of them returns the typed `ResourceLimitExceeded` error, which the
/// caller propagates — rolling the whole transaction back, so the chunk, oplog row,
/// and projection are NEVER persisted and no existing data is deleted
/// (reject-not-delete). The check reads off `tx`, so it is the real staged state, and
/// it is a PURE function of that state + the trusted policy (no wall clock), so a
/// replay reproduces the same accept/reject decision.
pub(crate) fn enforce_records_write_tx(
    tx: &rusqlite::Transaction<'_>,
    collection: &str,
) -> Result<()> {
    let usage = usage_from_conn(tx)?;
    let policy = policy_from_conn(tx)?;
    let applet = applet_of_collection(collection);

    // Compare each touched budget's REAL post-write total (already reflecting the
    // staged chunk + oplog + projection) against its limit; the FIRST breach rejects.
    // Order: workspace, then per-applet, then the two categories the write grew —
    // a stable order for a deterministic message.
    let checks: [(QuotaScope, u64, u64); 4] = [
        (
            QuotaScope::Workspace,
            usage.workspace_total_bytes,
            policy.workspace_limit,
        ),
        (
            QuotaScope::Applet { applet: applet.to_string() },
            usage.applet_bytes(applet),
            policy.per_applet_limit,
        ),
        (
            QuotaScope::Category { category: QuotaCategory::RetainedChunks },
            usage.category_bytes(QuotaCategory::RetainedChunks),
            policy.category_cap(QuotaCategory::RetainedChunks),
        ),
        (
            QuotaScope::Category { category: QuotaCategory::Cache },
            usage.category_bytes(QuotaCategory::Cache),
            policy.category_cap(QuotaCategory::Cache),
        ),
    ];
    for (scope, projected, limit) in checks {
        if let Some(err) = over_quota_breach(scope, projected, limit) {
            return Err(err);
        }
    }
    Ok(())
}

/// `Some(ResourceLimitExceeded)` iff `projected` exceeds `limit` for `scope` (the
/// typed over-quota error naming the scope + suggesting the DL-22 remedies, never a
/// deletion), else `None`. The shared post-write breach check behind the records-write
/// and attachment enforcement paths, so both reject with the identical typed error.
fn over_quota_breach(scope: QuotaScope, projected: u64, limit: u64) -> Option<CoreError> {
    if projected > limit {
        QuotaDecision::OverQuota { scope, projected, limit }.over_quota_error()
    } else {
        None
    }
}

/// The PRE-FLIGHT run-admission decision (review 178): read the ALREADY-COMMITTED
/// `quota_usage` and the trusted [`QuotaPolicy`] and decide whether a NEW run may
/// START — BEFORE any applet side effect runs. Returns the typed
/// `ResourceLimitExceeded` (the DL-22 compaction/cleanup/export suggestion) when the
/// workspace has no budget left to admit a run, else `None`.
///
/// WHY a pre-flight gate, not a post-execution one (review 178 P1): each `ctx.db` write
/// an applet makes commits its CRDT mutation to SQLite IMMEDIATELY as the applet runs
/// (`apply_mutation_crdt`), so the applet's record writes are durable the moment it
/// executes. Gating the MANDATORY run record (CR-9) AFTER the applet ran could then
/// reject the run record while the applet's writes are already committed — leaving
/// UNREPLAYABLE side effects (durable writes with no run record to replay from). This
/// check runs BEFORE the applet/handler/callback executes: because nothing has run yet,
/// a rejection leaves NO new records, NO UI state, and NO callback writes — no torn,
/// unreplayable state. Once a run is ADMITTED, its run record ALWAYS persists
/// ([`save_run_tx`](Self::save_run_tx)); the mandatory record may push the run_logs
/// category up to one record past its cap, and the NEXT run is then rejected here.
///
/// A run is REFUSED when the run_logs category is already exhausted: its committed usage
/// sits at/over `run_logs_cap`, so there is no headroom for a new mandatory run record.
///
/// The workspace TOTAL is deliberately NOT gated here (`spec/quotas.md` §6): a run that
/// FAILS because its `ctx.db` write was rejected at the records boundary committed NO
/// durable write (its records transaction rolled back) — it has no unreplayable side
/// effect — and its *failed* run record is the auditable record of that very rejection,
/// which must survive even when the workspace is at `workspace_limit`. Gating admission
/// on the total would drop that audit trail at exactly the moment the workspace is full,
/// which reject-not-delete forbids. The run_logs cap is the dedicated DL-22 backstop that
/// bounds run records.
///
/// PURE + DETERMINISTIC: a function of the COMMITTED usage + the trusted policy with no
/// wall clock and no request input, so it is in the LIVE command path only — a rejected
/// run has no record to replay, and an admitted run replays from its recorded run, so
/// the demo stays REPLAY IDENTICAL.
fn admit_run_decision(usage: &QuotaUsage, policy: &QuotaPolicy) -> Option<CoreError> {
    // No run-log headroom: the committed run_logs usage already sits at/over the cap, so
    // there is no budget to admit even the mandatory run record of a new run. Reject
    // before the applet runs (reject-not-delete: existing run logs are kept; the user
    // must compact/clean up/export to free budget before starting new runs).
    let run_logs = usage.category_bytes(QuotaCategory::RunLogs);
    let run_logs_cap = policy.category_cap(QuotaCategory::RunLogs);
    if run_logs >= run_logs_cap {
        return QuotaDecision::OverQuota {
            scope: QuotaScope::Category { category: QuotaCategory::RunLogs },
            projected: run_logs,
            limit: run_logs_cap,
        }
        .over_quota_error();
    }
    None
}

impl Store {
    // --- Size accounting (DL-22, pure function of persisted state) --------

    /// Compute the deterministic [`QuotaUsage`] report from the persisted tables.
    ///
    /// PURE: every number is summed from bytes already on disk — `length(...)` over
    /// `records`/`crdt_chunks`/`crdt_snapshots`/`oplog`/`run_logs`/`runs`/`audit_log`
    /// and `byte_len` over the deduplicated `attachments` rows — with NO wall clock
    /// and NO request input. Two reads of an unchanged store are byte-equal and a
    /// replay reproduces the same report (the DL-22 determinism contract).
    pub fn quota_usage(&self) -> Result<QuotaUsage> {
        usage_from_conn(&self.conn)
    }

    // --- Policy: trusted config (const default + persisted override) -----

    /// The effective trusted [`QuotaPolicy`]: the persisted override from the
    /// local-only [`QUOTA_META_NS`], or [`QuotaPolicy::DEFAULT`] when none is set.
    ///
    /// TRUSTED state: read from durable workspace config, NEVER from the request
    /// being checked. A persisted override that fails [`QuotaPolicy::validate`]
    /// surfaces an error rather than silently disabling enforcement.
    pub fn quota_policy(&self) -> Result<QuotaPolicy> {
        policy_from_conn(&self.conn)
    }

    /// Persist a trusted [`QuotaPolicy`] override (user-configurable quotas, DL-22).
    /// Validated before it lands, and stored in the local-only [`QUOTA_META_NS`] so
    /// it stays per-install config (out of synced/exported bundles).
    pub fn set_quota_policy(&mut self, policy: &QuotaPolicy) -> Result<()> {
        policy.validate()?;
        let json = serde_json::to_vec(policy).map_err(|e| map_json("quota policy encode", e))?;
        self.transact(|tx| kv_set_tx(tx, QUOTA_META_NS, QUOTA_POLICY_KEY, &json, "application/json"))
    }

    // --- Enforcement: reject-not-delete (DL-22) --------------------------

    /// Decide whether a `write_bytes`-byte write into `category` (optionally owned by
    /// `applet`) is allowed, approaching a limit, or over quota.
    ///
    /// PURE: a function of the current [`quota_usage`](Self::quota_usage), the trusted
    /// [`quota_policy`](Self::quota_policy), and `write_bytes` — no wall clock, no
    /// request payload. The projected post-write total is compared against the
    /// workspace limit, the per-applet limit (when `applet` is given), and the
    /// category cap; the TIGHTEST limit decides. An over-quota result REJECTS the
    /// write (the caller surfaces [`QuotaDecision::over_quota_error`]); it never
    /// deletes or evicts existing data to make room.
    pub fn check_quota(
        &self,
        category: QuotaCategory,
        applet: Option<&str>,
        write_bytes: u64,
    ) -> Result<QuotaDecision> {
        let usage = self.quota_usage()?;
        let policy = self.quota_policy()?;
        Ok(decide_quota(&usage, &policy, category, applet, write_bytes))
    }

    /// The DL-22 quota status of `collection`'s applet+workspace+retained-chunks
    /// budgets as of the CURRENT persisted state — the seam the live host boundary
    /// uses to surface the non-blocking APPROACHING-LIMIT warning AFTER a records
    /// write has already committed.
    ///
    /// This is [`check_quota`](Self::check_quota) with `write_bytes = 0`: the write
    /// already landed, so its bytes are already counted in
    /// [`quota_usage`](Self::quota_usage); we only ask whether the post-write totals
    /// now sit at/above the approaching threshold (`ApproachingLimit`) or still have
    /// headroom (`Ok`). It can never return `OverQuota` here — an over-quota write was
    /// already REJECTED at the write boundary ([`enforce_records_write_tx`]) and rolled
    /// back, so a committed write is by construction within every limit.
    ///
    /// PURE + DETERMINISTIC: a function of the persisted usage and the trusted policy
    /// with NO wall clock and NO request input, so a replay of the same writes
    /// reproduces the same warning (the determinism lesson). The category is
    /// [`RetainedChunks`](QuotaCategory::RetainedChunks) — the slice a records write
    /// grows — matching what `enforce_records_write_tx` charges the write against.
    pub fn records_write_quota_status(&self, collection: &str) -> Result<QuotaDecision> {
        let applet = applet_of_collection(collection);
        self.check_quota(QuotaCategory::RetainedChunks, Some(applet), 0)
    }

    /// The PRE-FLIGHT run-admission gate (review 178): may a NEW run START? Reads the
    /// ALREADY-COMMITTED [`quota_usage`](Self::quota_usage) and the trusted
    /// [`quota_policy`](Self::quota_policy) and returns the typed `ResourceLimitExceeded`
    /// (the DL-22 compaction/cleanup/export suggestion) when the workspace has no budget
    /// to admit a run, else `Ok(())`.
    ///
    /// This MUST be called BEFORE any applet side effect runs — before `runtime.run`'s
    /// recorded execution, before a `ui.dispatch_event` handler, and before a `db.watch`
    /// callback. Each `ctx.db` write an applet makes commits to SQLite immediately as the
    /// applet runs, so its record writes are durable the instant it executes; gating the
    /// MANDATORY run record (CR-9) AFTER the fact would let a rejection strand those
    /// committed writes with no run record to replay from (review 178 P1: unreplayable
    /// side effects). Because this check runs first, a rejection leaves NO new records,
    /// NO UI state, and NO callback writes — no torn state. Once admitted, the run record
    /// ALWAYS persists via [`save_run_tx`](Self::save_run_tx).
    ///
    /// A run is REFUSED when the committed run_logs usage already sits at/over
    /// `run_logs_cap` (no headroom for a new mandatory run record). The workspace total is
    /// deliberately NOT gated here (`spec/quotas.md` §6): a run that FAILS because its
    /// `ctx.db` write was rejected at the records boundary committed no durable write, and
    /// its *failed* run record — the audit trail of that rejection — must survive even on
    /// a full workspace. Existing data is never deleted — the user compacts/cleans
    /// up/exports to free run-log budget before starting new runs (reject-not-delete).
    ///
    /// PURE + DETERMINISTIC: a function of committed state + the trusted policy with no
    /// wall clock and no request input — a rejected run has no record to replay and an
    /// admitted run replays from its recorded run, so a replay reproduces the same
    /// admit/reject decision.
    pub fn admit_run_or_reject(&self) -> Result<()> {
        let usage = self.quota_usage()?;
        let policy = self.quota_policy()?;
        match admit_run_decision(&usage, &policy) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }
}

/// The pure DL-22 quota decision: project the post-write totals and compare against
/// every relevant limit, returning the FIRST over-quota breach (so a hard rejection
/// dominates a soft warning) and otherwise the strongest approaching warning.
///
/// Factored out of [`Store::check_quota`] so it is unit-testable without a store and
/// is provably free of any I/O or wall clock — the determinism lesson made explicit.
pub fn decide_quota(
    usage: &QuotaUsage,
    policy: &QuotaPolicy,
    category: QuotaCategory,
    applet: Option<&str>,
    write_bytes: u64,
) -> QuotaDecision {
    // Each (scope, projected, limit) the write touches. The workspace total and the
    // category both grow by `write_bytes`; the per-applet budget grows only when the
    // write is attributed to an applet (a records write).
    let mut checks: Vec<(QuotaScope, u64, u64)> = vec![
        (
            QuotaScope::Workspace,
            usage.workspace_total_bytes.saturating_add(write_bytes),
            policy.workspace_limit,
        ),
        (
            QuotaScope::Category { category },
            usage.category_bytes(category).saturating_add(write_bytes),
            policy.category_cap(category),
        ),
    ];
    if let Some(applet) = applet {
        checks.push((
            QuotaScope::Applet {
                applet: applet.to_string(),
            },
            usage.applet_bytes(applet).saturating_add(write_bytes),
            policy.per_applet_limit,
        ));
    }

    // A hard breach of ANY limit rejects (reject-not-delete dominates). Among the
    // limits, surface the first breach in check order for a stable message.
    if let Some((scope, projected, limit)) = checks
        .iter()
        .find(|(_, projected, limit)| projected > limit)
    {
        return QuotaDecision::OverQuota {
            scope: scope.clone(),
            projected: *projected,
            limit: *limit,
        };
    }

    // No hard breach: surface a non-blocking approaching warning if the projected
    // total reaches the threshold of any limit. Pick the scope with the highest
    // utilization ratio so the warning names the tightest budget. Tie-break by check
    // ORDER (workspace, then category, then applet): on equal ratios keep the FIRST,
    // so the decision is deterministic and stable for fixtures (a later scope must be
    // STRICTLY more utilized to win).
    let mut best: Option<(QuotaScope, u64, u64)> = None;
    let mut best_ratio = 0.0_f64;
    for (scope, projected, limit) in checks {
        if (projected as f64) < (limit as f64) * policy.approaching_threshold {
            continue;
        }
        let ratio = projected as f64 / limit as f64;
        if best.is_none() || ratio > best_ratio {
            best = Some((scope, projected, limit));
            best_ratio = ratio;
        }
    }
    match best {
        Some((scope, projected, limit)) => QuotaDecision::ApproachingLimit {
            scope,
            projected,
            limit,
        },
        None => QuotaDecision::Ok,
    }
}

/// The result of a [`Store::put_attachment`]: the content hash the bytes are keyed
/// under, and whether the bytes were NEWLY stored (`true`) or an existing blob was
/// refcounted (`false`, the dedup hit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AttachmentPut {
    pub content_hash: String,
    /// `true` if a new blob row was written; `false` if identical bytes were already
    /// stored and only the refcount was bumped (deduplicated).
    pub stored_new: bool,
    /// The reference count AFTER this put.
    pub refcount: u64,
}

/// The atomic body of [`Store::put_attachment`], run inside one `BEGIN IMMEDIATE`
/// transaction (review 176 P2). With the writer lock already held, the dedup lookup,
/// the quota enforcement, and the insert/refcount-bump are one indivisible unit:
///
///   - If a blob for `hash` already exists, bump its refcount (`stored_new = false`);
///     a dedup hit adds no storage, so it is allowed even at quota.
///   - Otherwise STAGE the insert, then recompute usage off the SAME `tx` and enforce
///     the attachments cap + the workspace limit against the REAL post-insert totals
///     (matching `quota_usage`, mirroring the records path). Over quota ⇒ return the
///     typed error, which rolls the staged insert back (reject-not-delete).
///
/// Because the writer lock was taken before the lookup, a second concurrent handle
/// either sees the committed row (and dedups) or is enforced against the committed
/// higher usage — it can neither oversubscribe the cap nor race into a primary-key
/// error. The `INSERT … ON CONFLICT(content_hash) DO UPDATE` upsert keeps the insert
/// branch idempotent under that serialization.
fn put_attachment_tx(
    tx: &rusqlite::Transaction<'_>,
    hash: &str,
    bytes: &[u8],
    write_bytes: u64,
    created_at: i64,
) -> Result<AttachmentPut> {
    // Dedup lookup INSIDE the locked txn.
    let existing: Option<i64> = tx
        .query_row(
            "SELECT refcount FROM attachments WHERE content_hash = ?1",
            params![hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_sql)?;
    if let Some(current) = existing {
        let next = (current.max(0) as u64).saturating_add(1);
        tx.execute(
            "UPDATE attachments SET refcount = ?2 WHERE content_hash = ?1",
            params![hash, next as i64],
        )
        .map_err(map_sql)?;
        return Ok(AttachmentPut {
            content_hash: hash.to_string(),
            stored_new: false,
            refcount: next,
        });
    }
    // New bytes: STAGE the insert (idempotent upsert under the writer lock), then
    // enforce against the REAL post-insert usage.
    tx.execute(
        "INSERT INTO attachments (content_hash, bytes, byte_len, refcount, created_at)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT(content_hash) DO UPDATE SET refcount = refcount",
        params![hash, bytes, write_bytes as i64, created_at],
    )
    .map_err(map_sql)?;

    let usage = usage_from_conn(tx)?;
    let policy = policy_from_conn(tx)?;
    // Charge the same slices `quota_usage` counts: the attachments cap (which now
    // includes this blob) and the workspace total. A breach of either rejects and
    // rolls the staged insert back.
    let attachments = usage.category_bytes(QuotaCategory::Attachments);
    if let Some(err) = over_quota_breach(
        QuotaScope::Category { category: QuotaCategory::Attachments },
        attachments,
        policy.attachments_cap,
    ) {
        return Err(err);
    }
    if let Some(err) = over_quota_breach(
        QuotaScope::Workspace,
        usage.workspace_total_bytes,
        policy.workspace_limit,
    ) {
        return Err(err);
    }
    Ok(AttachmentPut {
        content_hash: hash.to_string(),
        stored_new: true,
        refcount: 1,
    })
}

impl Store {
    // --- Attachments: content-hash dedup (DL-22) -------------------------

    /// Store an attachment, deduplicated by content hash.
    ///
    /// The bytes are keyed by their `sha256:` [`content_hash`]. The FIRST put of a
    /// given content writes one blob row (`stored_new = true`); every subsequent put
    /// of IDENTICAL bytes stores NOTHING new — it only bumps the refcount
    /// (`stored_new = false`) — so identical attachments occupy ONE blob and are
    /// accounted ONCE by [`quota_usage`](Self::quota_usage) no matter how many records
    /// reference them (DL-22 "attachments deduplicated by content hash").
    ///
    /// Enforcement runs only for genuinely new bytes: a dedup hit never adds storage,
    /// so it is allowed even at quota (it cannot push usage up); a new blob is checked
    /// against the attachments cap + the workspace limit and REJECTED over quota,
    /// leaving every existing attachment byte-for-byte intact.
    ///
    /// The whole lookup → enforce → insert/refcount path runs in ONE `BEGIN IMMEDIATE`
    /// transaction (review 176 P2): it takes the writer lock BEFORE the dedup lookup,
    /// so two file-backed handles cannot both observe the same pre-write headroom and
    /// then both insert distinct blobs that together exceed the cap, and two identical
    /// first puts cannot race into a primary-key error — the second blocks, then sees
    /// the committed row and takes the refcount-bump branch. An `INSERT … ON CONFLICT`
    /// upsert makes the new-blob branch idempotent under that serialization.
    pub fn put_attachment(&mut self, bytes: &[u8]) -> Result<AttachmentPut> {
        let hash = content_hash(bytes);
        let write_bytes = bytes.len() as u64;
        let created_at = now_ms();
        self.transact_immediate(|tx| put_attachment_tx(tx, &hash, bytes, write_bytes, created_at))
    }

    /// Read an attachment's stored bytes by content hash, if present.
    pub fn get_attachment(&self, content_hash: &str) -> Result<Option<Vec<u8>>> {
        self.conn
            .query_row(
                "SELECT bytes FROM attachments WHERE content_hash = ?1",
                params![content_hash],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()
            .map_err(map_sql)
    }

    /// The current refcount of the attachment keyed by `content_hash`, or `None` if
    /// no such blob is stored.
    pub fn attachment_refcount(&self, content_hash: &str) -> Result<Option<u64>> {
        self.conn
            .query_row(
                "SELECT refcount FROM attachments WHERE content_hash = ?1",
                params![content_hash],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(map_sql)
            .map(|opt| opt.map(|n| n.max(0) as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IndexManager, Mutation};
    use serde_json::json;

    fn store() -> Store {
        Store::open_in_memory().expect("open in-memory store")
    }

    fn obj(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        v.as_object().expect("object").clone()
    }

    fn insert(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Insert {
            collection: collection.into(),
            id: Some(id.into()),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }

    #[test]
    fn empty_store_accounts_zero() {
        let s = store();
        let usage = s.quota_usage().unwrap();
        assert_eq!(usage.workspace_total_bytes, 0);
        assert!(usage.per_applet.is_empty());
        // Every category is present (stable report shape) and zero.
        assert_eq!(usage.per_category.len(), QuotaCategory::ALL.len());
        for c in &usage.per_category {
            assert_eq!(c.bytes, 0, "category {:?} should be zero", c.category);
        }
    }

    #[test]
    fn accounting_is_a_pure_function_of_state() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 1), &idx)
            .unwrap();
        let a = s.quota_usage().unwrap();
        let b = s.quota_usage().unwrap();
        assert_eq!(a, b, "two reads of an unchanged store must be byte-equal");
        assert!(a.workspace_total_bytes > 0);
        // Records show up under the owning applet (collection prefix = "tasks").
        assert_eq!(a.per_applet.len(), 1);
        assert_eq!(a.per_applet[0].applet, "tasks");
        assert!(a.per_applet[0].collections_bytes > 0);
        // A retained CRDT chunk is accounted under retained_chunks.
        assert!(a.category_bytes(QuotaCategory::RetainedChunks) > 0);
    }

    #[test]
    fn applet_grouping_splits_collections_by_prefix() {
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("notes/inbox", "n1", json!({"body": "hi"}), 1), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("notes/archive", "n2", json!({"body": "yo"}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "A"}), 3), &idx)
            .unwrap();
        let usage = s.quota_usage().unwrap();
        // notes/inbox + notes/archive collapse into one "notes" applet bucket.
        assert_eq!(usage.applet_bytes("tasks").min(1), 1);
        let notes = usage.applet_bytes("notes");
        assert!(notes > 0, "both notes collections fold into the notes applet");
    }

    #[test]
    fn default_policy_is_the_dl22_defaults() {
        let s = store();
        let p = s.quota_policy().unwrap();
        assert_eq!(p, QuotaPolicy::DEFAULT);
        assert_eq!(p.workspace_limit, GIB);
        assert_eq!(p.per_applet_limit, 100 * MIB);
        assert_eq!(p.approaching_threshold, 0.8);
    }

    #[test]
    fn persisted_policy_override_is_trusted_state() {
        let mut s = store();
        let mut p = QuotaPolicy::DEFAULT;
        p.workspace_limit = 4096;
        s.set_quota_policy(&p).unwrap();
        assert_eq!(s.quota_policy().unwrap().workspace_limit, 4096);
        // The override is in the local-only namespace (not synced/exported).
        assert!(crate::is_local_only_namespace(QUOTA_META_NS));
    }

    #[test]
    fn invalid_policy_override_is_rejected() {
        let mut s = store();
        let mut bad = QuotaPolicy::DEFAULT;
        bad.workspace_limit = 0;
        assert!(s.set_quota_policy(&bad).is_err());
        let mut bad2 = QuotaPolicy::DEFAULT;
        bad2.approaching_threshold = 1.5;
        assert!(s.set_quota_policy(&bad2).is_err());
    }

    #[test]
    fn under_limit_is_ok_approaching_warns_over_rejects() {
        let usage = QuotaUsage {
            workspace_total_bytes: 0,
            per_applet: vec![],
            per_category: QuotaCategory::ALL
                .iter()
                .map(|&category| CategoryUsage { category, bytes: 0 })
                .collect(),
        };
        let mut policy = QuotaPolicy::DEFAULT;
        policy.workspace_limit = 1000;
        policy.attachments_cap = 1000;
        policy.approaching_threshold = 0.8;

        // Well under the threshold ⇒ Ok.
        let ok = decide_quota(&usage, &policy, QuotaCategory::Attachments, None, 100);
        assert_eq!(ok, QuotaDecision::Ok);

        // At/above 80% but within the limit ⇒ a non-blocking warning.
        let warn = decide_quota(&usage, &policy, QuotaCategory::Attachments, None, 850);
        assert!(warn.is_approaching(), "got {warn:?}");
        assert!(!warn.is_over_quota());

        // Over the limit ⇒ a rejection carrying a suggestion, NOT a deletion.
        let over = decide_quota(&usage, &policy, QuotaCategory::Attachments, None, 1200);
        assert!(over.is_over_quota(), "got {over:?}");
        let err = over.over_quota_error().unwrap();
        assert_eq!(err.code(), "ResourceLimitExceeded");
        assert!(format!("{err}").contains("no data was deleted"));
    }

    #[test]
    fn attachment_dedup_stores_identical_bytes_once() {
        let mut s = store();
        let bytes = b"the same attachment bytes";
        let first = s.put_attachment(bytes).unwrap();
        assert!(first.stored_new, "first put stores the blob");
        assert_eq!(first.refcount, 1);
        let before = s.quota_usage().unwrap().category_bytes(QuotaCategory::Attachments);

        let second = s.put_attachment(bytes).unwrap();
        assert!(!second.stored_new, "identical bytes must NOT store a new blob");
        assert_eq!(second.refcount, 2);
        assert_eq!(second.content_hash, first.content_hash);

        let after = s.quota_usage().unwrap().category_bytes(QuotaCategory::Attachments);
        assert_eq!(before, after, "dedup adds no accounted bytes");
        // Different bytes ⇒ a distinct blob, accounted additionally.
        let other = s.put_attachment(b"different bytes").unwrap();
        assert!(other.stored_new);
        assert_ne!(other.content_hash, first.content_hash);
        assert!(s.quota_usage().unwrap().category_bytes(QuotaCategory::Attachments) > after);
    }

    #[test]
    fn over_quota_attachment_rejects_without_deleting_existing_data() {
        let mut s = store();
        // Seed one attachment, then tighten the cap below a second blob.
        let kept = s.put_attachment(b"keep me intact").unwrap();
        let kept_bytes = s.get_attachment(&kept.content_hash).unwrap().unwrap();
        let usage = s.quota_usage().unwrap();
        let mut policy = QuotaPolicy::DEFAULT;
        // Cap the attachments category just above what is already stored so any new
        // blob is over quota, but the workspace limit is roomy.
        policy.attachments_cap = usage.category_bytes(QuotaCategory::Attachments) + 4;
        s.set_quota_policy(&policy).unwrap();

        let err = s.put_attachment(b"this new blob does not fit").unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");

        // PROVE no silent deletion: the prior attachment is byte-for-byte intact and
        // the accounted usage is unchanged.
        assert_eq!(
            s.get_attachment(&kept.content_hash).unwrap().unwrap(),
            kept_bytes,
            "the over-quota rejection must not touch existing data"
        );
        assert_eq!(
            s.quota_usage().unwrap(),
            usage,
            "a rejected write must leave accounted usage unchanged"
        );
    }

    #[test]
    fn over_quota_records_write_rejects_on_the_real_path_without_deleting() {
        // LIVE-WIRING + REJECT-NOT-DELETE proof on the REAL DL-4 records write path.
        let mut s = store();
        let idx = IndexManager::new();
        // Seed an existing record, then tighten the workspace limit just above what is
        // already stored so the NEXT records write is over quota.
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "keep me"}), 1), &idx)
            .unwrap();
        let before_usage = s.quota_usage().unwrap();
        let before_record = s.get_record("tasks", "t1").unwrap().unwrap();
        let before_chunks: Vec<String> = s
            .get_chunks(&crate::collection_doc_id("tasks"))
            .unwrap()
            .into_iter()
            .map(|c| c.chunk_id)
            .collect();

        let mut policy = QuotaPolicy::DEFAULT;
        // A workspace limit exactly at the current total leaves zero headroom: any new
        // chunk pushes the projected total over the limit.
        policy.workspace_limit = before_usage.workspace_total_bytes;
        s.set_quota_policy(&policy).unwrap();

        // The real applet mutation surface must REJECT the over-quota write.
        let err = s
            .apply_mutation_crdt(&insert("tasks", "t2", json!({"title": "does not fit"}), 2), &idx)
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
        assert!(format!("{err}").contains("no data was deleted"));

        // PROVE no deletion / no partial write: the prior record + chunk substrate +
        // accounted usage are all byte-for-byte unchanged, and the rejected record never
        // landed (the whole transaction rolled back).
        assert_eq!(s.get_record("tasks", "t1").unwrap().unwrap(), before_record);
        assert!(s.get_record("tasks", "t2").unwrap().is_none());
        let after_chunks: Vec<String> = s
            .get_chunks(&crate::collection_doc_id("tasks"))
            .unwrap()
            .into_iter()
            .map(|c| c.chunk_id)
            .collect();
        assert_eq!(after_chunks, before_chunks, "no new chunk after a rejected write");
        assert_eq!(
            s.quota_usage().unwrap(),
            before_usage,
            "a rejected records write leaves accounted usage unchanged"
        );
    }

    #[test]
    fn dedup_put_is_allowed_even_at_quota() {
        let mut s = store();
        let first = s.put_attachment(b"already stored").unwrap();
        // Pin the attachments cap to exactly what is stored: a NEW blob would be over
        // quota, but re-putting the SAME bytes adds nothing and must be allowed.
        let usage = s.quota_usage().unwrap();
        let mut policy = QuotaPolicy::DEFAULT;
        policy.attachments_cap = usage.category_bytes(QuotaCategory::Attachments);
        s.set_quota_policy(&policy).unwrap();
        let again = s.put_attachment(b"already stored").unwrap();
        assert!(!again.stored_new);
        assert_eq!(again.refcount, 2);
        assert_eq!(again.content_hash, first.content_hash);
    }

    #[test]
    fn accepted_records_write_never_leaves_usage_over_a_limit() {
        // Review 176 P1: the gate charges the SAME slices `quota_usage` reports, so an
        // ACCEPTED write can never leave the workspace/per-applet/cache usage over its
        // limit. With a roomy policy we accept a chain of writes and assert AFTER EACH
        // that every budget the write grew is still within its limit — the gate would
        // reject a write that would breach, so a write that landed proves it did not.
        let mut s = store();
        let idx = IndexManager::new();
        let policy = QuotaPolicy::DEFAULT;
        s.set_quota_policy(&policy).unwrap();
        for n in 0..8 {
            s.apply_mutation_crdt(
                &insert("tasks", &format!("t{n}"), json!({ "title": format!("row {n} body") }), n + 1),
                &idx,
            )
            .unwrap();
            let usage = s.quota_usage().unwrap();
            assert!(
                usage.workspace_total_bytes <= policy.workspace_limit,
                "workspace usage must stay within the limit after an accepted write"
            );
            assert!(
                usage.applet_bytes("tasks") <= policy.per_applet_limit,
                "per-applet usage must stay within the limit after an accepted write"
            );
            assert!(
                usage.category_bytes(QuotaCategory::RetainedChunks)
                    <= policy.category_cap(QuotaCategory::RetainedChunks),
                "retained_chunks usage must stay within its cap"
            );
            assert!(
                usage.category_bytes(QuotaCategory::Cache) <= policy.category_cap(QuotaCategory::Cache),
                "cache (oplog) usage must stay within its cap"
            );
        }

        // And the gate is exact at the boundary: pin the workspace limit to the CURRENT
        // total (zero headroom) — the next write, which grows records + chunk + oplog,
        // must be rejected (it would otherwise land over the very limit it was checked
        // against, the review 176 P1 bug).
        let total = s.quota_usage().unwrap().workspace_total_bytes;
        let mut tight = QuotaPolicy::DEFAULT;
        tight.workspace_limit = total;
        s.set_quota_policy(&tight).unwrap();
        let err = s
            .apply_mutation_crdt(&insert("tasks", "overflow", json!({"title": "nope"}), 99), &idx)
            .unwrap_err();
        assert_eq!(err.code(), "ResourceLimitExceeded");
        assert!(
            s.quota_usage().unwrap().workspace_total_bytes <= tight.workspace_limit,
            "after the rejected write the workspace is NOT left over its limit"
        );
    }

    #[test]
    fn two_handles_cannot_oversubscribe_the_attachments_cap() {
        // Review 176 P2: the dedup lookup + quota check + insert run in one BEGIN
        // IMMEDIATE txn, so two file-backed handles serialize on the writer lock — they
        // cannot both observe the same headroom and then both insert distinct blobs that
        // together exceed the cap.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        let blob_a = b"distinct attachment AAAA";
        let blob_b = b"distinct attachment BBBB";

        let mut s1 = Store::open(&path).unwrap();
        // Cap the attachments category so EXACTLY ONE of the two distinct blobs fits.
        let mut policy = QuotaPolicy::DEFAULT;
        policy.attachments_cap = blob_a.len() as u64 + 4;
        s1.set_quota_policy(&policy).unwrap();

        let mut s2 = Store::open(&path).unwrap();
        let r1 = s1.put_attachment(blob_a);
        let r2 = s2.put_attachment(blob_b);

        // At least one is rejected over quota; the two together never exceed the cap.
        let ok_count = [&r1, &r2].iter().filter(|r| r.is_ok()).count();
        assert!(ok_count <= 1, "the two distinct blobs must not BOTH be accepted past the cap");
        let stored = s1.quota_usage().unwrap().category_bytes(QuotaCategory::Attachments);
        assert!(
            stored <= policy.attachments_cap,
            "attachments usage {stored} must not exceed the cap {}",
            policy.attachments_cap
        );
        // Whichever was rejected carries the typed error.
        for r in [r1, r2] {
            if let Err(e) = r {
                assert_eq!(e.code(), "ResourceLimitExceeded");
            }
        }
    }

    #[test]
    fn two_handles_identical_first_puts_dedup_without_a_primary_key_error() {
        // Review 176 P2: two identical first puts from two file-backed handles must NOT
        // race into a primary-key error — the writer lock serializes them so the second
        // sees the committed row and takes the refcount-bump branch (dedup), leaving ONE
        // stored blob with refcount 2.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ws.db");
        let bytes = b"the same first-put bytes from two handles";

        let mut s1 = Store::open(&path).unwrap();
        let mut s2 = Store::open(&path).unwrap();
        let r1 = s1.put_attachment(bytes).expect("first put");
        let r2 = s2.put_attachment(bytes).expect("second put must dedup, not PK-error");
        assert_eq!(r1.content_hash, r2.content_hash, "identical bytes share a content hash");
        // Exactly one stored blob; the two puts are stored_new in some order, the later
        // one a dedup hit with refcount 2.
        assert_eq!(s1.attachment_refcount(&r1.content_hash).unwrap(), Some(2));
        let blobs: i64 = s1
            .connection()
            .query_row("SELECT COUNT(*) FROM attachments WHERE content_hash = ?1", params![r1.content_hash], |row| row.get(0))
            .unwrap();
        assert_eq!(blobs, 1, "identical bytes occupy exactly ONE stored blob");
    }

    /// A minimal contract-valid [`RunRecord`] for the run-log admission tests.
    fn sample_run(run_id: &str) -> forge_domain::RunRecord {
        forge_domain::RunRecord {
            run_id: forge_domain::RunId::new(run_id),
            applet_id: forge_domain::AppletId::new("app"),
            code_hash: forge_domain::code_hash("body"),
            input: json!({}),
            random_seed: 1,
            time_start: 0,
            calls: vec![],
            logs: vec![],
            permissions: forge_domain::PermissionSnapshot::default(),
            resource_assets: std::collections::BTreeMap::new(),
            outcome: forge_domain::RunOutcome::Completed {
                result: forge_domain::AppResult { ok: true, value: json!(null) },
            },
        }
    }

    #[test]
    fn admit_run_rejects_when_run_logs_has_no_headroom() {
        // Review 178: the run_logs cap is a PRE-FLIGHT ADMISSION gate — a workspace whose
        // run-log budget is exhausted refuses to START new runs (reject-not-delete).
        let mut s = store();
        // Persist a first run so run_logs carries bytes, then pin the cap to exactly the
        // committed run_logs usage: zero headroom, so a NEW run cannot be admitted.
        s.transact(|tx| Store::save_run_tx(tx, &sample_run("run_1")))
            .expect("first run persists");
        let cap = s.quota_usage().unwrap().category_bytes(QuotaCategory::RunLogs);
        let mut policy = QuotaPolicy::DEFAULT;
        policy.run_logs_cap = cap;
        s.set_quota_policy(&policy).unwrap();

        // With run_logs usage AT the cap (>= run_logs_cap), admission is REFUSED with the
        // typed error + the compaction/cleanup/export suggestion.
        let err = s
            .admit_run_or_reject()
            .expect_err("a workspace at the run_logs cap refuses to admit a new run");
        assert_eq!(err.code(), "ResourceLimitExceeded");
        assert!(
            err.to_string().contains("no data was deleted"),
            "carries the compaction/cleanup/export suggestion: {err}"
        );
        assert!(
            err.to_string().contains("run_logs") || err.to_string().contains("category"),
            "names the run_logs budget: {err}"
        );
        // Reject-not-delete: nothing was deleted — the prior run is intact and usage is
        // unchanged (admission is a pure read; it commits nothing).
        assert!(s.load_run("run_1").unwrap().is_some(), "the prior run is untouched");
        assert_eq!(
            s.quota_usage().unwrap().category_bytes(QuotaCategory::RunLogs),
            cap,
            "admission is read-only: run_logs usage is unchanged by a rejection"
        );
    }

    #[test]
    fn admit_run_does_not_gate_on_a_full_workspace() {
        // Review 178 / spec/quotas.md §6: the workspace TOTAL is deliberately NOT part of
        // the admission gate. A run that FAILS because its `ctx.db` write was rejected at
        // the records boundary committed no durable write, and its *failed* run record
        // must be recorded even on a full workspace — so admission must ADMIT here, gated
        // only on run_logs headroom.
        let mut s = store();
        let idx = IndexManager::new();
        s.apply_mutation_crdt(&insert("tasks", "t1", json!({"title": "seed"}), 1), &idx)
            .unwrap();
        let total = s.quota_usage().unwrap().workspace_total_bytes;
        let mut policy = QuotaPolicy::DEFAULT;
        // Workspace total AT the limit (zero headroom), but run_logs is roomy.
        policy.workspace_limit = total;
        s.set_quota_policy(&policy).unwrap();

        s.admit_run_or_reject()
            .expect("a full workspace still admits a run (gated only on run_logs)");
    }

    #[test]
    fn admit_run_allows_a_run_with_headroom() {
        // The gate is a backstop, not a blanket block: with run-log budget to spare, a run
        // is admitted (and may then persist its mandatory record).
        let s = store();
        s.admit_run_or_reject()
            .expect("a fresh workspace under the default policy admits a run");
    }
}
