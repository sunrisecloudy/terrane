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

/// Enforce the DL-22 quota for a records write of `write_bytes` bytes into
/// `collection`, INSIDE the caller's open transaction `tx`.
///
/// This is the LIVE write-boundary check: the usage and policy are read off the
/// SAME transaction that is about to persist the chunk/oplog/projection, so the
/// decision is against the real persisted state. An over-quota result returns the
/// typed `ResourceLimitExceeded` error, which the caller propagates — rolling the
/// whole transaction back, so the chunk, oplog row, and projection are NEVER
/// written and no existing data is deleted (reject-not-delete). `Ok`/approaching
/// allow the write to proceed; the approaching warning is non-blocking here (the
/// applet-facing surface surfaces it via `quota.status`).
pub(crate) fn enforce_records_write_tx(
    tx: &rusqlite::Transaction<'_>,
    collection: &str,
    write_bytes: u64,
) -> Result<()> {
    let usage = usage_from_conn(tx)?;
    let policy = policy_from_conn(tx)?;
    let applet = applet_of_collection(collection);
    let decision = decide_quota(
        &usage,
        &policy,
        QuotaCategory::RetainedChunks,
        Some(applet),
        write_bytes,
    );
    if let Some(err) = decision.over_quota_error() {
        return Err(err);
    }
    Ok(())
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
    /// Enforcement runs FIRST and only for genuinely new bytes: a dedup hit never
    /// adds storage, so it is allowed even at quota (it cannot push usage up); a new
    /// blob is checked against the attachments cap + the workspace limit and REJECTED
    /// over quota, leaving every existing attachment byte-for-byte intact.
    pub fn put_attachment(&mut self, bytes: &[u8]) -> Result<AttachmentPut> {
        let hash = content_hash(bytes);
        let existing = self.attachment_refcount(&hash)?;
        // Dedup hit: identical bytes already stored. Bump the refcount only; this
        // adds no storage, so it bypasses the quota check (it can never exceed it).
        if let Some(current) = existing {
            let next = current.saturating_add(1);
            self.conn
                .execute(
                    "UPDATE attachments SET refcount = ?2 WHERE content_hash = ?1",
                    params![hash, next as i64],
                )
                .map_err(map_sql)?;
            return Ok(AttachmentPut {
                content_hash: hash,
                stored_new: false,
                refcount: next,
            });
        }
        // New bytes: enforce the quota BEFORE writing. Over quota ⇒ reject, store
        // nothing, delete nothing.
        let write_bytes = bytes.len() as u64;
        let decision = self.check_quota(QuotaCategory::Attachments, None, write_bytes)?;
        if let Some(err) = decision.over_quota_error() {
            return Err(err);
        }
        self.conn
            .execute(
                "INSERT INTO attachments (content_hash, bytes, byte_len, refcount, created_at)
                 VALUES (?1, ?2, ?3, 1, ?4)",
                params![hash, bytes, write_bytes as i64, now_ms()],
            )
            .map_err(map_sql)?;
        Ok(AttachmentPut {
            content_hash: hash,
            stored_new: true,
            refcount: 1,
        })
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
}
