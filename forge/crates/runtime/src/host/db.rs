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
}
