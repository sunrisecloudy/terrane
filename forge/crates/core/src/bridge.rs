//! [`StorageHostBridge`]: the [`HostBridge`] that backs `ctx.*` effects with the
//! real workspace [`Store`] — this is where the spine's "Rust capability ctx →
//! SQLite write → UI tree patch" links live.
//!
//! prd-merged/01 CR-1/CR-3 (effects injected through `ctx`, never imported) +
//! prd-merged/02 DL-4/DL-18 (records projection, KV namespaces) +
//! prd-merged/05 UI-1 (tree diff → patches).
//!
//! The bridge is a thin effect surface: policy/capability gating is enforced one
//! layer up by the runtime's [`HostContext`](forge_runtime::HostContext) (built
//! from a [`PolicyEngine`](forge_policy::PolicyEngine)) *before* any method here
//! runs, exactly as the [`HostBridge`] contract promises. So a denied call never
//! reaches the Store.
//!
//! Two effects are special:
//!   * `db.insert` builds a [`RecordEnvelope`] and `put_record`s it into the
//!     `records` projection — the literal **SQLite write** link of the spine —
//!     returning the new record id.
//!   * `ui.render` parses the rendered tree into a [`forge_ui::Node`], diffs it
//!     against the previously-rendered tree, and captures the resulting
//!     [`forge_ui::Patch`] list so [`WorkspaceCore`](crate::WorkspaceCore) can
//!     emit a `ui.patch` `CoreEvent` per render — the **UI tree patch** link.

use forge_domain::{
    CollectionId, CoreError, LogicalTimestamp, RecordEnvelope, RecordId, Result,
};
use forge_runtime::HostBridge;
use forge_storage::Store;
use std::collections::BTreeMap;

/// A single UI render captured during a run: the full tree the applet rendered,
/// plus the minimal patch list that turns the *previous* rendered tree into it
/// (prd-merged/05 UI-1). The first render diffs against `None` → a single
/// root `replace`.
#[derive(Debug, Clone)]
pub struct UiRender {
    /// The full rendered node tree (canonical JSON).
    pub tree: serde_json::Value,
    /// The patch list from the previous tree to this one (canonical JSON).
    pub patches: serde_json::Value,
}

/// A [`HostBridge`] backed by a real [`Store`], scoped to one applet.
///
/// `ctx.storage` keys are namespaced per applet (`applet/<id>` namespace) so two
/// applets in the same workspace can't read or clobber each other's KV (DL-18).
/// `ctx.db` writes land in the shared `records` projection keyed by the
/// collection the applet names (capability gating upstream limits *which*
/// collections it may touch).
pub struct StorageHostBridge<'a> {
    store: &'a mut Store,
    /// Applet id, used to scope the KV namespace.
    applet_ns: String,
    /// Logical clock for record `created_at`/`updated_at`; advances per write so
    /// the run's effects are ordered without consulting wall-clock.
    logical: LogicalTimestamp,
    /// Per-collection monotone counter for deterministic record ids
    /// (`<collection>/<n>`), mirroring the in-memory bridge so a real run's ids
    /// are reproducible.
    db_counter: BTreeMap<String, u64>,
    /// The previous rendered tree, used as the diff base for the next render.
    prev_ui: Option<forge_ui::Node>,
    /// Every UI render captured this run (tree + patch list), in order.
    pub ui_renders: Vec<UiRender>,
    /// Every log line captured this run.
    pub logs: Vec<String>,
}

impl<'a> StorageHostBridge<'a> {
    /// Build a bridge over `store`, scoped to `applet_id`.
    pub fn new(store: &'a mut Store, applet_id: &str) -> Self {
        StorageHostBridge {
            store,
            applet_ns: format!("applet/{applet_id}"),
            logical: LogicalTimestamp::default(),
            db_counter: BTreeMap::new(),
            prev_ui: None,
            ui_renders: Vec::new(),
            logs: Vec::new(),
        }
    }

    /// Advance and return the next logical timestamp for a write.
    fn tick(&mut self) -> LogicalTimestamp {
        self.logical = self.logical.next();
        self.logical
    }

    /// Map the JSON `record` an applet passed to `ctx.db.insert` into the
    /// display-named `fields` map of a [`RecordEnvelope`]. A non-object record
    /// is rejected as a `ValidationError` (the `DbRecord` contract is an object).
    fn record_fields(
        record: &serde_json::Value,
    ) -> Result<BTreeMap<String, serde_json::Value>> {
        match record {
            serde_json::Value::Object(map) => {
                Ok(map.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            }
            other => Err(CoreError::ValidationError(format!(
                "ctx.db.insert record must be an object, got {other}"
            ))),
        }
    }
}

impl HostBridge for StorageHostBridge<'_> {
    fn storage_get(&mut self, key: &str) -> Result<serde_json::Value> {
        match self.store.kv_get(&self.applet_ns, key)? {
            // Stored as canonical JSON bytes; parse back to a JSON value so the
            // applet sees structured data, not a string blob.
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                CoreError::StorageError(format!("ctx.storage.get decode failed: {e}"))
            }),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(&value)
            .map_err(|e| CoreError::StorageError(format!("ctx.storage.set encode failed: {e}")))?;
        self.store
            .kv_set(&self.applet_ns, key, &bytes, "application/json")
    }

    fn storage_delete(&mut self, key: &str) -> Result<()> {
        self.store.kv_delete(&self.applet_ns, key)
    }

    fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>> {
        self.store.kv_list(&self.applet_ns, prefix)
    }

    fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String> {
        let fields = Self::record_fields(&record)?;
        // Deterministic, readable record id: `<collection>/<n>`. The per-run
        // counter is seeded on first use from the count of records already in the
        // collection, so ids never collide with a prior run's writes (each run
        // would otherwise restart at 1 and clobber `<collection>/1`). The id is
        // captured into the recorded trace, so replay (which serves the recorded
        // response) reproduces it without re-running this generator.
        let next = match self.db_counter.get(collection) {
            Some(n) => n + 1,
            None => {
                let existing = self.store.list_records(collection)?.len() as u64;
                existing + 1
            }
        };
        self.db_counter.insert(collection.to_string(), next);
        let id = format!("{collection}/{next}");
        let at = self.tick();
        let env = RecordEnvelope::new(
            CollectionId::new(collection),
            RecordId::new(id.clone()),
            fields,
            at,
        );
        // THE SQLite write link of the spine.
        self.store.put_record(&env)?;
        Ok(id)
    }

    fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        match self.store.get_record(collection, id)? {
            Some(env) => serde_json::to_value(env.fields).map_err(|e| {
                CoreError::StorageError(format!("ctx.db.get encode failed: {e}"))
            }),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        let records = self.store.list_records(collection)?;
        records
            .into_iter()
            .map(|env| {
                serde_json::to_value(env.fields).map_err(|e| {
                    CoreError::StorageError(format!("ctx.db.list encode failed: {e}"))
                })
            })
            .collect()
    }

    fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        // Parse the rendered tree into a typed Node (unknown component types are
        // tolerated as Node::Unknown, UI-6 — never an error here).
        let node = forge_ui::from_str(&tree.to_string())?;
        // Diff against the previous tree → minimal index-path patches (UI-1).
        let patches = forge_ui::diff(self.prev_ui.as_ref(), &node);
        let patches_json = serde_json::to_value(&patches).map_err(|e| {
            CoreError::ValidationError(format!("ui patch serialize failed: {e}"))
        })?;
        // Re-serialize the parsed node canonically so the emitted tree is the
        // catalog-normalized shape (and round-trips for the renderer).
        let canonical = forge_ui::to_canonical_string(&node)?;
        let tree_json = serde_json::from_str(&canonical).map_err(|e| {
            CoreError::ValidationError(format!("ui tree re-parse failed: {e}"))
        })?;
        self.ui_renders.push(UiRender {
            tree: tree_json,
            patches: patches_json,
        });
        self.prev_ui = Some(node);
        Ok(())
    }

    fn log(&mut self, line: &str) -> Result<()> {
        self.logs.push(line.to_string());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        Store::open_in_memory().unwrap()
    }

    #[test]
    fn db_insert_writes_a_record_into_the_projection() {
        let mut s = store();
        let id = {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.db_insert("tasks", serde_json::json!({ "title": "Ship", "done": false }))
                .unwrap()
        };
        assert_eq!(id, "tasks/1");
        let env = s.get_record("tasks", "tasks/1").unwrap().unwrap();
        assert_eq!(env.fields["title"], serde_json::json!("Ship"));
        assert_eq!(env.fields["done"], serde_json::json!(false));
    }

    #[test]
    fn db_insert_seeds_id_from_existing_records_across_bridges() {
        // Two separate bridges (≈ two runs) over the same store must not collide.
        let mut s = store();
        let id1 = {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.db_insert("tasks", serde_json::json!({ "t": 1 })).unwrap()
        };
        let id2 = {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.db_insert("tasks", serde_json::json!({ "t": 2 })).unwrap()
        };
        assert_eq!(id1, "tasks/1");
        assert_eq!(id2, "tasks/2", "second run must not clobber the first record");
        assert_eq!(s.list_records("tasks").unwrap().len(), 2);
    }

    #[test]
    fn db_insert_rejects_non_object_record() {
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        let err = b.db_insert("tasks", serde_json::json!("not an object")).unwrap_err();
        assert_eq!(err.code(), "ValidationError");
    }

    #[test]
    fn storage_roundtrips_through_kv_namespaced_per_applet() {
        let mut s = store();
        {
            let mut b = StorageHostBridge::new(&mut s, "app1");
            b.storage_set("app/k", serde_json::json!({ "v": 1 })).unwrap();
            assert_eq!(b.storage_get("app/k").unwrap(), serde_json::json!({ "v": 1 }));
            assert_eq!(b.storage_list("app/").unwrap(), vec!["app/k".to_string()]);
            assert_eq!(b.storage_get("missing").unwrap(), serde_json::Value::Null);
        }
        // A different applet sees an isolated namespace.
        let mut b2 = StorageHostBridge::new(&mut s, "app2");
        assert_eq!(b2.storage_get("app/k").unwrap(), serde_json::Value::Null);
    }

    #[test]
    fn ui_render_first_render_is_root_replace_then_diffs() {
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        // First render → diff against None → single root replace.
        b.ui_render(serde_json::json!({
            "type": "Stack", "direction": "v",
            "children": [ { "type": "Text", "text": "A" } ]
        }))
        .unwrap();
        assert_eq!(b.ui_renders.len(), 1);
        let patches = b.ui_renders[0].patches.as_array().unwrap();
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0]["op"], serde_json::json!("replace"));
        assert!(b.ui_renders[0].tree.to_string().contains("\"A\""));

        // Second render changes only the Text → a minimal update_text patch.
        b.ui_render(serde_json::json!({
            "type": "Stack", "direction": "v",
            "children": [ { "type": "Text", "text": "B" } ]
        }))
        .unwrap();
        assert_eq!(b.ui_renders.len(), 2);
        let patches = b.ui_renders[1].patches.as_array().unwrap();
        assert_eq!(patches.len(), 1, "only the text changed → one patch");
        assert_eq!(patches[0]["op"], serde_json::json!("update_text"));
        assert_eq!(patches[0]["value"], serde_json::json!("B"));
    }

    #[test]
    fn ui_render_tolerates_unknown_node_types() {
        // UI-6: an unknown component type is not an error; it round-trips.
        let mut s = store();
        let mut b = StorageHostBridge::new(&mut s, "app1");
        b.ui_render(serde_json::json!({ "type": "FutureWidget", "x": 1 })).unwrap();
        assert_eq!(b.ui_renders.len(), 1);
        assert!(b.ui_renders[0].tree.to_string().contains("FutureWidget"));
    }
}
