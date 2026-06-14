//! `db.watch` / `db.unwatch` (DL-16, `forge/spec/live-queries.md`) and the
//! committed-mutation NOTIFICATION DELIVERY loop — the Phase-2 (forge-core) wiring
//! that turns the storage [`WatchRegistry`](forge_storage::WatchRegistry) substrate
//! (which COMPUTES notification bytes) into a reactive loop that RECORDS each
//! delivered notification in the run/session record and re-enters the watching
//! applet's callback.
//!
//! ## What lives here
//!
//!   - [`cmd_db_watch`](WorkspaceCore::cmd_db_watch) / [`cmd_db_unwatch`](WorkspaceCore::cmd_db_unwatch)
//!     — the command-registry handlers. `db.watch` gates on the SAME collection-scoped
//!     `db.read` grant as `query.execute` (spec §Registration), registers the watch
//!     (owning applet + callback handler + query) in the persisted
//!     [`WatchSessions`](crate::workspace::watch::WatchSessions), and returns the
//!     watch_id + current result ids. `db.unwatch` is idempotent and stops later
//!     notifications.
//!   - [`commit_and_notify`](WorkspaceCore::commit_and_notify) — drive ONE committed
//!     mutation transaction (snapshot → atomic write → registry commit → notifications),
//!     RECORD each notification, DISPATCH it into the watching applet's callback
//!     (non-reentrant: a callback mutation is QUEUED as the next turn, never a
//!     recursive flush), persist the bumped version, and return the delivered batch.
//!
//! The version monotonicity, dirty-set coalescing, filter semantics, and the
//! canonical notification bytes are all the storage substrate's (DL-16); this module
//! is purely the workspace ORCHESTRATION + persistence + record/replay seam.

use forge_domain::{CoreError, RecordedCall, Result, RunRecord};
use forge_runtime::{record_notification, RunRecorder};
use forge_storage::{DirtyChanges, DirtySet, Mutation, ResultSnapshot, WatchNotification};

/// One committed write paired with the watch result membership snapshot taken
/// IMMEDIATELY BEFORE it landed (DL-16). The pre-write snapshot is what lets a turn
/// apply the correct enter/leave/changed filter for an update/patch/delete that
/// removes a record from a watched result — the post-write store can no longer reveal
/// the pre-write membership. Produced by the [`StorageHostBridge`](crate::StorageHostBridge)
/// for every live `ctx.db` write and drained by the spine / callback turn loop.
pub type CommittedWrite = (Mutation, ResultSnapshot);

use super::super::auth::require_db_read;
use super::super::persistence::META_NS;
use super::super::watch::{store_watch_sessions, WatchSubscription};
use super::super::WorkspaceCore;
use super::require_applet_id;

/// A mutation a watch callback COMMITTED, tagged with the owning applet id and the
/// watch result membership snapshot taken IMMEDIATELY BEFORE that callback write, to
/// be delivered as the NEXT event-loop turn (non-reentrant delivery, T047 (a)). The
/// pre-write snapshot keeps the leave/changed filter correct for a callback that
/// updates/deletes a watched record.
type QueuedMutation = (String, Mutation, ResultSnapshot);

/// One delivered notification batch for a single committed mutation transaction:
/// the canonical notifications (in watch registration order), the trace calls
/// recorded for them (`db.watch.notification` envelopes the session replay re-serves
/// byte-identically), the assigned dirty set, and any mutations a watch CALLBACK
/// requested — QUEUED for the NEXT event-loop turn (non-reentrant, T047 (a)).
#[derive(Debug, Clone, Default)]
pub struct DeliveredBatch {
    /// The notifications delivered this transaction, in watch registration order.
    pub notifications: Vec<WatchNotification>,
    /// The recorded `db.watch.notification` calls (the replayable notification
    /// stream, `forge/spec/live-queries.md` §Replay). One per delivered notification.
    pub recorded_calls: Vec<RecordedCall>,
    /// The dirty set assigned to this committed transaction (`None` for a rolled-back
    /// or schema-only transaction, which produces no dirty set / no notification).
    pub dirty: Option<DirtySet>,
    /// The run ids of the watch CALLBACKS this batch re-entered (one per notification
    /// delivered to an installed applet whose callback handler exists). Each is a
    /// saved [`RunRecord`](forge_domain::RunRecord) whose trace carries the
    /// `db.watch.notification` envelope — the replayable proof the callback ran.
    pub callback_runs: Vec<forge_domain::RunId>,
    /// Mutations a watch callback requested during delivery, to be applied as the
    /// NEXT committed transaction (a later version) — never recursively inside this
    /// batch (non-reentrant delivery, T047 (a)).
    pub queued_mutations: Vec<QueuedMutation>,
}

impl WorkspaceCore {
    /// `db.watch` — register a live query (DL-16, `forge/spec/live-queries.md`).
    ///
    /// Payload: `{ applet_id, watch_id, query, callback? }`.
    ///
    /// `forge/spec/live-queries.md` §Registration: "Registration requires the same
    /// `db.read` grant as `all()` for the watched collection." So the gate is the
    /// SAME collection-scoped `db.read` ([`require_db_read`]) `query.execute` uses,
    /// resolved from the TRUSTED grant table (never the request payload, review 048).
    /// The watched collection is the query's `from`; an aggregate/group query is
    /// rejected (it has no row `result_ids`, review 129 #2). On success the watch is
    /// registered in the persisted [`WatchSessions`](crate::workspace::watch::WatchSessions)
    /// (owning applet + callback handler + query) and the response carries the
    /// watch_id + the watch's CURRENT result ids (so the applet can render its first
    /// view without an immediate follow-up `all()`).
    pub(in crate::workspace) fn cmd_db_watch(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let watch_id = cmd
            .payload
            .get("watch_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::ValidationError("db.watch requires a `watch_id`".into()))?
            .to_string();
        let query = cmd
            .payload
            .get("query")
            .cloned()
            .ok_or_else(|| CoreError::ValidationError("db.watch requires a `query`".into()))?;
        // The callback is the exported handler a delivered notification re-enters
        // (the `ActionRef` the applet wired its `ctx.db.watch(..., callback)` to).
        // Default to a conventional `onWatch` handler name when unspecified.
        let callback = cmd
            .payload
            .get("callback")
            .and_then(|v| v.as_str())
            .unwrap_or("onWatch")
            .to_string();

        // The watched collection is the query's `from`; it must be a string.
        let collection = query
            .get("from")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                CoreError::ValidationError(
                    "db.watch `query` requires a string `from` collection".into(),
                )
            })?
            .to_string();

        // §Registration: the SAME collection-scoped `db.read` gate as `query.execute`,
        // resolved from the TRUSTED grant table (review 048). Denied BEFORE any state
        // changes.
        let trusted_scope = self.db_read_grants.get(cmd.actor.actor.as_str()).cloned();
        require_db_read(cmd, &collection, trusted_scope.as_deref())?;

        // Validate the query is a watchable ROW plan (rejects aggregate/group, review
        // 129 #2) by registering it into a registry rebuilt from the current sessions:
        // `register_from_value` runs the canonical parse + non-row rejection. We do this
        // on a throwaway registry first so a bad query is rejected before we mutate
        // persisted state.
        let mut probe = self.watch_sessions.to_registry()?;
        probe.register_from_value(&watch_id, &query)?;

        // Register OWNER-SCOPED (review 133 #2): a re-watch by the SAME applet replaces
        // its subscription in place (idempotent), but a `watch_id` already owned by a
        // DIFFERENT applet is REFUSED — one applet must not be able to replace another's
        // owner/callback/query (which would also break the original owner's owner-scoped
        // `db.unwatch` and reroute its notifications). Denied BEFORE any persist.
        let registered = self.watch_sessions.register_owned(WatchSubscription {
            watch_id: watch_id.clone(),
            applet_id: applet_id.as_str().to_string(),
            callback: callback.clone(),
            query: query.clone(),
        });
        if !registered {
            return Err(CoreError::PermissionDenied(format!(
                "watch_id `{watch_id}` is already registered by another applet"
            )));
        }
        self.persist_watch_sessions()?;

        // The watch's current result ids (so the applet renders its initial view).
        let registry = self.watch_sessions.to_registry()?;
        let result_ids = registry
            .watch_result_ids(&self.store, &watch_id)?
            .unwrap_or_default();

        self.events.emit(
            Some(applet_id.clone()),
            "db.watch.registered",
            serde_json::json!({
                "applet_id": applet_id,
                "watch_id": watch_id,
                "collection": collection,
                "callback": callback,
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "watch_id": watch_id,
            "collection": collection,
            "active": true,
            "result_ids": result_ids,
        }))
    }

    /// `db.unwatch` — cancel a live query (DL-16). Idempotent: unwatching an unknown
    /// id is a no-op. After it commits the watch receives no further notifications.
    ///
    /// Payload: `{ watch_id, applet_id }`. Gated on the same read-capable roles as
    /// `db.watch` (a caller that could never read could never have watched); the
    /// cancellation itself reads no collection data.
    ///
    /// OWNER-SCOPED (review 132 #2): the cancel binds to the calling `applet_id` — it
    /// removes the watch ONLY when that applet OWNS it. Watch ids are applet-visible
    /// strings, so one applet must not be able to stop another applet's subscription
    /// by naming its id; `was_active` reflects whether THIS applet's own watch was
    /// cancelled, so cancelling an id owned by a different applet is a no-op that
    /// reports `was_active: false` (and leaves that subscription live).
    pub(in crate::workspace) fn cmd_db_unwatch(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let watch_id = cmd
            .payload
            .get("watch_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CoreError::ValidationError("db.unwatch requires a `watch_id`".into()))?
            .to_string();
        // Owner-scoped: removes the watch only when `applet_id` owns it; a foreign /
        // unknown id is a non-destructive no-op (`was_active = false`).
        let was_active = self
            .watch_sessions
            .unregister_owned(applet_id.as_str(), &watch_id);
        self.persist_watch_sessions()?;
        self.events.emit(
            Some(applet_id.clone()),
            "db.watch.unregistered",
            serde_json::json!({
                "applet_id": applet_id,
                "watch_id": watch_id,
                "was_active": was_active,
            }),
        );
        Ok(serde_json::json!({
            "applet_id": applet_id,
            "watch_id": watch_id,
            "active": false,
            "was_active": was_active,
            "active_watches": self.watch_sessions.active_watch_ids(),
        }))
    }

    /// The active watch ids in registration order (read-only access for tests /
    /// the conformance harness / a shell reporting the watch set).
    pub fn active_watch_ids(&self) -> Vec<String> {
        self.watch_sessions.active_watch_ids()
    }

    /// The next monotone notification `version` a committed transaction will be
    /// assigned (DL-16). Read-only access for tests / the conformance harness, which
    /// pin the version sequence.
    pub fn next_watch_version(&self) -> u64 {
        self.watch_sessions.next_version
    }

    /// Seed the workspace's monotone notification `version` (the trusted in-process
    /// seam, used by the conformance harness which pins the starting `next_version`).
    /// In-process only (mirrors `grant_db_read`): an in-process caller that holds a
    /// `WorkspaceCore` already owns the workspace. Persisted so the seed survives
    /// reopen.
    pub fn seed_watch_version(&mut self, next_version: u64) -> Result<()> {
        self.watch_sessions.next_version = next_version;
        self.persist_watch_sessions()
    }

    /// The current result ids of a registered watch (DL-16 row order), `None` if no
    /// such watch. Lets a caller read a watch's initial view (the fixtures'
    /// `watch_initial_result_ids`).
    pub fn watch_result_ids(&self, watch_id: &str) -> Result<Option<Vec<String>>> {
        let registry = self.watch_sessions.to_registry()?;
        registry.watch_result_ids(&self.store, watch_id)
    }

    /// Register a watch directly (the trusted in-process seam, used by the
    /// conformance harness and tests). Bypasses the command-level RBAC/`db.read`
    /// gate — an in-process caller that holds a `WorkspaceCore` already owns the
    /// workspace (mirrors [`grant_db_read`](WorkspaceCore::grant_db_read)). Validates
    /// the query is a watchable ROW plan and persists the subscription.
    pub fn register_watch(
        &mut self,
        applet_id: impl Into<String>,
        watch_id: impl Into<String>,
        callback: impl Into<String>,
        query: serde_json::Value,
    ) -> Result<()> {
        let watch_id = watch_id.into();
        // Reject a non-row / malformed watch before mutating persisted state.
        let mut probe = self.watch_sessions.to_registry()?;
        probe.register_from_value(&watch_id, &query)?;
        self.watch_sessions.register(WatchSubscription {
            watch_id,
            applet_id: applet_id.into(),
            callback: callback.into(),
            query,
        });
        self.persist_watch_sessions()
    }

    /// Cancel a watch directly (the trusted in-process seam). Idempotent.
    pub fn unregister_watch(&mut self, watch_id: &str) -> Result<()> {
        self.watch_sessions.unregister(watch_id);
        self.persist_watch_sessions()
    }

    /// Seed one record directly into the projection via the CRDT write path (the
    /// trusted in-process seam, used by the conformance harness to set up a
    /// fixture's `given` state). `deleted` inserts then tombstones the record.
    /// Bypasses notification delivery — it establishes pre-transaction state BEFORE
    /// any watch observes (mirrors a workspace seeded before the live session).
    pub fn seed_record(
        &mut self,
        collection: &str,
        id: &str,
        fields: serde_json::Map<String, serde_json::Value>,
        logical_at: i64,
        deleted: bool,
    ) -> Result<()> {
        let insert = Mutation::Insert {
            collection: collection.to_string(),
            id: Some(id.to_string()),
            fields,
            logical_at: Some(logical_at),
        };
        self.store.apply_mutation_crdt(&insert, &self.indexes)?;
        if deleted {
            let delete = Mutation::Delete {
                collection: collection.to_string(),
                id: id.to_string(),
                logical_at: Some(logical_at + 1),
            };
            self.store.apply_mutation_crdt(&delete, &self.indexes)?;
        }
        Ok(())
    }

    /// Drive ONE committed mutation transaction and DELIVER the resulting watch
    /// notifications (DL-16, `forge/spec/live-queries.md`).
    ///
    /// This is the Phase-2 reactive loop's single turn:
    ///   1. snapshot every registered watch's result ids BEFORE the write (the filter
    ///      semantics need the pre-transaction membership);
    ///   2. apply `mutation` through the CRDT write path — a `Transact` group commits
    ///      atomically via [`transact_mutations_crdt`](forge_storage::Store::transact_mutations_crdt)
    ///      (review 129 #1), a single op via
    ///      [`apply_mutation_crdt`](forge_storage::Store::apply_mutation_crdt). A
    ///      ROLLED-BACK write returns its error and NEVER reaches the registry commit,
    ///      so it produces no dirty set / no notification / consumes no version
    ///      (live-queries.md §Dirty Set);
    ///   3. commit the dirty changes to a registry rebuilt from the persisted
    ///      sessions — the substrate assigns the next monotone `version`, builds the
    ///      deterministic dirty set, and computes one canonical notification per
    ///      affected watch (coalesced, filter-evaluated, sorted+deduped);
    ///   4. RECORD each notification (a `db.watch.notification` envelope) and, when the
    ///      watch's owning applet is installed and exports the callback handler,
    ///      DISPATCH it by re-entering that callback via [`record_notification`] (the
    ///      same engine/host/record path as a UI dispatch) — NON-REENTRANT: a mutation
    ///      the callback makes is QUEUED for the next turn (a later version), never a
    ///      recursive flush inside this batch (T047 (a));
    ///   5. persist the bumped `next_version` so the version sequence survives reopen.
    ///
    /// Returns the [`DeliveredBatch`]. The caller (the turn loop) applies any
    /// `queued_mutations` as the NEXT `commit_and_notify`, which gets a later version.
    pub fn commit_and_notify(
        &mut self,
        mutation: &Mutation,
    ) -> Result<DeliveredBatch> {
        // (1) Snapshot before the write — the filter semantics need pre-transaction
        // membership to tell a record that LEFT the result from one never in it.
        let registry = self.watch_sessions.to_registry()?;
        let before = registry.snapshot(&self.store)?;

        // (2) Apply the mutation through the CRDT write path. A rollback propagates
        // the error here and NEVER reaches the registry commit below, so no dirty set
        // / notification is produced and no version is consumed.
        match mutation {
            Mutation::Transact { items } => {
                self.store.transact_mutations_crdt(items, &self.indexes)?;
            }
            single => {
                self.store.apply_mutation_crdt(single, &self.indexes)?;
            }
        }

        // (3) The FIRST turn: commit this mutation's dirty changes → version + dirty
        // set + notifications, RECORD + DISPATCH each. The `DeliveredBatch.dirty` is
        // this first turn's dirty set (the one the caller pins). The shared driver
        // then drains any callback-queued mutation as the NEXT turn (a later version).
        let changes = DirtyChanges::from_mutations(std::slice::from_ref(mutation));
        let mut batch = DeliveredBatch::default();
        let dirty = self.run_notify_turn(changes, &before, &mut batch)?;
        batch.dirty = Some(dirty);
        self.drain_callback_turns(&mut batch)?;

        Ok(batch)
    }

    /// Deliver live-query notifications for mutations a LIVE applet run/dispatch
    /// already COMMITTED through `ctx.db` (DL-16). The bridge applied each write
    /// directly through the CRDT path (one atomic write per call) and captured it in
    /// [`StorageHostBridge::applied_mutations`](crate::StorageHostBridge); the run/
    /// dispatch spine drains those and hands them here so registered watches FIRE on
    /// the real applet mutation — recorded for replay, callback re-entered.
    ///
    /// This is the NOTIFICATION-ONLY counterpart to [`commit_and_notify`]: the writes
    /// already landed (we do NOT re-apply them), so each mutation is driven as one
    /// committed-transaction notification turn (its own monotone version, in apply
    /// order), and any callback-queued mutation is then drained as a later turn (the
    /// non-reentrant next-turn loop, T047 (a)). Returns the aggregate
    /// [`DeliveredBatch`] across every turn (its `dirty` is the FIRST turn's dirty
    /// set, like `commit_and_notify`).
    ///
    /// `_applet_id` names the applet that issued the writes (for symmetry with the
    /// callback path); ownership of the affected watches is resolved per-notification
    /// from the registry, so a write by one applet correctly notifies a watch owned
    /// by another (the workspace is the single observer).
    pub(in crate::workspace) fn notify_committed_mutations(
        &mut self,
        _applet_id: &str,
        writes: &[CommittedWrite],
    ) -> Result<DeliveredBatch> {
        let mut batch = DeliveredBatch::default();
        if writes.is_empty() {
            return Ok(batch);
        }
        // Each captured write was applied as its OWN atomic write (one `ctx.db` call =
        // one CRDT transaction), so it is one committed-transaction turn with its own
        // version, in apply order. The writes already landed; each carries the watch
        // membership snapshot taken IMMEDIATELY BEFORE it, so the enter/leave/changed
        // filter is correct even for an update/patch/delete that removed a record from
        // the watched result (the post-write store can no longer reveal the pre-write
        // membership). The turn computes the dirty set + notifications WITHOUT
        // re-applying the write.
        for (i, (mutation, before)) in writes.iter().enumerate() {
            let changes = DirtyChanges::from_mutations(std::slice::from_ref(mutation));
            let dirty = self.run_notify_turn(changes, before, &mut batch)?;
            // The aggregate `dirty` pins the FIRST turn's dirty set (the caller's
            // first observed transaction), matching `commit_and_notify`.
            if i == 0 {
                batch.dirty = Some(dirty);
            }
            // Drain any callback-queued mutation from THIS turn before the next
            // applet write's turn, so versions stay in committed order.
            self.drain_callback_turns(&mut batch)?;
        }
        Ok(batch)
    }

    /// The TURN LOOP (non-reentrant, T047 (a)): a callback that mutated through
    /// `ctx.db` committed an already-applied write captured in
    /// `batch.queued_mutations`. Drive each as the NEXT event-loop turn — a SEPARATE
    /// registry commit that gets a strictly LATER version, never a recursive flush
    /// inside the current batch. The captured mutations already landed in the store,
    /// so the turn computes their dirty set + notifications WITHOUT re-applying them.
    /// Each follow-up turn may itself queue more mutations, so loop until quiescent.
    fn drain_callback_turns(&mut self, batch: &mut DeliveredBatch) -> Result<()> {
        while !batch.queued_mutations.is_empty() {
            let queued = std::mem::take(&mut batch.queued_mutations);
            // Each queued callback write carries the watch membership snapshot taken
            // IMMEDIATELY BEFORE it landed (captured by the callback's bridge), so the
            // leave/changed filter is correct even for a callback that updated/deleted
            // a watched record — not just the insert case. Drive each as its OWN turn
            // in commit order (its own later version).
            for (_applet, mutation, before) in queued {
                let changes = DirtyChanges::from_mutations(std::slice::from_ref(&mutation));
                self.run_notify_turn(changes, &before, batch)?;
            }
        }
        Ok(())
    }

    /// Run ONE notification turn: commit `changes` to the registry against the
    /// `before` snapshot (assigning the next monotone version), RECORD each resulting
    /// notification as a `db.watch.notification` envelope, DISPATCH it into the
    /// watching applet's callback (capturing any callback mutation into
    /// `batch.queued_mutations` for the NEXT turn), and persist the bumped version.
    /// Returns the turn's [`DirtySet`].
    ///
    /// This is the shared body of every turn the loop drives — the FIRST turn (the
    /// caller's mutation) and every CALLBACK-queued follow-up turn — so the version
    /// monotonicity, recording, and dispatch are identical across turns.
    fn run_notify_turn(
        &mut self,
        changes: DirtyChanges,
        before: &ResultSnapshot,
        batch: &mut DeliveredBatch,
    ) -> Result<DirtySet> {
        // Commit the dirty changes to a registry rebuilt from the persisted sessions
        // (its `next_version` is the workspace's monotone version) → version + dirty
        // set + one notification per affected watch.
        let mut registry = self.watch_sessions.to_registry()?;
        let (dirty, notifications) = registry.commit(changes, before, &self.store)?;

        // The registry consumed the next version; persist the bumped version so the
        // sequence is monotone across reopen.
        self.watch_sessions.next_version = registry.next_version();
        self.persist_watch_sessions()?;

        for notification in notifications {
            // RECORD the notification into the run/session record (a
            // `db.watch.notification` envelope) so REPLAY serves the recorded
            // sequence byte-identically (live-queries.md §Replay).
            batch.recorded_calls.push(RecordedCall {
                seq: batch.recorded_calls.len() as u64,
                method: "db.watch.notification".to_string(),
                args: notification.to_recorded_args(),
                response: serde_json::json!({ "delivered": true }),
            });
            self.events.emit(
                None,
                "db.watch.notification",
                notification.to_canonical_json(),
            );

            // DISPATCH into the watching applet's callback when it is installed and
            // exports the callback handler. The callback runs over the same engine /
            // host / record path as a UI dispatch (`record_notification`); a mutation
            // it makes is QUEUED for the next turn (non-reentrant, T047 (a)). When no
            // such applet/callback exists (a substrate-only / data-driven watch), the
            // notification is recorded but not re-entered.
            if let Some((run_id, queued)) = self.dispatch_notification_callback(&notification)? {
                batch.callback_runs.push(run_id);
                batch.queued_mutations.extend(queued);
            }

            batch.notifications.push(notification);
        }

        Ok(dirty)
    }

    /// Re-enter the watching applet's callback for one delivered notification, when
    /// the owning applet is installed and exports the callback handler. Returns the
    /// mutations the callback requested through `ctx.db`, to be QUEUED for the next
    /// turn (non-reentrant, T047 (a)). `None` when there is no installed applet /
    /// callback to re-enter (a substrate-only watch).
    ///
    /// The callback runs over the SAME engine/host/record path as a UI dispatch via
    /// [`record_notification`]; its run record is persisted (replay source) and its
    /// captured `ctx.db.watch`/`unwatch` intents fold into the workspace registry. A
    /// failed callback is surfaced as a typed error (the notification was still
    /// recorded, so the audit trail is intact), not a panic.
    fn dispatch_notification_callback(
        &mut self,
        notification: &WatchNotification,
    ) -> Result<Option<(forge_domain::RunId, Vec<QueuedMutation>)>> {
        // Resolve the owning applet + callback handler for this watch.
        let Some((applet_id, callback)) = self
            .watch_sessions
            .callback_for(&notification.watch_id)
            .map(|(a, c)| (a.to_string(), c.to_string()))
        else {
            return Ok(None);
        };
        // Only re-enter an INSTALLED applet (an uninstalled/never-installed watch
        // owner has no code to run; the notification is still recorded). A suspended
        // applet is also not re-entered (no live session), mirroring the UI dispatch
        // gate.
        let Some(installed) = self.load_applet(&applet_id)? else {
            return Ok(None);
        };
        if self.applet_lifecycle(&applet_id)? == crate::workspace::AppletLifecycle::Suspended {
            return Ok(None);
        }

        let program = forge_runtime::Program::new(
            forge_domain::AppletId::new(applet_id.clone()),
            installed.js_code.clone(),
        );
        // The callback's deterministic seams are derived from (code_hash, payload) —
        // exactly like a UI dispatch — so a re-delivery reproduces the same seeded
        // time/random and the notification replays byte-identically.
        let payload = notification.to_canonical_json();
        let (random_seed, time_start) =
            crate::determinism::derive_seeds(&installed.code_hash, &payload);
        let invocation = self.next_run_counter()?;

        let http_client = (self.http_client_factory)();
        let secret_store = (self.secret_store_factory)();
        let file_system = (self.file_system_factory)();
        let actor = forge_domain::ActorContext::owner("watch-callback");

        // The live watch registry (rebuilt from the persisted sessions) so a callback
        // that itself mutates through `ctx.db` captures the pre-write watch membership
        // for the NEXT turn's leave/changed filter (DL-16).
        let watch_registry = self.watch_sessions.to_registry()?;
        let mut bridge = crate::StorageHostBridge::with_http_client(
            &mut self.store,
            &applet_id,
            http_client,
        )
        .with_secret_store(secret_store)
        .with_file_system(file_system)
        .with_watch_registry(watch_registry);
        let run = record_notification(
            &program,
            &installed.manifest,
            &actor,
            &callback,
            &payload,
            random_seed,
            time_start,
            &mut bridge,
        )?;
        // Drain the callback's captured UI renders, watch intents (a callback that
        // itself watched/unwatched) AND the record-mutating writes it COMMITTED through
        // `ctx.db` (already applied to the store) BEFORE dropping the bridge releases
        // the &mut Store. The captured writes drive the NEXT event-loop turn's
        // notification (non-reentrant, T047 (a)); the facade does not re-apply them.
        let ui_renders = std::mem::take(&mut bridge.ui_renders);
        let watch_intents = std::mem::take(&mut bridge.watch_intents);
        let applied_mutations = std::mem::take(&mut bridge.applied_mutations);
        drop(bridge);

        // Persist the callback's run (replay source) under a unique per-execution id.
        let mut run: RunRecord = run;
        run.run_id = crate::determinism::unique_run_id(&run.code_hash, invocation);
        self.store_run_program(run.run_id.as_str(), &installed)?;
        self.store_program(&installed)?;
        self.store.save_run(&run)?;

        // A notification callback re-renders the watching applet's view (the reactive
        // loop's whole point). Emit a `ui.patch` per captured render — the SAME link a
        // `runtime.run`/`ui.dispatch_event` emits — so a subscribed renderer advances
        // the live tree when a notification arrives, and persist the callback's LAST
        // render as the applet's next diff base (UI-4) so a subsequent UI event diffs
        // against the notification-rendered view.
        for (i, render) in ui_renders.iter().enumerate() {
            self.events.emit(
                Some(forge_domain::AppletId::new(applet_id.clone())),
                "ui.patch",
                serde_json::json!({
                    "applet_id": applet_id,
                    "watch_id": notification.watch_id,
                    "run_id": run.run_id,
                    "render_index": i,
                    "tree": render.tree,
                    "patches": render.patches,
                }),
            );
        }
        if let Some(last) = ui_renders.last() {
            self.store_ui_tree(&applet_id, &last.tree)?;
        }

        // Fold the callback's own watch/unwatch intents into the workspace registry
        // (a callback may register/cancel a watch), then persist.
        self.apply_watch_intents(&applet_id, &watch_intents)?;

        // A callback that mutates does so through `ctx.db`, which the live bridge
        // applied as a committed write DURING this dispatch. Those writes already
        // landed in the store; to keep delivery NON-REENTRANT we DO NOT compute their
        // notifications inside this batch. Instead we hand the applied mutations back
        // to the turn loop ([`commit_and_notify`]), which drives them as the NEXT
        // event-loop turn — a separate registry commit with a strictly LATER version
        // (T047 (a)). The dirty set is computed from these captured mutations without
        // re-applying them (they already committed).
        let queued: Vec<QueuedMutation> = applied_mutations
            .into_iter()
            .map(|(m, before)| (applet_id.clone(), m, before))
            .collect();
        Ok(Some((run.run_id, queued)))
    }

    /// Apply a run/callback's captured live-query subscription intents
    /// ([`WatchIntent`](crate::bridge::WatchIntent)) to the workspace registry and
    /// persist (DL-16). Drained from the [`StorageHostBridge`](crate::StorageHostBridge)
    /// after a run/dispatch/callback so a `ctx.db.watch`/`unwatch` an applet issued
    /// becomes a registered/cancelled workspace watch.
    pub(in crate::workspace) fn apply_watch_intents(
        &mut self,
        applet_id: &str,
        intents: &[crate::bridge::WatchIntent],
    ) -> Result<()> {
        if intents.is_empty() {
            return Ok(());
        }
        for intent in intents {
            match intent {
                crate::bridge::WatchIntent::Watch { watch_id, query } => {
                    // Validate the watch is a row plan before registering (the runtime
                    // host already validated, but the registry rebuild re-checks).
                    let mut probe = self.watch_sessions.to_registry()?;
                    probe.register_from_value(watch_id, query)?;
                    // The callback handler an applet wires via `ctx.db.watch` is a
                    // conventional `onWatch` name (the runtime surface carries only
                    // (watch_id, query)); a richer callback ref can be threaded later.
                    //
                    // OWNER-SCOPED register (review 133 #2): a re-watch by THIS applet
                    // replaces its own subscription in place, but a `watch_id` already
                    // owned by a DIFFERENT applet is REFUSED — one applet must not be able
                    // to replace another's subscription by reusing its id. A refused
                    // collision is a non-destructive no-op that leaves the original owner's
                    // watch intact; we emit a denial so the attempt is auditable but do not
                    // abort the rest of this run's intent folding.
                    let registered = self.watch_sessions.register_owned(WatchSubscription {
                        watch_id: watch_id.clone(),
                        applet_id: applet_id.to_string(),
                        callback: "onWatch".to_string(),
                        query: query.clone(),
                    });
                    if !registered {
                        self.events.emit(
                            Some(forge_domain::AppletId::new(applet_id.to_string())),
                            "db.watch.denied",
                            serde_json::json!({
                                "applet_id": applet_id,
                                "watch_id": watch_id,
                                "reason": "watch_id owned by another applet",
                            }),
                        );
                    }
                }
                crate::bridge::WatchIntent::Unwatch { watch_id } => {
                    // OWNER-SCOPED cancel (review 132 #2): an applet may cancel ONLY
                    // its own watch. A cancel for an id owned by a different applet (or
                    // unknown) is a silent no-op — one applet must not be able to stop
                    // another applet's subscription by guessing its watch id.
                    self.watch_sessions.unregister_owned(applet_id, watch_id);
                }
            }
        }
        self.persist_watch_sessions()
    }

    /// Persist the workspace's live-query session state to the workspace file
    /// (DL-16; mirrors `grant_db_read`). Called after every `db.watch`/`db.unwatch`,
    /// every delivered batch (the bumped version), and every folded callback intent.
    fn persist_watch_sessions(&mut self) -> Result<()> {
        store_watch_sessions(&mut self.store, META_NS, &self.watch_sessions)
    }
}

/// Replay a recorded notification stream and assert it reproduces byte-identically
/// (DL-16, `forge/spec/live-queries.md` §Replay). Given the `recorded` calls a
/// session produced (each a `db.watch.notification` envelope), re-serve them through
/// a fresh [`RunRecorder`] in REPLAY mode: each `notification(args)` call must line
/// up at the cursor (same payload, same order) and is served the recorded
/// `{delivered: true}` — a diverging payload, a missing notification, or an extra one
/// is a determinism `RuntimeError`. Replay touches NO live SQLite update hooks and
/// recomputes NO result ids; it replays the recorded bytes.
///
/// Returns the produced calls (which must equal `recorded`) so a caller can assert
/// byte-identity. This is the workspace-level analogue of `runtime.replay` for the
/// notification stream.
pub fn replay_notification_stream(
    recorded: &[RecordedCall],
) -> Result<Vec<RecordedCall>> {
    let mut recorder = RunRecorder::replaying(0, 0, recorded.to_vec());
    for call in recorded {
        // Re-issue the SAME notification payload; replay asserts it matches the
        // recording at the cursor and serves the recorded response.
        recorder.notification(call.args.clone())?;
    }
    // Every recorded call must have been consumed (no notification left unserved).
    recorder.assert_fully_consumed()?;
    Ok(recorder.into_calls())
}
