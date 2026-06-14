//! `ctx.db.*` host calls for [`HostContext`]: the capability-checked, recorded
//! collection effects (`db.insert`/`get`/`list`/`query`).
//!
//! Each call funnels through the shared
//! [`HostContext::check_or_record_denial`](super::HostContext::check_or_record_denial)
//! policy/denial chokepoint, then performs its single effect inside
//! `recorder.host_call(method, args, || bridge_call)` so record/replay stays
//! byte-identical. `db.query` additionally **pins** the query's `from` to the
//! capability-checked collection before any bridge sees it.

use super::HostContext;
use forge_domain::Result;
use forge_policy::{Access, HostCall};

impl HostContext<'_> {
    // --- Live-query notification delivery (recorded, replay-bound) --------

    /// Record (or replay) a delivered **watch notification** (DL-16, `forge/spec/
    /// live-queries.md` §Replay): the canonical `db.watch.notification` payload the
    /// facade computed and re-entered into the applet's watch callback. The
    /// callback's own `ctx.*` effects are captured as ordinary host calls; this
    /// records the *notification envelope* so a session replays the same
    /// notification sequence byte-identically.
    ///
    /// On replay the recorder serves the recorded `{delivered: true}` and asserts
    /// the payload matches the recording (a diverging notification payload/order is
    /// a determinism `RuntimeError`). Like the UI dispatch envelope this is NOT a
    /// policy-gated host call — the delivery itself touches no user data; the
    /// callback's effects are gated as usual — but it IS counted toward the trace
    /// order so the `replay_fingerprint` covers every delivered notification.
    pub fn deliver_notification(
        &mut self,
        notification: serde_json::Value,
    ) -> Result<serde_json::Value> {
        self.recorder.notification(notification)
    }

    // --- Db (capability-checked, recorded effects) ----------------------

    pub fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String> {
        let args = serde_json::json!([collection, record]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Write, collection: collection.to_string() },
            "db.insert",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let r = record.clone();
        let resp = self.recorder.host_call("db.insert", args, || {
            Ok(serde_json::json!(bridge.db_insert(&c, r)?))
        })?;
        Ok(resp.as_str().unwrap_or("").to_string())
    }

    /// `ctx.db.update(collection, id, record)` — REPLACE a record's display fields
    /// (DL-17). Gated on `db.write` for `collection` and recorded exactly like
    /// `db.insert`: in record mode the call + the returned id are appended as a
    /// `RecordedCall`; on replay the recorded id is served (the live store is never
    /// re-written), so replay stays byte-identical.
    pub fn db_update(
        &mut self,
        collection: &str,
        id: &str,
        record: serde_json::Value,
    ) -> Result<String> {
        let args = serde_json::json!([collection, id, record]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Write, collection: collection.to_string() },
            "db.update",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let i = id.to_string();
        let r = record.clone();
        let resp = self.recorder.host_call("db.update", args, || {
            Ok(serde_json::json!(bridge.db_update(&c, &i, r)?))
        })?;
        Ok(resp.as_str().unwrap_or("").to_string())
    }

    /// `ctx.db.patch(collection, id, partial)` — MERGE the supplied fields into a
    /// record, preserving omitted fields (DL-9/DL-17). Gated on `db.write` and
    /// recorded like `db.update`.
    pub fn db_patch(
        &mut self,
        collection: &str,
        id: &str,
        partial: serde_json::Value,
    ) -> Result<String> {
        let args = serde_json::json!([collection, id, partial]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Write, collection: collection.to_string() },
            "db.patch",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let i = id.to_string();
        let p = partial.clone();
        let resp = self.recorder.host_call("db.patch", args, || {
            Ok(serde_json::json!(bridge.db_patch(&c, &i, p)?))
        })?;
        Ok(resp.as_str().unwrap_or("").to_string())
    }

    /// `ctx.db.delete(collection, id)` — tombstone a record (DL-4/DL-17). Gated on
    /// `db.write` and recorded; the call returns `null`.
    pub fn db_delete(&mut self, collection: &str, id: &str) -> Result<()> {
        let args = serde_json::json!([collection, id]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Write, collection: collection.to_string() },
            "db.delete",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let i = id.to_string();
        self.recorder.host_call("db.delete", args, || {
            bridge.db_delete(&c, &i).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }

    /// `ctx.db.transact(ops)` — apply a group of mutations atomically (DL-17). `ops`
    /// is the JSON array of `{op, collection, id?, fields?}` leaves. Gated on
    /// `db.write` for EACH leaf's collection (so a group cannot write a collection the
    /// applet lacks `db.write` on) and recorded once; the call returns the applied
    /// leaf count.
    pub fn db_transact(&mut self, ops: serde_json::Value) -> Result<u64> {
        // Gate every leaf's collection BEFORE the bridge applies anything, so an
        // atomic group is denied as a whole when ANY leaf touches an ungranted
        // collection (no partial write). The denial is recorded once under the first
        // offending leaf's collection, mirroring the single-op write gate.
        for collection in transact_collections(&ops)? {
            let args = serde_json::json!([collection]);
            self.check_or_record_denial(
                &HostCall::Db { op: Access::Write, collection: collection.clone() },
                "db.transact",
                &args,
            )?;
        }
        let args = serde_json::json!([ops]);
        let bridge = &mut *self.bridge;
        let o = ops.clone();
        let resp = self.recorder.host_call("db.transact", args, || {
            Ok(serde_json::json!(bridge.db_transact(o)?))
        })?;
        Ok(resp.as_u64().unwrap_or(0))
    }

    pub fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        let args = serde_json::json!([collection, id]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.to_string() },
            "db.get",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let i = id.to_string();
        self.recorder
            .host_call("db.get", args, || bridge.db_get(&c, &i))
    }

    pub fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        let args = serde_json::json!([collection]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.to_string() },
            "db.list",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let resp = self.recorder.host_call("db.list", args, || {
            Ok(serde_json::json!(bridge.db_list(&c)?))
        })?;
        Ok(resp.as_array().cloned().unwrap_or_default())
    }

    /// `ctx.db.query(collection, query)` — run the structured query plan against
    /// the collection and return the matched rows (DL-15). Like the other `db.*`
    /// reads it is gated on `db.read` for `collection` and recorded: in record
    /// mode the call + the bridge's rows are appended as a `RecordedCall`; on
    /// replay the recorded rows are *served* (the live storage is never touched),
    /// so replay stays byte-identical. A denied query is recorded as the run's
    /// denial and no rows are returned.
    pub fn db_query(
        &mut self,
        collection: &str,
        mut query: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Pin the query's `from` to the capability-checked `collection` BEFORE it
        // reaches any bridge, so a caller cannot read an ungranted collection by
        // putting a different `from` in the query body — the host is the single
        // source of truth for which collection a db.read grant authorizes
        // (review 052 #2; the real StorageHostBridge also pins this, but
        // normalizing here means no bridge — incl. test doubles — can widen).
        if let Some(obj) = query.as_object_mut() {
            obj.insert("from".into(), serde_json::Value::String(collection.to_string()));
        }
        let args = serde_json::json!([collection, query]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.to_string() },
            "db.query",
            &args,
        )?;
        let bridge = &mut *self.bridge;
        let c = collection.to_string();
        let q = query.clone();
        self.recorder
            .host_call("db.query", args, || bridge.db_query(&c, q))
    }

    /// `ctx.db.watch(watch_id, query)` — register a live query (DL-16, `forge/spec/
    /// live-queries.md`). The watched collection is the query's `from`; registration
    /// requires the SAME `db.read` grant as `ctx.db.from(...).all()` over that
    /// collection (spec §Registration), so the gate is `db.read` for `from` — pinned
    /// into the query so a caller cannot widen the watch to an ungranted collection
    /// by naming a different `from` (mirrors [`db_query`](Self::db_query)). The call
    /// is recorded as `db.watch` so replay serves the recording; a denied watch is
    /// recorded as the run's denial and registers nothing.
    pub fn db_watch(
        &mut self,
        watch_id: &str,
        mut query: serde_json::Value,
    ) -> Result<String> {
        let collection = watch_collection(&query)?;
        // Pin the query's `from` to the watched collection BEFORE the gate / bridge
        // see it, so the recorded args and the registered plan agree and the gate
        // authorizes exactly the collection that will be watched.
        if let Some(obj) = query.as_object_mut() {
            obj.insert("from".into(), serde_json::Value::String(collection.clone()));
        }
        let args = serde_json::json!([watch_id, query]);
        self.check_or_record_denial(
            &HostCall::Db { op: Access::Read, collection: collection.clone() },
            "db.watch",
            &args,
        )?;
        // Owner-collision gate (review 135 #1): the `watch_id` is applet-visible, but a
        // `watch_id` already registered by a DIFFERENT applet must NOT be re-registered
        // here — that would let one applet hijack another's subscription. Detect the
        // collision BEFORE recording the call and surface it through the SAME recorded-
        // denial path as a policy denial, so `ctx.db.watch` returns `PermissionDenied`
        // at host-call time (the run records the denial and registers nothing) instead
        // of returning success and having the facade silently drop the intent after the
        // run when it folds the registration owner-scoped.
        if self.bridge.db_watch_owner_conflict(watch_id) {
            let denied = forge_domain::CoreError::PermissionDenied(format!(
                "watch_id `{watch_id}` is already registered by another applet"
            ));
            self.recorder.record_denial("db.watch", args, &denied)?;
            return Err(denied);
        }
        let bridge = &mut *self.bridge;
        let id = watch_id.to_string();
        let q = query.clone();
        let resp = self.recorder.host_call("db.watch", args, || {
            Ok(serde_json::json!(bridge.db_watch(&id, q)?))
        })?;
        Ok(resp.as_str().unwrap_or(watch_id).to_string())
    }

    /// `ctx.db.unwatch(watch_id)` — cancel a live query (DL-16). Idempotent: an
    /// unknown id is a no-op; after it commits the watch receives no further
    /// notifications.
    ///
    /// Cancellation is a CONTROL op that reads no collection data — the `watch_id`
    /// alone names no collection, and dropping a subscription cannot reveal a row —
    /// so it is NOT policy-gated on a `db.read` scope (a `db.read` gate keyed on the
    /// empty collection would spuriously deny every unwatch, since no manifest grants
    /// the empty collection). It is, however, RECORDED (like the `ui.dispatch_event`
    /// envelope) so the cancellation is part of the replayable trace and replay
    /// serves the recording without re-touching the live watch registry.
    pub fn db_unwatch(&mut self, watch_id: &str) -> Result<()> {
        let args = serde_json::json!([watch_id]);
        let bridge = &mut *self.bridge;
        let id = watch_id.to_string();
        self.recorder.host_call("db.unwatch", args, || {
            bridge.db_unwatch(&id).map(|()| serde_json::Value::Null)
        })?;
        Ok(())
    }
}

/// The distinct collections a `ctx.db.transact(ops)` group touches, in first-touch
/// order (DL-17). `ops` is the JSON array of `{op, collection, …}` leaves; each leaf
/// must name a string `collection`. Used to gate `db.write` on EVERY collection a
/// group touches before any leaf is applied, so an atomic group is denied as a whole
/// when any leaf targets an ungranted collection (no partial write).
fn transact_collections(ops: &serde_json::Value) -> Result<Vec<String>> {
    let leaves = ops.as_array().ok_or_else(|| {
        forge_domain::CoreError::QueryError(
            "ctx.db.transact(ops) requires an array of mutation leaves".into(),
        )
    })?;
    let mut collections: Vec<String> = Vec::new();
    for leaf in leaves {
        let collection = leaf
            .get("collection")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                forge_domain::CoreError::QueryError(
                    "ctx.db.transact leaf requires a string 'collection'".into(),
                )
            })?
            .to_string();
        if !collections.contains(&collection) {
            collections.push(collection);
        }
    }
    Ok(collections)
}

/// The watched collection for a `db.watch` query value: its required string `from`
/// (DL-16 / DL-15). A query without a string `from` is a `QueryError` — a watch
/// must name a row collection to observe.
fn watch_collection(query: &serde_json::Value) -> Result<String> {
    query
        .get("from")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| {
            forge_domain::CoreError::QueryError(
                "ctx.db.watch(watch_id, query) requires a string 'from' collection".into(),
            )
        })
}
