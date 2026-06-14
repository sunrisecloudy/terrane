//! The DL-16 live-query reactive substrate: a watch registry, a deterministic
//! dirty set produced per committed mutation transaction, and the canonical
//! `db.watch.notification` payload computed per affected watch.
//!
//! Normative spec: `forge/spec/live-queries.md` (DL-16) and the fixtures under
//! `fixtures/live-queries/` (T035) + `fixtures/live-queries-e2e/` (T047).
//!
//! ## What lives here (Phase 1, forge-storage)
//!
//! This is the *substrate* only — it computes notification bytes, it does not
//! deliver them. Delivery (re-entering the applet, recording in the run/session
//! record, non-reentrant event-loop queuing) is the runtime/core's job and is
//! wired on top of these types.
//!
//! - [`WatchRegistry`] — register/unregister a [`Watch`] (idempotent) and own the
//!   workspace-local monotonic notification `version`.
//! - [`DirtyKind`] / [`DirtySet`] — the deterministic set of record ids (per
//!   collection) a committed transaction changed, sorted + deduped, each tagged
//!   with the kind of write that touched it. A rolled-back transaction produces
//!   NO dirty set (the caller never calls [`WatchRegistry::commit`]).
//! - [`WatchNotification`] / [`NotificationReason`] — the canonical callback
//!   payload, byte-stable via [`WatchNotification::to_canonical_json`].
//!
//! ## Determinism (replay-safe)
//!
//! The dirty set is derived from the **mutation path** (which ids the write
//! touched, in op order), NOT from live SQLite update hooks — those are
//! non-deterministic and replay-hostile. `record_ids` are sorted by `entity_id`
//! and deduped so the notification bytes are identical on every run.
//!
//! ## Filter semantics (live-queries.md §Filter Semantics)
//!
//! A dirty record notifies a filtered watch iff it was in the query result
//! BEFORE the transaction OR is in the result AFTER it (enter / leave /
//! selected-field-changed). A record outside the result both before and after is
//! suppressed. This is why [`WatchRegistry::commit`] takes a pre-transaction
//! result snapshot ([`WatchRegistry::snapshot`]) captured before the write.

use std::collections::BTreeMap;

use forge_domain::{CoreError, Result};

use crate::query::{Mutation, Query, QueryResult};
use crate::store::Store;

/// How a committed write touched a record, for the notification `reason`. Both
/// `update` and `patch` are *modifications* of an existing record, so they
/// collapse to [`DirtyKind::Update`] — the fixtures report `reason: "update"`
/// for a `patch` (live-queries.md §Notification Shape). An `insert` over a prior
/// tombstone is still an `insert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyKind {
    Insert,
    Update,
    Delete,
}

impl DirtyKind {
    /// The single-kind notification `reason` when every dirty id in a collection
    /// shares this kind.
    fn single_reason(self) -> NotificationReason {
        match self {
            DirtyKind::Insert => NotificationReason::Insert,
            DirtyKind::Update => NotificationReason::Update,
            DirtyKind::Delete => NotificationReason::Delete,
        }
    }
}

/// The notification `reason` (live-queries.md §Notification Shape). A single op
/// kind reports `insert`/`update`/`delete`; a transaction whose dirty ids in the
/// watched collection mix op kinds reports `mixed`. `changed` is the
/// conservative catch-all the runtime may emit when it cannot prove a narrower
/// reason — the substrate never produces it from a known dirty kind, but it is
/// part of the canonical vocabulary so the payload type can carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationReason {
    Insert,
    Update,
    Delete,
    Changed,
    Mixed,
}

impl NotificationReason {
    /// The canonical wire token (`"insert"`, `"update"`, …).
    pub fn as_str(self) -> &'static str {
        match self {
            NotificationReason::Insert => "insert",
            NotificationReason::Update => "update",
            NotificationReason::Delete => "delete",
            NotificationReason::Changed => "changed",
            NotificationReason::Mixed => "mixed",
        }
    }

    /// Fold the dirty kinds of the ids that drive one notification into a single
    /// reason: a uniform kind keeps its own reason; differing kinds are `mixed`.
    fn from_kinds(kinds: &[DirtyKind]) -> NotificationReason {
        match kinds.split_first() {
            None => NotificationReason::Changed,
            Some((first, rest)) => {
                if rest.iter().all(|k| k == first) {
                    first.single_reason()
                } else {
                    NotificationReason::Mixed
                }
            }
        }
    }
}

/// A registered live query (DL-16 `db.watch`). The query AST is the canonical
/// validated plan (`forge/spec/query-dsl.md`); a bare-collection watch has
/// `query.filter == None` and matches every live record in the collection.
#[derive(Debug, Clone)]
pub struct Watch {
    /// Runtime-assigned, stable until `db.unwatch`.
    pub watch_id: String,
    /// The watched collection (the query's `from`).
    pub collection: String,
    /// The full validated query plan (filter / order / limit).
    pub query: Query,
}

/// The dirty set for one committed mutation transaction (live-queries.md §Dirty
/// Set): the record ids each collection changed, tagged with the write kind, plus
/// the monotonic notification `version` assigned to the transaction.
///
/// Ids within a collection are stored in a [`BTreeMap`] keyed by `entity_id`, so
/// reading them back is always sorted + deduped — the deterministic notification
/// bytes DL-16 requires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirtySet {
    pub version: u64,
    /// `collection -> { entity_id -> kind }`, sorted by both keys.
    pub collections: BTreeMap<String, BTreeMap<String, DirtyKind>>,
}

impl DirtySet {
    /// The sorted, deduped ids dirtied in `collection` (empty if untouched).
    pub fn ids(&self, collection: &str) -> Vec<String> {
        self.collections
            .get(collection)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// The dirty-set JSON shape the fixtures pin: `{version, collections:
    /// {collection: [id, …]}}` with ids sorted + deduped.
    pub fn to_json(&self) -> serde_json::Value {
        let collections: serde_json::Map<String, serde_json::Value> = self
            .collections
            .iter()
            .map(|(c, ids)| {
                let list: Vec<serde_json::Value> =
                    ids.keys().map(|id| serde_json::Value::String(id.clone())).collect();
                (c.clone(), serde_json::Value::Array(list))
            })
            .collect();
        serde_json::json!({
            "version": self.version,
            "collections": serde_json::Value::Object(collections),
        })
    }
}

/// The records a single mutation transaction dirtied, before a `version` is
/// assigned. The write path builds this from the ids it touched, in op order; the
/// [`WatchRegistry`] stamps the next version when the transaction commits.
///
/// A collapse of kinds per id keeps the LAST write's kind for that id within the
/// transaction (e.g. insert-then-patch of a new id stays `insert` is NOT what we
/// want — see [`DirtyChanges::touch`]). The deterministic ordering comes from the
/// inner [`BTreeMap`] when this is finalized into a [`DirtySet`].
#[derive(Debug, Clone, Default)]
pub struct DirtyChanges {
    collections: BTreeMap<String, BTreeMap<String, DirtyKind>>,
}

impl DirtyChanges {
    /// An empty change set (no records touched).
    pub fn new() -> Self {
        DirtyChanges::default()
    }

    /// Whether no record was touched (a transaction with no leaf writes).
    pub fn is_empty(&self) -> bool {
        self.collections.values().all(|m| m.is_empty())
    }

    /// Record that `id` in `collection` was touched by a write of `kind`.
    ///
    /// Within one transaction an id may be touched more than once (e.g. inserted
    /// then patched). The dirtying that decides the *reason* is the write that
    /// established the record's identity for this transaction: an `insert`
    /// anywhere in the transaction wins over a later `update`/`patch` of the same
    /// fresh id (the transaction, as a whole, *inserted* that record), and a
    /// `delete` after a modification wins (the record ends up deleted). Update
    /// and patch both map to [`DirtyKind::Update`].
    pub fn touch(&mut self, collection: &str, id: &str, kind: DirtyKind) {
        let by_id = self.collections.entry(collection.to_string()).or_default();
        match by_id.get(id).copied() {
            None => {
                by_id.insert(id.to_string(), kind);
            }
            Some(prev) => {
                by_id.insert(id.to_string(), combine_kind(prev, kind));
            }
        }
    }

    /// Build the dirty changes from the mutation(s) a committed transaction
    /// applied (DL-16 §Dirty Set) — derived from the **mutation path**, not live
    /// SQLite update hooks. Each leaf's target id is dirtied with its op kind
    /// (`patch`/`update` -> [`DirtyKind::Update`]); a nested `transact` is
    /// flattened so a group's leaves all land in one change set. Only call this
    /// for a transaction that COMMITTED — a rollback dirties nothing.
    pub fn from_mutations(mutations: &[Mutation]) -> Self {
        let mut changes = DirtyChanges::new();
        for m in mutations {
            changes.touch_mutation(m);
        }
        changes
    }

    /// Dirty every record one (possibly nested) mutation touches, in op order.
    fn touch_mutation(&mut self, m: &Mutation) {
        match m {
            Mutation::Insert { collection, id, .. } => {
                if let Some(id) = id {
                    self.touch(collection, id, DirtyKind::Insert);
                }
            }
            Mutation::Update { collection, id, .. } | Mutation::Patch { collection, id, .. } => {
                self.touch(collection, id, DirtyKind::Update);
            }
            Mutation::Delete { collection, id, .. } => {
                self.touch(collection, id, DirtyKind::Delete);
            }
            Mutation::Transact { items } => {
                for it in items {
                    self.touch_mutation(it);
                }
            }
        }
    }

    /// Finalize into a versioned [`DirtySet`], dropping collections with no ids.
    fn into_dirty_set(self, version: u64) -> DirtySet {
        let collections = self
            .collections
            .into_iter()
            .filter(|(_, ids)| !ids.is_empty())
            .collect();
        DirtySet {
            version,
            collections,
        }
    }
}

/// Combine the kind already recorded for an id this transaction with a later
/// write of the same id. A `delete` always wins (the record ends deleted); an
/// `insert` is sticky over a later modification (the transaction *created* the
/// record), but a `delete`-then-`insert` (reinsert) is an `insert`.
fn combine_kind(prev: DirtyKind, next: DirtyKind) -> DirtyKind {
    match (prev, next) {
        (_, DirtyKind::Delete) => DirtyKind::Delete,
        (DirtyKind::Insert, _) => DirtyKind::Insert,
        (DirtyKind::Delete, DirtyKind::Insert) => DirtyKind::Insert,
        (_, next) => next,
    }
}

/// The canonical `db.watch.notification` callback payload (live-queries.md
/// §Notification Shape). One is computed per affected watch per committed
/// transaction; multiple dirty ids fold into one (coalesced) notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchNotification {
    pub watch_id: String,
    pub version: u64,
    pub collection: String,
    /// The dirty ids in the watched collection that drove this notification,
    /// sorted + deduped.
    pub record_ids: Vec<String>,
    pub reason: NotificationReason,
    /// The current matching query result ids after the transaction, in query
    /// order.
    pub result_ids: Vec<String>,
    /// `true` when more than one dirty id folded into this single notification.
    pub coalesced: bool,
}

impl WatchNotification {
    /// The full canonical callback payload, including the `type` discriminator.
    /// Keys are emitted in the fixtures' field order so a structural compare with
    /// the vectors is exact.
    pub fn to_canonical_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "db.watch.notification",
            "watch_id": self.watch_id,
            "version": self.version,
            "collection": self.collection,
            "record_ids": self.record_ids,
            "reason": self.reason.as_str(),
            "result_ids": self.result_ids,
            "coalesced": self.coalesced,
        })
    }

    /// The recorded-call `args` payload (the canonical payload minus `type`,
    /// which becomes the run-record `method`; live-queries.md §Replay). The
    /// recorded subset is identical to the event the applet observed, so replay
    /// never recomputes an omitted field (review 103).
    pub fn to_recorded_args(&self) -> serde_json::Value {
        serde_json::json!({
            "watch_id": self.watch_id,
            "version": self.version,
            "collection": self.collection,
            "record_ids": self.record_ids,
            "reason": self.reason.as_str(),
            "result_ids": self.result_ids,
            "coalesced": self.coalesced,
        })
    }
}

/// A snapshot of each registered watch's result ids, captured BEFORE a mutation
/// transaction so the filter semantics can tell whether a dirty record *left* the
/// result (in before, not after) as well as whether it *entered* (not before, in
/// after). See [`WatchRegistry::snapshot`].
#[derive(Debug, Clone, Default)]
pub struct ResultSnapshot {
    by_watch: BTreeMap<String, Vec<String>>,
}

impl ResultSnapshot {
    fn ids_for(&self, watch_id: &str) -> &[String] {
        self.by_watch.get(watch_id).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

/// Owns the registered live queries and the workspace-local monotonic
/// notification version (DL-16). Watches are kept in registration order so two
/// watches affected by one transaction notify in a deterministic order
/// (live-queries.md `notification_order: "watch registration order"`).
#[derive(Debug)]
pub struct WatchRegistry {
    /// Registered watches, in registration order.
    watches: Vec<Watch>,
    /// The next notification version to assign to a committed transaction.
    next_version: u64,
}

impl Default for WatchRegistry {
    fn default() -> Self {
        WatchRegistry::new()
    }
}

impl WatchRegistry {
    /// A fresh registry whose first committed transaction will be version `0`.
    pub fn new() -> Self {
        WatchRegistry {
            watches: Vec::new(),
            next_version: 0,
        }
    }

    /// A registry whose next committed transaction takes `next_version` (used by
    /// the fixtures, which pin the starting `next_version`).
    pub fn with_next_version(next_version: u64) -> Self {
        WatchRegistry {
            watches: Vec::new(),
            next_version,
        }
    }

    /// The version the next committed transaction will be assigned (the fixtures'
    /// `next_version`).
    pub fn next_version(&self) -> u64 {
        self.next_version
    }

    /// The registered watches, in registration order.
    pub fn watches(&self) -> &[Watch] {
        &self.watches
    }

    /// The active watch ids, in registration order (the fixtures'
    /// `active_watches`).
    pub fn active_watch_ids(&self) -> Vec<String> {
        self.watches.iter().map(|w| w.watch_id.clone()).collect()
    }

    /// Register a watch over `query` under `watch_id` (DL-16 `db.watch`), rejecting
    /// a non-watchable query.
    ///
    /// Re-registering an existing `watch_id` REPLACES its query in place
    /// (keeping its registration position) so a re-`watch` is not a duplicate.
    /// A watch registered after a mutation has no history — it is simply added,
    /// and the just-committed transaction does not notify it (the caller does not
    /// re-run [`commit`](Self::commit) for a past transaction).
    ///
    /// An aggregate (`aggregate:{…}`) or `groupBy` query is REJECTED with a
    /// `QueryError`: its result is a scalar/group bundle, not a row id list, so the
    /// notification payload (which pins `result_ids` as ordered row ids) has no
    /// defined shape for it, and the dirty-set→result-membership filter the
    /// notification computation relies on does not apply (review 129 #2). A watch
    /// must observe a *row* result; an applet needing a reactive count watches the
    /// underlying rows and reduces in its callback.
    pub fn try_register(&mut self, watch_id: impl Into<String>, query: Query) -> Result<()> {
        reject_non_row_watch(&query)?;
        let watch_id = watch_id.into();
        let collection = query.from.clone();
        let watch = Watch {
            watch_id: watch_id.clone(),
            collection,
            query,
        };
        if let Some(existing) = self.watches.iter_mut().find(|w| w.watch_id == watch_id) {
            *existing = watch;
        } else {
            self.watches.push(watch);
        }
        Ok(())
    }

    /// Register a watch over a query already known to be a watchable row query
    /// (the internal happy path / tests). Panics on a non-watchable (aggregate /
    /// group) query — host-call registration must use the fallible
    /// [`try_register`](Self::try_register) / [`register_from_value`](Self::register_from_value).
    pub fn register(&mut self, watch_id: impl Into<String>, query: Query) {
        self.try_register(watch_id, query)
            .expect("register() requires a watchable row query; use try_register for untrusted input");
    }

    /// Register a watch from a fixture/host-call query value (the `db.watch`
    /// `query` field). Reuses the canonical query parser so the watch plan is the
    /// same validated AST as `ctx.db.from(...).all()`, and rejects a non-row
    /// (aggregate / group) query with a `QueryError` (review 129 #2).
    pub fn register_from_value(
        &mut self,
        watch_id: impl Into<String>,
        query: &serde_json::Value,
    ) -> Result<()> {
        let q = Query::from_fixture_value(query)?;
        self.try_register(watch_id, q)
    }

    /// Unregister a watch (DL-16 `db.unwatch`). Idempotent: unwatching an unknown
    /// id is a no-op. After it returns, the watch receives no further
    /// notifications (it is gone before the next [`commit`](Self::commit), and
    /// dropping it from a pending batch is the caller's concern — a notification
    /// is only emitted for a watch present at `commit`).
    pub fn unregister(&mut self, watch_id: &str) {
        self.watches.retain(|w| w.watch_id != watch_id);
    }

    /// The result ids of every registered watch right now (`store` is the
    /// pre-transaction state). Capture this BEFORE applying a mutation so
    /// [`commit`](Self::commit) can tell a record that *left* the result from one
    /// that was never in it.
    pub fn snapshot(&self, store: &Store) -> Result<ResultSnapshot> {
        let mut by_watch = BTreeMap::new();
        for w in &self.watches {
            by_watch.insert(w.watch_id.clone(), run_watch_ids(store, &w.query)?);
        }
        Ok(ResultSnapshot { by_watch })
    }

    /// The current result ids for a single watch query (convenience for the
    /// initial-result expectations the fixtures pin, e.g.
    /// `watch_initial_result_ids`).
    pub fn watch_result_ids(&self, store: &Store, watch_id: &str) -> Result<Option<Vec<String>>> {
        match self.watches.iter().find(|w| w.watch_id == watch_id) {
            Some(w) => Ok(Some(run_watch_ids(store, &w.query)?)),
            None => Ok(None),
        }
    }

    /// Commit a transaction's dirty changes: assign the next monotonic version,
    /// build the deterministic [`DirtySet`], and compute one notification per
    /// affected watch (DL-16).
    ///
    /// `before` is the [`snapshot`](Self::snapshot) taken before the write;
    /// `store` is the post-commit state. The version is consumed even when no
    /// watch is affected, so later transactions are strictly greater. A
    /// transaction that touched nothing (e.g. after the caller dropped a
    /// rolled-back write) MUST NOT reach here — a rollback produces no dirty set.
    ///
    /// Returns the dirty set (with its assigned version) and the notifications in
    /// watch registration order.
    pub fn commit(
        &mut self,
        changes: DirtyChanges,
        before: &ResultSnapshot,
        store: &Store,
    ) -> Result<(DirtySet, Vec<WatchNotification>)> {
        let version = self.next_version;
        self.next_version += 1;
        let dirty = changes.into_dirty_set(version);

        let mut notifications = Vec::new();
        for w in &self.watches {
            let Some(dirty_ids) = dirty.collections.get(&w.collection) else {
                continue; // no dirty id in this watch's collection
            };
            let after_ids = run_watch_ids(store, &w.query)?;
            let before_ids = before.ids_for(&w.watch_id);

            // Filter semantics: a dirty id notifies iff it was in the result
            // before the transaction OR is in it after (enter / leave /
            // selected-field-changed). An id outside both is suppressed.
            let mut relevant: Vec<(&String, DirtyKind)> = dirty_ids
                .iter()
                .filter(|(id, _)| {
                    after_ids.iter().any(|a| a == *id)
                        || before_ids.iter().any(|b| b == *id)
                })
                .map(|(id, kind)| (id, *kind))
                .collect();
            if relevant.is_empty() {
                continue;
            }
            // `dirty_ids` is a BTreeMap, so this is already sorted by id; keep the
            // explicit sort so the contract is local and obvious.
            relevant.sort_by(|a, b| a.0.cmp(b.0));

            let record_ids: Vec<String> = relevant.iter().map(|(id, _)| (*id).clone()).collect();
            let kinds: Vec<DirtyKind> = relevant.iter().map(|(_, k)| *k).collect();
            notifications.push(WatchNotification {
                watch_id: w.watch_id.clone(),
                version,
                collection: w.collection.clone(),
                coalesced: record_ids.len() > 1,
                reason: NotificationReason::from_kinds(&kinds),
                record_ids,
                result_ids: after_ids,
            });
        }
        Ok((dirty, notifications))
    }
}

/// Reject a query that does not produce a watchable ROW result (review 129 #2). A
/// `db.watch` must observe ordered row ids: an `aggregate` produces a scalar bundle
/// and a `groupBy` produces group buckets, neither of which fits the notification
/// payload's `result_ids` (ordered row ids) nor the dirty-id→result-membership
/// filter. Such a watch would stay active yet never notify, so it is rejected at
/// registration with a `QueryError` instead of being silently inert.
fn reject_non_row_watch(query: &Query) -> Result<()> {
    if query.aggregate.is_some() {
        return Err(CoreError::QueryError(
            "db.watch does not support aggregate queries; watch the underlying rows \
             and reduce in the callback"
                .into(),
        ));
    }
    if query.group_by.is_some() {
        return Err(CoreError::QueryError(
            "db.watch does not support groupBy queries; watch the underlying rows \
             and group in the callback"
                .into(),
        ));
    }
    Ok(())
}

/// Run a watch's query against `store` and return the ordered result ids (DL-15
/// row order). A registered watch is always a row query (aggregate/group queries
/// are rejected at registration by [`reject_non_row_watch`]); the non-row arms
/// here are unreachable for a registered watch and yield no ids defensively.
fn run_watch_ids(store: &Store, query: &Query) -> Result<Vec<String>> {
    match store.query(query)? {
        QueryResult::Rows(rows) => Ok(rows.into_iter().map(|r| r.id).collect()),
        _ => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::IndexManager;
    use crate::Mutation;
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
    fn patch(collection: &str, id: &str, fields: serde_json::Value, at: i64) -> Mutation {
        Mutation::Patch {
            collection: collection.into(),
            id: id.into(),
            fields: obj(fields),
            logical_at: Some(at),
        }
    }
    fn delete(collection: &str, id: &str, at: i64) -> Mutation {
        Mutation::Delete {
            collection: collection.into(),
            id: id.into(),
            logical_at: Some(at),
        }
    }

    /// Seed `records` directly via the CRDT write path (no watches yet) so a test
    /// can set up the pre-transaction `given` state.
    fn seed(s: &mut Store, idx: &IndexManager, ms: &[Mutation]) {
        for m in ms {
            s.apply_mutation_crdt(m, idx).unwrap();
        }
    }

    fn watch_query(v: serde_json::Value) -> Query {
        Query::from_fixture_value(&v).expect("parse watch query")
    }

    // --- registry register / unregister (idempotent) ----------------------

    #[test]
    fn register_then_unregister_is_idempotent() {
        let mut reg = WatchRegistry::new();
        reg.register("w1", watch_query(json!({"from": "tasks"})));
        assert_eq!(reg.active_watch_ids(), vec!["w1".to_string()]);
        // Re-register replaces in place, not duplicate.
        reg.register("w1", watch_query(json!({"from": "tasks", "where": ["done", "=", false]})));
        assert_eq!(reg.active_watch_ids(), vec!["w1".to_string()]);
        // Unregister, then unregister again — no panic, still empty.
        reg.unregister("w1");
        reg.unregister("w1");
        assert!(reg.active_watch_ids().is_empty());
    }

    // --- non-row watches (aggregate / groupBy) are rejected (review 129 #2) ---

    #[test]
    fn aggregate_watch_is_rejected_at_registration() {
        // An aggregate watch has no row `result_ids`, so registering it must fail
        // rather than register a watch that can never notify.
        let mut reg = WatchRegistry::new();
        let err = reg
            .register_from_value("watch:count", &json!({"from": "tasks", "aggregate": {"count": true}}))
            .expect_err("aggregate watch must be rejected");
        assert_eq!(err.code(), "QueryError");
        assert!(err.to_string().contains("aggregate"), "error names the unsupported feature: {err}");
        assert!(reg.active_watch_ids().is_empty(), "the rejected watch is not registered");
    }

    #[test]
    fn group_by_watch_is_rejected_at_registration() {
        let mut reg = WatchRegistry::new();
        let err = reg
            .register_from_value("watch:by-status", &json!({"from": "tasks", "groupBy": "status"}))
            .expect_err("groupBy watch must be rejected");
        assert_eq!(err.code(), "QueryError");
        assert!(err.to_string().contains("groupBy"), "error names the unsupported feature: {err}");
        assert!(reg.active_watch_ids().is_empty(), "the rejected watch is not registered");
    }

    #[test]
    fn try_register_rejects_aggregate_and_keeps_prior_watches() {
        // try_register surfaces the rejection without disturbing already-registered
        // watches (the registry is unchanged on the error path).
        let mut reg = WatchRegistry::new();
        reg.register("w-rows", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));
        let err = reg
            .try_register("w-agg", watch_query(json!({"from": "tasks", "aggregate": {"count": true}})))
            .expect_err("aggregate rejected");
        assert_eq!(err.code(), "QueryError");
        assert_eq!(reg.active_watch_ids(), vec!["w-rows".to_string()], "prior watch survives");
    }

    // --- dirty set: deterministic, sorted, deduped ------------------------

    #[test]
    fn dirty_changes_sort_and_dedupe_ids() {
        let mut c = DirtyChanges::new();
        c.touch("tasks", "tasks/2", DirtyKind::Insert);
        c.touch("tasks", "tasks/1", DirtyKind::Update);
        c.touch("tasks", "tasks/1", DirtyKind::Update); // dup id collapses
        let ds = c.into_dirty_set(7);
        assert_eq!(ds.version, 7);
        assert_eq!(ds.ids("tasks"), vec!["tasks/1", "tasks/2"]);
        assert_eq!(
            ds.to_json(),
            json!({"version": 7, "collections": {"tasks": ["tasks/1", "tasks/2"]}})
        );
    }

    #[test]
    fn dirty_kind_combination_prefers_delete_and_sticky_insert() {
        // insert then patch in one txn stays an insert (the txn created it).
        let mut c = DirtyChanges::new();
        c.touch("tasks", "tasks/1", DirtyKind::Insert);
        c.touch("tasks", "tasks/1", DirtyKind::Update);
        assert_eq!(*c.into_dirty_set(0).collections["tasks"].get("tasks/1").unwrap(), DirtyKind::Insert);
        // modify then delete ends deleted.
        let mut c = DirtyChanges::new();
        c.touch("tasks", "tasks/1", DirtyKind::Update);
        c.touch("tasks", "tasks/1", DirtyKind::Delete);
        assert_eq!(*c.into_dirty_set(0).collections["tasks"].get("tasks/1").unwrap(), DirtyKind::Delete);
    }

    // --- notification computation: insert / update / delete reasons -------

    #[test]
    fn insert_notifies_with_insert_reason_and_result_ids() {
        let mut s = store();
        let idx = IndexManager::new();
        let mut reg = WatchRegistry::with_next_version(1);
        reg.register("watch:tasks-all", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&insert("tasks", "tasks/1", json!({"title": "Ship", "done": false}), 1), &idx)
            .unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/1", DirtyKind::Insert);
        let (dirty, notifs) = reg.commit(changes, &before, &s).unwrap();

        assert_eq!(dirty.to_json(), json!({"version": 1, "collections": {"tasks": ["tasks/1"]}}));
        assert_eq!(notifs.len(), 1);
        assert_eq!(
            notifs[0].to_canonical_json(),
            json!({
                "type": "db.watch.notification",
                "watch_id": "watch:tasks-all",
                "version": 1,
                "collection": "tasks",
                "record_ids": ["tasks/1"],
                "reason": "insert",
                "result_ids": ["tasks/1"],
                "coalesced": false
            })
        );
    }

    #[test]
    fn patch_notifies_with_update_reason() {
        let mut s = store();
        let idx = IndexManager::new();
        seed(&mut s, &idx, &[insert("tasks", "tasks/1", json!({"title": "Ship", "done": false}), 1)]);
        let mut reg = WatchRegistry::with_next_version(7);
        reg.register(
            "watch:tasks-open",
            watch_query(json!({"from": "tasks", "where": ["done", "=", false], "orderBy": ["id", "asc"]})),
        );

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&patch("tasks", "tasks/1", json!({"title": "Ship v2"}), 2), &idx)
            .unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/1", DirtyKind::Update);
        let (_, notifs) = reg.commit(changes, &before, &s).unwrap();

        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].reason, NotificationReason::Update);
        assert_eq!(notifs[0].result_ids, vec!["tasks/1"]);
    }

    #[test]
    fn delete_notifies_with_post_delete_result_excluding_it() {
        let mut s = store();
        let idx = IndexManager::new();
        seed(
            &mut s,
            &idx,
            &[
                insert("tasks", "tasks/1", json!({"title": "Ship", "done": false}), 1),
                insert("tasks", "tasks/2", json!({"title": "Keep", "done": false}), 2),
            ],
        );
        let mut reg = WatchRegistry::with_next_version(2);
        reg.register(
            "watch:tasks-open",
            watch_query(json!({"from": "tasks", "where": ["done", "=", false], "orderBy": ["id", "asc"]})),
        );

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&delete("tasks", "tasks/1", 3), &idx).unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/1", DirtyKind::Delete);
        let (_, notifs) = reg.commit(changes, &before, &s).unwrap();

        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].reason, NotificationReason::Delete);
        assert_eq!(notifs[0].record_ids, vec!["tasks/1"]);
        assert_eq!(notifs[0].result_ids, vec!["tasks/2"], "post-delete result excludes the deleted id");
    }

    // --- filter semantics -------------------------------------------------

    #[test]
    fn dirty_outside_result_before_and_after_does_not_notify() {
        let mut s = store();
        let idx = IndexManager::new();
        seed(&mut s, &idx, &[insert("tasks", "tasks/1", json!({"title": "Done", "done": true, "prio": 1}), 1)]);
        let mut reg = WatchRegistry::with_next_version(4);
        reg.register(
            "watch:tasks-open",
            watch_query(json!({"from": "tasks", "where": ["done", "=", false], "orderBy": ["id", "asc"]})),
        );

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&patch("tasks", "tasks/1", json!({"prio": 2}), 2), &idx).unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/1", DirtyKind::Update);
        let (dirty, notifs) = reg.commit(changes, &before, &s).unwrap();

        // The record was dirtied (write-op semantics) but never matched the filter.
        assert_eq!(dirty.ids("tasks"), vec!["tasks/1"]);
        assert!(notifs.is_empty(), "outside-before-and-after suppresses the notification");
    }

    #[test]
    fn leaving_the_result_still_notifies() {
        // done:false -> true leaves the open filter; before-membership drives it.
        let mut s = store();
        let idx = IndexManager::new();
        seed(&mut s, &idx, &[insert("tasks", "tasks/1", json!({"title": "x", "done": false}), 1)]);
        let mut reg = WatchRegistry::with_next_version(0);
        reg.register(
            "watch:tasks-open",
            watch_query(json!({"from": "tasks", "where": ["done", "=", false], "orderBy": ["id", "asc"]})),
        );

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&patch("tasks", "tasks/1", json!({"done": true}), 2), &idx).unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/1", DirtyKind::Update);
        let (_, notifs) = reg.commit(changes, &before, &s).unwrap();

        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].record_ids, vec!["tasks/1"]);
        assert!(notifs[0].result_ids.is_empty(), "the record left the result");
    }

    #[test]
    fn non_watched_collection_does_not_notify() {
        let mut s = store();
        let idx = IndexManager::new();
        seed(&mut s, &idx, &[insert("tasks", "tasks/1", json!({"title": "Ship"}), 1)]);
        let mut reg = WatchRegistry::with_next_version(3);
        reg.register("watch:tasks-all", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&insert("notes", "notes/1", json!({"body": "private"}), 2), &idx).unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("notes", "notes/1", DirtyKind::Insert);
        let (dirty, notifs) = reg.commit(changes, &before, &s).unwrap();

        assert_eq!(dirty.ids("notes"), vec!["notes/1"]);
        assert!(notifs.is_empty(), "a mutation in another collection does not notify a tasks watch");
    }

    // --- coalescing + mixed reason ----------------------------------------

    #[test]
    fn transaction_coalesces_to_one_notification_with_mixed_reason() {
        let mut s = store();
        let idx = IndexManager::new();
        seed(&mut s, &idx, &[insert("tasks", "tasks/1", json!({"title": "Existing", "done": false}), 1)]);
        let mut reg = WatchRegistry::with_next_version(11);
        reg.register("watch:tasks-all", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));

        let before = reg.snapshot(&s).unwrap();
        let items = vec![
            insert("tasks", "tasks/2", json!({"title": "New", "done": false}), 2),
            patch("tasks", "tasks/1", json!({"done": true}), 3),
        ];
        s.transact_mutations_crdt(&items, &idx).unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/2", DirtyKind::Insert);
        changes.touch("tasks", "tasks/1", DirtyKind::Update);
        let (dirty, notifs) = reg.commit(changes, &before, &s).unwrap();

        assert_eq!(dirty.ids("tasks"), vec!["tasks/1", "tasks/2"]);
        assert_eq!(notifs.len(), 1, "one notification per watch per transaction");
        let n = &notifs[0];
        assert_eq!(n.record_ids, vec!["tasks/1", "tasks/2"]);
        assert_eq!(n.reason, NotificationReason::Mixed, "differing op kinds -> mixed");
        assert!(n.coalesced);
        assert_eq!(n.result_ids, vec!["tasks/1", "tasks/2"]);
    }

    #[test]
    fn two_watchers_notify_in_registration_order() {
        let mut s = store();
        let idx = IndexManager::new();
        let mut reg = WatchRegistry::with_next_version(5);
        reg.register("watch:tasks-all", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));
        reg.register(
            "watch:tasks-open",
            watch_query(json!({"from": "tasks", "where": ["done", "=", false], "orderBy": ["id", "asc"]})),
        );

        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&insert("tasks", "tasks/1", json!({"title": "Ship", "done": false}), 1), &idx)
            .unwrap();
        let mut changes = DirtyChanges::new();
        changes.touch("tasks", "tasks/1", DirtyKind::Insert);
        let (_, notifs) = reg.commit(changes, &before, &s).unwrap();

        let ids: Vec<&str> = notifs.iter().map(|n| n.watch_id.as_str()).collect();
        assert_eq!(ids, vec!["watch:tasks-all", "watch:tasks-open"]);
        // Both share the one transaction version.
        assert!(notifs.iter().all(|n| n.version == 5));
    }

    // --- monotonic versions, shared per transaction -----------------------

    #[test]
    fn versions_are_monotonic_and_shared_within_a_transaction() {
        let mut s = store();
        let idx = IndexManager::new();
        let mut reg = WatchRegistry::with_next_version(100);
        reg.register("watch:tasks-all", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));
        reg.register("watch:notes-all", watch_query(json!({"from": "notes", "orderBy": ["id", "asc"]})));

        // txn 1: insert tasks/1 -> version 100
        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&insert("tasks", "tasks/1", json!({"title": "A", "done": false}), 1), &idx)
            .unwrap();
        let mut c1 = DirtyChanges::new();
        c1.touch("tasks", "tasks/1", DirtyKind::Insert);
        let (_, n1) = reg.commit(c1, &before, &s).unwrap();
        assert_eq!(n1.len(), 1);
        assert_eq!(n1[0].version, 100);

        // txn 2: insert tasks/2 + notes/1 in one transaction -> both version 101
        let before = reg.snapshot(&s).unwrap();
        // (two collections: drive each via its own CRDT write, but ONE commit)
        s.apply_mutation_crdt(&insert("tasks", "tasks/2", json!({"title": "B", "done": false}), 2), &idx)
            .unwrap();
        s.apply_mutation_crdt(&insert("notes", "notes/1", json!({"body": "N"}), 3), &idx)
            .unwrap();
        let mut c2 = DirtyChanges::new();
        c2.touch("tasks", "tasks/2", DirtyKind::Insert);
        c2.touch("notes", "notes/1", DirtyKind::Insert);
        let (_, n2) = reg.commit(c2, &before, &s).unwrap();
        assert_eq!(n2.len(), 2);
        assert!(n2.iter().all(|n| n.version == 101), "same transaction shares one version");

        // txn 3: patch tasks/1 -> version 102, strictly greater
        let before = reg.snapshot(&s).unwrap();
        s.apply_mutation_crdt(&patch("tasks", "tasks/1", json!({"done": true}), 4), &idx).unwrap();
        let mut c3 = DirtyChanges::new();
        c3.touch("tasks", "tasks/1", DirtyKind::Update);
        let (_, n3) = reg.commit(c3, &before, &s).unwrap();
        assert_eq!(n3[0].version, 102);
        assert_eq!(reg.next_version(), 103);
    }

    // --- rolled-back transaction produces no dirty set / no notify --------

    #[test]
    fn rolled_back_transaction_emits_no_dirty_set_and_no_notification() {
        // A group whose second leaf patches a missing record fails; the whole
        // group rolls back. The caller must NOT call commit() (no dirty set, no
        // version consumed) — the version stays put and the records are unchanged.
        let mut s = store();
        let idx = IndexManager::new();
        seed(&mut s, &idx, &[insert("tasks", "tasks/1", json!({"title": "Before", "done": false}), 1)]);
        let mut reg = WatchRegistry::with_next_version(1);
        reg.register("watch:tasks-all", watch_query(json!({"from": "tasks", "orderBy": ["id", "asc"]})));

        let before_record = s.get_record("tasks", "tasks/1").unwrap().unwrap();
        // Snapshot is captured before the write (as the live path would), but a
        // rolled-back transaction never reaches commit() so it is unused here.
        let _before = reg.snapshot(&s).unwrap();
        let items = vec![
            patch("tasks", "tasks/1", json!({"title": "After"}), 2),
            patch("tasks", "ghost", json!({"x": 1}), 3),
        ];
        let err = s.transact_mutations_crdt(&items, &idx).unwrap_err();
        assert_eq!(err.code(), "QueryError");

        // The write rolled back: record unchanged, and since we never commit() to
        // the registry, the version is untouched and no notification is produced.
        assert_eq!(s.get_record("tasks", "tasks/1").unwrap().unwrap(), before_record);
        assert_eq!(reg.next_version(), 1, "a rolled-back transaction consumes no version");
    }

    // --- canonical recorded args (replay) ---------------------------------

    #[test]
    fn recorded_args_drop_only_the_type_field() {
        let n = WatchNotification {
            watch_id: "watch:tasks-all".into(),
            version: 30,
            collection: "tasks".into(),
            record_ids: vec!["tasks/1".into()],
            reason: NotificationReason::Insert,
            result_ids: vec!["tasks/1".into()],
            coalesced: false,
        };
        assert_eq!(
            n.to_recorded_args(),
            json!({
                "watch_id": "watch:tasks-all",
                "version": 30,
                "collection": "tasks",
                "record_ids": ["tasks/1"],
                "reason": "insert",
                "result_ids": ["tasks/1"],
                "coalesced": false
            })
        );
        // The full payload re-adds exactly `type`.
        let full = n.to_canonical_json();
        assert_eq!(full["type"], json!("db.watch.notification"));
    }
}
