//! The host bridge: the seam through which the sandbox performs effects.
//!
//! prd-merged/01 CR-1 (zero ambient capability) + CR-3 (`ctx` namespaces). The
//! engine never imports storage/db/ui directly; it calls *out* to a
//! [`HostBridge`] so effects are **injected**, capability-checked, and
//! recordable. The `ctx` object the engine installs into the JS realm forwards
//! every method here, but only after the policy check and the host-call counter
//! (in the engine). The bridge itself is pure effect — no policy logic lives
//! here.
//!
//! This module is **target-independent** (no QuickJS); it compiles on
//! `wasm32-unknown-unknown`. Note `time.now()` and `random.next()` are *not*
//! bridge methods: those are deterministic seams owned by the recorder
//! (prd-merged/01 CR-11), not host effects.

use forge_domain::Result;
use std::collections::BTreeMap;

/// The effect surface the runtime calls out to. Each method maps to one
/// capability-checked `ctx.*` host call. Implementors provide real storage/db
/// (forge-storage) or an in-memory test double.
///
/// Args/returns are `serde_json::Value` so the trace ([`forge_domain::RecordedCall`])
/// can capture them canonically and the engine stays decoupled from concrete
/// storage types in M0a.
pub trait HostBridge {
    /// `ctx.storage.get(key)` → the stored JSON value, or `null` if absent.
    fn storage_get(&mut self, key: &str) -> Result<serde_json::Value>;
    /// `ctx.storage.set(key, value)`.
    fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()>;
    /// `ctx.storage.delete(key)`.
    fn storage_delete(&mut self, key: &str) -> Result<()>;
    /// `ctx.storage.list(prefix)` → sorted matching keys.
    fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>>;

    /// `ctx.db.insert(collection, record)` → the inserted record's id.
    fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String>;
    /// `ctx.db.get(collection, id)` → the record JSON, or `null` if absent.
    fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value>;
    /// `ctx.db.list(collection)` → all records in the collection.
    fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>>;

    /// `ctx.ui.render(tree)` — emit a UI tree for the shell to paint.
    fn ui_render(&mut self, tree: serde_json::Value) -> Result<()>;

    /// `ctx.log(line)` — append a log line (the engine enforces `log_bytes`).
    fn log(&mut self, line: &str) -> Result<()>;
}

/// An in-memory [`HostBridge`] for tests and replay sandboxes: storage in a
/// `BTreeMap`, db rows in per-collection vectors, and the last UI tree + all log
/// lines captured for assertions. No SQLite needed.
///
/// Db ids are assigned deterministically (`<collection>/<n>`) so a record-mode
/// run is itself reproducible without relying on wall-clock or RNG.
#[derive(Debug, Default)]
pub struct MemoryHostBridge {
    storage: BTreeMap<String, serde_json::Value>,
    db: BTreeMap<String, Vec<(String, serde_json::Value)>>,
    db_counter: BTreeMap<String, u64>,
    /// Every UI tree rendered this run, in order (last is the current view).
    pub ui_trees: Vec<serde_json::Value>,
    /// Every log line captured this run.
    pub logs: Vec<String>,
}

impl MemoryHostBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// The most recently rendered UI tree, if any (test convenience).
    pub fn last_ui(&self) -> Option<&serde_json::Value> {
        self.ui_trees.last()
    }

    /// Direct read of a stored value (test convenience; bypasses recording).
    pub fn peek_storage(&self, key: &str) -> Option<&serde_json::Value> {
        self.storage.get(key)
    }
}

impl HostBridge for MemoryHostBridge {
    fn storage_get(&mut self, key: &str) -> Result<serde_json::Value> {
        Ok(self
            .storage
            .get(key)
            .cloned()
            .unwrap_or(serde_json::Value::Null))
    }

    fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        self.storage.insert(key.to_string(), value);
        Ok(())
    }

    fn storage_delete(&mut self, key: &str) -> Result<()> {
        self.storage.remove(key);
        Ok(())
    }

    fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .storage
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }

    fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String> {
        let n = self.db_counter.entry(collection.to_string()).or_insert(0);
        *n += 1;
        let id = format!("{collection}/{n}");
        self.db
            .entry(collection.to_string())
            .or_default()
            .push((id.clone(), record));
        Ok(id)
    }

    fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        Ok(self
            .db
            .get(collection)
            .and_then(|rows| rows.iter().find(|(rid, _)| rid == id))
            .map(|(_, rec)| rec.clone())
            .unwrap_or(serde_json::Value::Null))
    }

    fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        Ok(self
            .db
            .get(collection)
            .map(|rows| rows.iter().map(|(_, rec)| rec.clone()).collect())
            .unwrap_or_default())
    }

    fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        self.ui_trees.push(tree);
        Ok(())
    }

    fn log(&mut self, line: &str) -> Result<()> {
        self.logs.push(line.to_string());
        Ok(())
    }
}

/// A [`HostBridge`] that refuses every effect — used as the live bridge in
/// replay mode, where the recorder *serves* recorded responses and the live
/// bridge must never be consulted. If replay ever calls through to the live
/// bridge it is a bug, surfaced loudly as a `RuntimeError`.
#[derive(Debug, Default)]
pub struct NullBridge;

impl NullBridge {
    pub fn new() -> Self {
        NullBridge
    }
}

fn null_violation(method: &str) -> forge_domain::CoreError {
    forge_domain::CoreError::RuntimeError(format!(
        "replay attempted a live host effect ({method}); the recorder must serve recorded responses"
    ))
}

impl HostBridge for NullBridge {
    fn storage_get(&mut self, _key: &str) -> Result<serde_json::Value> {
        Err(null_violation("storage.get"))
    }
    fn storage_set(&mut self, _key: &str, _value: serde_json::Value) -> Result<()> {
        Err(null_violation("storage.set"))
    }
    fn storage_delete(&mut self, _key: &str) -> Result<()> {
        Err(null_violation("storage.delete"))
    }
    fn storage_list(&mut self, _prefix: &str) -> Result<Vec<String>> {
        Err(null_violation("storage.list"))
    }
    fn db_insert(&mut self, _collection: &str, _record: serde_json::Value) -> Result<String> {
        Err(null_violation("db.insert"))
    }
    fn db_get(&mut self, _collection: &str, _id: &str) -> Result<serde_json::Value> {
        Err(null_violation("db.get"))
    }
    fn db_list(&mut self, _collection: &str) -> Result<Vec<serde_json::Value>> {
        Err(null_violation("db.list"))
    }
    fn ui_render(&mut self, _tree: serde_json::Value) -> Result<()> {
        Err(null_violation("ui.render"))
    }
    fn log(&mut self, _line: &str) -> Result<()> {
        Err(null_violation("log"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_storage_roundtrips() {
        let mut b = MemoryHostBridge::new();
        assert_eq!(b.storage_get("k").unwrap(), serde_json::Value::Null);
        b.storage_set("k", serde_json::json!("v")).unwrap();
        assert_eq!(b.storage_get("k").unwrap(), serde_json::json!("v"));
        b.storage_set("app/a", serde_json::json!(1)).unwrap();
        b.storage_set("app/b", serde_json::json!(2)).unwrap();
        assert_eq!(b.storage_list("app/").unwrap(), vec!["app/a", "app/b"]);
        b.storage_delete("k").unwrap();
        assert_eq!(b.storage_get("k").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn memory_db_inserts_deterministic_ids() {
        let mut b = MemoryHostBridge::new();
        let id1 = b.db_insert("tasks", serde_json::json!({"t": "a"})).unwrap();
        let id2 = b.db_insert("tasks", serde_json::json!({"t": "b"})).unwrap();
        assert_eq!(id1, "tasks/1");
        assert_eq!(id2, "tasks/2");
        assert_eq!(
            b.db_get("tasks", "tasks/1").unwrap(),
            serde_json::json!({"t": "a"})
        );
        assert_eq!(b.db_list("tasks").unwrap().len(), 2);
        assert_eq!(
            b.db_get("tasks", "missing").unwrap(),
            serde_json::Value::Null
        );
    }

    #[test]
    fn memory_ui_and_log_capture() {
        let mut b = MemoryHostBridge::new();
        b.ui_render(serde_json::json!({"type": "text", "value": "hi"}))
            .unwrap();
        b.log("line one").unwrap();
        assert_eq!(b.last_ui().unwrap()["value"], serde_json::json!("hi"));
        assert_eq!(b.logs, vec!["line one".to_string()]);
    }

    #[test]
    fn null_bridge_refuses_every_effect() {
        let mut b = NullBridge::new();
        assert!(b.storage_get("k").is_err());
        assert!(b.storage_set("k", serde_json::Value::Null).is_err());
        assert!(b.ui_render(serde_json::Value::Null).is_err());
        assert!(b.db_insert("c", serde_json::Value::Null).is_err());
        assert_eq!(b.log("x").unwrap_err().code(), "RuntimeError");
    }
}
