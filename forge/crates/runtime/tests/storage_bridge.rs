//! Minimal proof that the spine reaches real SQLite: a `HostBridge` backed by
//! `forge_storage::Store`. This is the "→ SQLite write" stage of the M0a jewel
//! made concrete (full wiring lives in forge-core later; this is the smoke).
//!
//! prd-merged/01 CR-3 (`ctx.storage`/`ctx.db`); prd-merged/02 DL-4/DL-18.

mod common;

use common::{owner, program, spine_manifest};
use forge_domain::{CollectionId, LogicalTimestamp, RecordEnvelope, RecordId, Result, RunOutcome};
use forge_runtime::{record_run, HostBridge};
use forge_storage::{Query, QueryResult, Store};

/// A `HostBridge` that persists `ctx.storage`/`ctx.db` into a real SQLite
/// `Store`. KV values are stored as canonical JSON bytes; db records become
/// `RecordEnvelope`s in the `records` projection. UI/log are kept in memory.
struct StoreHostBridge {
    store: Store,
    namespace: String,
    seq: u64,
    ui: Vec<serde_json::Value>,
    logs: Vec<String>,
}

impl StoreHostBridge {
    fn new(store: Store) -> Self {
        StoreHostBridge {
            store,
            namespace: "app_test".into(),
            seq: 0,
            ui: Vec::new(),
            logs: Vec::new(),
        }
    }
}

impl HostBridge for StoreHostBridge {
    fn storage_get(&mut self, key: &str) -> Result<serde_json::Value> {
        match self.store.kv_get(&self.namespace, key)? {
            Some(bytes) => Ok(serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn storage_set(&mut self, key: &str, value: serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec(&value).unwrap_or_default();
        self.store
            .kv_set(&self.namespace, key, &bytes, "application/json")
    }

    fn storage_delete(&mut self, key: &str) -> Result<()> {
        self.store.kv_delete(&self.namespace, key)
    }

    fn storage_list(&mut self, prefix: &str) -> Result<Vec<String>> {
        self.store.kv_list(&self.namespace, prefix)
    }

    fn db_insert(&mut self, collection: &str, record: serde_json::Value) -> Result<String> {
        self.seq += 1;
        let id = format!("{collection}/{}", self.seq);
        let fields = record
            .as_object()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let env = RecordEnvelope::new(
            CollectionId::new(collection),
            RecordId::new(&id),
            fields,
            LogicalTimestamp(self.seq),
        );
        self.store.put_record(&env)?;
        Ok(id)
    }

    fn db_get(&mut self, collection: &str, id: &str) -> Result<serde_json::Value> {
        match self.store.get_record(collection, id)? {
            Some(env) => Ok(serde_json::json!(env.fields)),
            None => Ok(serde_json::Value::Null),
        }
    }

    fn db_list(&mut self, collection: &str) -> Result<Vec<serde_json::Value>> {
        Ok(self
            .store
            .list_records(collection)?
            .into_iter()
            .map(|env| serde_json::json!(env.fields))
            .collect())
    }

    fn db_query(
        &mut self,
        collection: &str,
        query: serde_json::Value,
    ) -> Result<serde_json::Value> {
        // Exercise the REAL query engine: parse the plan, pin `from` to the
        // trusted collection, run it, and surface row `fields` like `db.list`.
        let mut q = Query::from_fixture_value(&query)?;
        q.from = collection.to_string();
        let rows = match self.store.query(&q)? {
            QueryResult::Rows(rows) => rows
                .into_iter()
                .map(|row| serde_json::json!(row.envelope.fields))
                .collect(),
            _ => Vec::new(),
        };
        Ok(serde_json::Value::Array(rows))
    }

    fn ui_render(&mut self, tree: serde_json::Value) -> Result<()> {
        self.ui.push(tree);
        Ok(())
    }

    fn log(&mut self, line: &str) -> Result<()> {
        self.logs.push(line.to_string());
        Ok(())
    }
}

/// The whole spine through real SQLite: run a program that writes KV + inserts a
/// db record, then assert the bytes actually landed in the `Store`.
#[test]
fn spine_writes_reach_sqlite() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.storage.set("app/greeting", "hello " + input.who);
            const id = await ctx.db.insert("tasks", { title: "from-spine" });
            const back = await ctx.db.get("tasks", id);
            await ctx.ui.render({ type: "text", value: back.title });
            return { ok: true, value: { id, greeting: await ctx.storage.get("app/greeting") } };
        }"#,
    );

    let store = Store::open_in_memory().unwrap();
    let mut bridge = StoreHostBridge::new(store);

    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({"who": "sqlite"}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();

    let value = match rec.outcome {
        RunOutcome::Completed { result } => result.value,
        other => panic!("spine run failed: {other:?}"),
    };
    assert_eq!(value["greeting"], serde_json::json!("hello sqlite"));
    assert_eq!(value["id"], serde_json::json!("tasks/1"));

    // The KV write is durable in SQLite.
    let raw = bridge
        .store
        .kv_get("app_test", "app/greeting")
        .unwrap()
        .unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&raw).unwrap();
    assert_eq!(stored, serde_json::json!("hello sqlite"));

    // The db record landed in the projection.
    let env = bridge
        .store
        .get_record("tasks", "tasks/1")
        .unwrap()
        .unwrap();
    assert_eq!(env.fields["title"], serde_json::json!("from-spine"));

    // The UI tree was captured.
    assert_eq!(
        bridge.ui.last().unwrap()["value"],
        serde_json::json!("from-spine")
    );
}

/// `ctx.db.query` reaches the REAL forge-storage query engine through the host
/// path: insert rows, query with a filter plan, and assert only the matching
/// rows come back (DL-15). The trusted `collection` pins `from`, so an applet
/// cannot query a different collection by naming it in the plan.
#[test]
fn db_query_runs_the_real_engine_through_the_host() {
    let prog = program(
        r#"export async function main(ctx, input) {
            await ctx.db.insert("tasks", { title: "A", status: "todo" });
            await ctx.db.insert("tasks", { title: "B", status: "done" });
            await ctx.db.insert("tasks", { title: "C", status: "todo" });
            const rows = await ctx.db.query("tasks", {
                from: "tasks",
                where: { field: "status", op: "eq", value: "todo" },
                orderBy: ["title", "asc"]
            });
            return { ok: true, value: rows.map(r => r.title) };
        }"#,
    );

    let store = Store::open_in_memory().unwrap();
    let mut bridge = StoreHostBridge::new(store);
    let rec = record_run(
        &prog,
        &spine_manifest(),
        &owner(),
        &serde_json::json!({}),
        1,
        0,
        &mut bridge,
    )
    .unwrap();
    let value = match rec.outcome {
        RunOutcome::Completed { result } => result.value,
        other => panic!("query run failed: {other:?}"),
    };
    assert_eq!(value, serde_json::json!(["A", "C"]));
}
