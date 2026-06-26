//! The workspace-level live-query (`db.watch`) session state and its persistence
//! (DL-16, `forge/spec/live-queries.md`).
//!
//! Phase 2 (forge-core) owns DELIVERY of the notifications the storage substrate
//! ([`forge_storage::WatchRegistry`]) COMPUTES. This module holds the facade's
//! watch state — the registry plus a `watch_id -> (applet_id, callback action_ref)`
//! map so a notification can be re-entered into the right applet's callback handler
//! — and (de)serializes it to the workspace file so a registered watch survives
//! reopening the workspace, exactly like the `db.read` grant table / schema registry.
//!
//! The split mirrors the rest of the spine: the storage [`WatchRegistry`] is the
//! pure, replay-safe SUBSTRATE (dirty set → notification bytes); this is the
//! workspace ORCHESTRATION (which applet callback to re-enter, the monotone version
//! that survives reopen, persistence).

use forge_domain::{CoreError, Result};
use forge_storage::{Store, WatchRegistry};
use serde::{Deserialize, Serialize};

/// The KV key (within the meta namespace) holding the persisted live-query watch
/// sessions: the registered watches (applet + callback + query) plus the workspace's
/// monotone notification `version`. Persisted so a registered watch — and the
/// version sequence — survives reopening the workspace file, mirroring the
/// `db_read_grants` / schema registry tables.
pub(crate) const WATCH_SESSIONS_KEY: &str = "watch_sessions";

/// One registered live query at the workspace level (DL-16). Beyond the storage
/// substrate's `(watch_id, collection, query)` this carries the OWNING applet and
/// the CALLBACK `action_ref` — the exported handler name a delivered notification
/// re-enters (reusing the UI event-dispatch machinery), so the facade knows whose
/// code to run and under which manifest/capabilities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WatchSubscription {
    /// Runtime-assigned, stable until `db.unwatch`.
    pub watch_id: String,
    /// The applet that owns this watch (whose callback is re-entered on a
    /// notification, under its manifest/capabilities).
    pub applet_id: String,
    /// The exported handler name the notification callback re-enters (the
    /// `ActionRef` the applet wired its `ctx.db.watch(..., callback)` to). The
    /// facade dispatches `<callback>(ctx, notification)` via the UI-event machinery.
    pub callback: String,
    /// The full canonical query value (the `db.watch` `query` field), reparsed into
    /// the storage [`WatchRegistry`] on load so the watch plan is the same validated
    /// AST as `ctx.db.from(...).all()`.
    pub query: serde_json::Value,
}

/// The persisted live-query session state for one workspace: every registered
/// [`WatchSubscription`] (in registration order) plus the next monotone notification
/// `version`. Serialized to [`WATCH_SESSIONS_KEY`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WatchSessions {
    /// Registered watches, in registration order (notification order is registration
    /// order, `forge/spec/live-queries.md`).
    pub subscriptions: Vec<WatchSubscription>,
    /// The next notification version to assign to a committed transaction. Persisted
    /// so versions stay strictly increasing across workspace reopen (DL-16
    /// monotonicity).
    pub next_version: u64,
}

impl WatchSessions {
    /// Build a fresh [`WatchRegistry`] from the persisted subscriptions, restoring
    /// the monotone `next_version`. Each subscription's query is reparsed through the
    /// canonical parser (rejecting a non-row aggregate/group watch as the original
    /// registration did), so the rebuilt registry is byte-identical to the live one.
    pub fn to_registry(&self) -> Result<WatchRegistry> {
        let mut reg = WatchRegistry::with_next_version(self.next_version);
        for sub in &self.subscriptions {
            reg.register_from_value(&sub.watch_id, &sub.query)?;
        }
        Ok(reg)
    }

    /// The `(applet_id, callback)` for `watch_id`, if registered. Used to address the
    /// callback handler a delivered notification re-enters.
    pub fn callback_for(&self, watch_id: &str) -> Option<(&str, &str)> {
        self.subscriptions
            .iter()
            .find(|s| s.watch_id == watch_id)
            .map(|s| (s.applet_id.as_str(), s.callback.as_str()))
    }

    /// The watch ids in registration order (the fixtures' `active_watches`).
    pub fn active_watch_ids(&self) -> Vec<String> {
        self.subscriptions.iter().map(|s| s.watch_id.clone()).collect()
    }

    /// The watch ids currently owned by a DIFFERENT applet than `applet_id` (review
    /// 135 #1). Injected into the [`StorageHostBridge`](crate::StorageHostBridge) before
    /// a live run so an applet's `ctx.db.watch` of a foreign-owned id is rejected AT
    /// HOST-CALL TIME (a recorded `PermissionDenied`), not silently accepted and dropped
    /// after the run by [`register_owned`](Self::register_owned). The two checks agree:
    /// the host-call denial is the runtime-visible mirror of `register_owned`'s refusal.
    pub fn foreign_owned_watch_ids(&self, applet_id: &str) -> Vec<String> {
        self.subscriptions
            .iter()
            .filter(|s| s.applet_id != applet_id)
            .map(|s| s.watch_id.clone())
            .collect()
    }

    /// Register/replace a watch (DL-16 `db.watch`). Idempotent on `watch_id`: a
    /// re-watch replaces the subscription IN PLACE (keeping its registration
    /// position) so a re-`watch` is not a duplicate, matching the storage registry.
    ///
    /// This is the TRUSTED in-process register (no owner check) — the workspace owner
    /// already holds a `WorkspaceCore`. An APPLET-originated register MUST use
    /// [`register_owned`](Self::register_owned) so one applet cannot REPLACE another
    /// applet's subscription by reusing its watch id (review 133 #2).
    pub fn register(&mut self, sub: WatchSubscription) {
        match self.subscriptions.iter_mut().find(|s| s.watch_id == sub.watch_id) {
            Some(existing) => *existing = sub,
            None => self.subscriptions.push(sub),
        }
    }

    /// Owner-scoped register (DL-16 `db.watch` from an applet, review 133 #2): register
    /// `sub` ONLY when its `watch_id` is unregistered OR already owned by `sub.applet_id`.
    /// Returns `true` when registered/replaced, `false` when the id already belongs to a
    /// DIFFERENT applet (the existing subscription is left intact — the caller surfaces a
    /// denial).
    ///
    /// Watch ids are applet-visible strings, so an applet that reuses another applet's
    /// id must NOT be able to silently replace that subscription's owner/callback/query
    /// (which would also break the original owner's owner-scoped `db.unwatch` and reroute
    /// its notifications). Binding registration to the owner upholds the CR-3
    /// capability-scoped subscription-ownership model. A same-owner re-watch still
    /// replaces in place (idempotent), matching [`register`](Self::register).
    pub fn register_owned(&mut self, sub: WatchSubscription) -> bool {
        match self.subscriptions.iter_mut().find(|s| s.watch_id == sub.watch_id) {
            // Foreign owner already holds this id — refuse to clobber it.
            Some(existing) if existing.applet_id != sub.applet_id => false,
            // Same owner — replace in place (idempotent re-watch).
            Some(existing) => {
                *existing = sub;
                true
            }
            // Unused id — register fresh.
            None => {
                self.subscriptions.push(sub);
                true
            }
        }
    }

    /// Cancel a watch (DL-16 `db.unwatch`). Idempotent: removing an unknown id is a
    /// no-op. After it returns the watch receives no further notifications.
    ///
    /// This is the TRUSTED in-process cancel (no owner check) — the workspace owner
    /// already holds a `WorkspaceCore`. An APPLET-originated cancel MUST use
    /// [`unregister_owned`](Self::unregister_owned) so one applet cannot stop
    /// another's subscription (review 132 #2).
    pub fn unregister(&mut self, watch_id: &str) {
        self.subscriptions.retain(|s| s.watch_id != watch_id);
    }

    /// Owner-scoped cancel (DL-16 `db.unwatch` from an applet, review 132 #2): remove
    /// `watch_id` ONLY when it is owned by `applet_id`. Returns `true` when a watch
    /// was removed, `false` when the id is unknown OR is owned by a DIFFERENT applet
    /// (idempotent + non-destructive of another applet's subscription). Watch ids are
    /// applet-visible strings, so an applet that guesses another's id must not be able
    /// to cancel it; binding the cancel to the owner upholds the CR-3 capability-scoped
    /// subscription-ownership model.
    pub fn unregister_owned(&mut self, applet_id: &str, watch_id: &str) -> bool {
        let owned = self
            .subscriptions
            .iter()
            .any(|s| s.watch_id == watch_id && s.applet_id == applet_id);
        if owned {
            self.subscriptions
                .retain(|s| !(s.watch_id == watch_id && s.applet_id == applet_id));
        }
        owned
    }
}

/// Load the persisted [`WatchSessions`] from the workspace file (mirrors the
/// `db.read` grant / schema registry loaders). Absent → an empty session set
/// (`next_version = 0`), the M0a default for a fresh workspace.
pub(crate) fn load_watch_sessions(store: &Store, meta_ns: &str) -> Result<WatchSessions> {
    match store.kv_get(meta_ns, WATCH_SESSIONS_KEY)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::StorageError(format!("deserialize watch sessions: {e}"))),
        None => Ok(WatchSessions::default()),
    }
}

/// Persist the [`WatchSessions`] to the workspace file, so a registered watch + the
/// version sequence survive reopening (DL-16; mirrors `grant_db_read`).
pub(crate) fn store_watch_sessions(
    store: &mut Store,
    meta_ns: &str,
    sessions: &WatchSessions,
) -> Result<()> {
    let bytes = serde_json::to_vec(sessions)
        .map_err(|e| CoreError::StorageError(format!("serialize watch sessions: {e}")))?;
    store.kv_set(meta_ns, WATCH_SESSIONS_KEY, &bytes, "application/json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sub(id: &str, query: serde_json::Value) -> WatchSubscription {
        WatchSubscription {
            watch_id: id.into(),
            applet_id: "app".into(),
            callback: "onTasks".into(),
            query,
        }
    }

    #[test]
    fn register_is_idempotent_in_place() {
        let mut s = WatchSessions::default();
        s.register(sub("w1", json!({ "from": "tasks" })));
        s.register(sub("w1", json!({ "from": "tasks", "where": ["done", "=", false] })));
        assert_eq!(s.active_watch_ids(), vec!["w1".to_string()]);
        // The query was replaced in place.
        assert_eq!(s.subscriptions[0].query["where"], json!(["done", "=", false]));
    }

    #[test]
    fn unregister_is_idempotent() {
        let mut s = WatchSessions::default();
        s.register(sub("w1", json!({ "from": "tasks" })));
        s.unregister("w1");
        s.unregister("w1"); // no panic
        assert!(s.active_watch_ids().is_empty());
    }

    #[test]
    fn to_registry_restores_version_and_rejects_non_row_watch() {
        let mut s = WatchSessions {
            next_version: 42,
            ..Default::default()
        };
        s.register(sub("w1", json!({ "from": "tasks", "orderBy": ["id", "asc"] })));
        let reg = s.to_registry().unwrap();
        assert_eq!(reg.next_version(), 42);
        assert_eq!(reg.active_watch_ids(), vec!["w1".to_string()]);

        // An aggregate watch in the persisted set is rejected on rebuild (review 129 #2).
        let mut bad = WatchSessions::default();
        bad.register(sub("agg", json!({ "from": "tasks", "aggregate": { "count": true } })));
        assert!(bad.to_registry().is_err());
    }

    #[test]
    fn unregister_owned_only_removes_the_owning_applets_watch() {
        // review 132 #2: an applet may cancel ONLY its own watch.
        let mut s = WatchSessions::default();
        s.register(WatchSubscription {
            watch_id: "w1".into(),
            applet_id: "app-a".into(),
            callback: "onWatch".into(),
            query: json!({ "from": "tasks" }),
        });
        // A different applet cannot cancel it.
        assert!(!s.unregister_owned("app-b", "w1"), "non-owner cancel is a no-op");
        assert_eq!(s.active_watch_ids(), vec!["w1".to_string()], "watch survives foreign unwatch");
        // An unknown id is a no-op too.
        assert!(!s.unregister_owned("app-a", "missing"));
        // The owner can cancel it (and the cancel is idempotent).
        assert!(s.unregister_owned("app-a", "w1"), "owner cancels its own watch");
        assert!(s.active_watch_ids().is_empty());
        assert!(!s.unregister_owned("app-a", "w1"), "idempotent: already gone");
    }

    #[test]
    fn register_owned_refuses_to_replace_another_applets_watch() {
        // review 133 #2: registration is owner-scoped — applet B cannot replace
        // applet A's subscription by reusing its watch id.
        let mut s = WatchSessions::default();
        assert!(s.register_owned(WatchSubscription {
            watch_id: "w1".into(),
            applet_id: "app-a".into(),
            callback: "onWatch".into(),
            query: json!({ "from": "tasks", "where": ["done", "=", false] }),
        }));
        // app-b reusing the same id is REFUSED, leaving app-a's subscription intact.
        assert!(!s.register_owned(WatchSubscription {
            watch_id: "w1".into(),
            applet_id: "app-b".into(),
            callback: "steal".into(),
            query: json!({ "from": "secrets" }),
        }));
        assert_eq!(s.callback_for("w1"), Some(("app-a", "onWatch")), "owner unchanged");
        assert_eq!(s.subscriptions[0].query["from"], json!("tasks"), "query unchanged");
        // The SAME owner re-watching replaces in place (idempotent), keeping ownership.
        assert!(s.register_owned(WatchSubscription {
            watch_id: "w1".into(),
            applet_id: "app-a".into(),
            callback: "onTasksV2".into(),
            query: json!({ "from": "tasks" }),
        }));
        assert_eq!(s.active_watch_ids(), vec!["w1".to_string()], "no duplicate");
        assert_eq!(s.callback_for("w1"), Some(("app-a", "onTasksV2")), "owner re-watch replaced");
    }

    #[test]
    fn callback_for_resolves_owner_and_handler() {
        let mut s = WatchSessions::default();
        s.register(WatchSubscription {
            watch_id: "w1".into(),
            applet_id: "tasks-app".into(),
            callback: "render".into(),
            query: json!({ "from": "tasks" }),
        });
        assert_eq!(s.callback_for("w1"), Some(("tasks-app", "render")));
        assert_eq!(s.callback_for("missing"), None);
    }
}
