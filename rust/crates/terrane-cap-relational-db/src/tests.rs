use std::any::Any;

use serde_json::json;
use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, EventRecord, QueryValue, ReadValue, ResourceReadCtx, StateStore,
};
use terrane_cap_kv::{KvCapability, KvState};

use crate::RelationalDbCapability;

#[derive(Default)]
struct TestState {
    kv: KvState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        match namespace {
            "kv" => Some(&self.kv),
            _ => None,
        }
    }

    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        match namespace {
            "kv" => Some(&mut self.kv),
            _ => None,
        }
    }
}

struct Bus;
impl CapBus for Bus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<QueryValue> {
        match (cap, name) {
            ("app", "exists") => Ok(QueryValue::Bool(true)),
            _ => unreachable!(),
        }
    }
}

fn apply(state: &mut TestState, records: Vec<EventRecord>) {
    for record in records {
        KvCapability.fold(state, &record).unwrap();
    }
}

fn decide_result(
    state: &TestState,
    name: &str,
    args: Vec<String>,
) -> terrane_cap_interface::Result<Vec<EventRecord>> {
    let bus = Bus;
    let ctx = CommandCtx { state, bus: &bus };
    match RelationalDbCapability.decide(ctx, name, &args)? {
        terrane_cap_interface::Decision::Commit(records) => Ok(records),
        _ => unreachable!(),
    }
}

fn decide(state: &TestState, name: &str, args: Vec<String>) -> Vec<EventRecord> {
    decide_result(state, name, args).unwrap()
}

fn dispatch(state: &mut TestState, name: &str, args: Vec<String>) {
    let records = decide(state, name, args);
    apply(state, records);
}

fn read(state: &TestState, method: &str, args: Vec<String>) -> ReadValue {
    let bus = Bus;
    let ctx = ResourceReadCtx {
        state,
        bus: &bus,
        app: "app",
    };
    RelationalDbCapability
        .read_resource(ctx, method, &args)
        .unwrap()
}

fn spec() -> String {
    serde_json::to_string(&json!({
        "specVersion": 1,
        "schemaVersion": 1,
        "fields": {
            "tenantId": { "type": "string", "required": true },
            "userId": { "type": "string", "required": true },
            "email": { "type": "string", "required": true, "format": "email" },
            "name": { "type": "string", "required": true },
            "status": { "type": "string", "default": "active", "enum": ["active", "disabled"] },
            "createdAt": { "type": "string", "required": true, "format": "date-time" }
        },
        "primaryKey": { "partition": ["tenantId"], "sort": ["userId"] },
        "indexes": {
            "byEmail": { "partition": ["tenantId", "email"], "unique": true, "projection": { "type": "include", "fields": ["name", "status"] } },
            "byStatus": { "partition": ["tenantId", "status"], "sort": ["createdAt", "userId"], "projection": { "type": "all" } }
        },
        "constraints": {
            "emailPerTenant": { "type": "unique", "fields": ["tenantId", "email"] }
        },
        "options": { "unknownFields": "reject", "defaultQueryLimit": 10, "maxQueryLimit": 50, "canonicalJson": true }
    })).unwrap()
}

fn row(user_id: &str, email: &str, status: &str) -> String {
    serde_json::to_string(&json!({
        "tenantId": "acme",
        "userId": user_id,
        "email": email,
        "name": format!("User {user_id}"),
        "status": status,
        "createdAt": format!("2026-06-28T00:00:0{}Z", if user_id == "u1" { 1 } else { 2 })
    }))
    .unwrap()
}

#[test]
fn defines_puts_gets_queries_updates_and_deletes_rows() {
    let mut state = TestState::default();
    dispatch(
        &mut state,
        "relational_db.defineTable",
        vec!["app".into(), "users".into(), spec()],
    );

    dispatch(
        &mut state,
        "relational_db.put",
        vec![
            "app".into(),
            "users".into(),
            row("u1", "ada@example.com", "active"),
        ],
    );
    dispatch(
        &mut state,
        "relational_db.put",
        vec![
            "app".into(),
            "users".into(),
            row("u2", "grace@example.com", "disabled"),
        ],
    );

    let ReadValue::OptString(Some(got)) = read(
        &state,
        "get",
        vec![
            "users".into(),
            r#"{"tenantId":"acme","userId":"u1"}"#.into(),
        ],
    ) else {
        panic!("missing get result");
    };
    assert!(got.contains("ada@example.com"));

    let ReadValue::OptString(Some(result)) = read(
        &state,
        "query",
        vec![
            "users".into(),
            "byEmail".into(),
            r#"{"partition":{"tenantId":"acme","email":"ada@example.com"},"limit":1}"#.into(),
        ],
    ) else {
        panic!("missing query result");
    };
    let rows: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 1);
    assert_eq!(rows[0]["userId"], "u1");

    let ReadValue::OptString(Some(result)) = read(
        &state,
        "query",
        vec![
            "users".into(),
            "byStatus".into(),
            r#"{"partition":["acme","active"],"select":"projection"}"#.into(),
        ],
    ) else {
        panic!("missing projection query result");
    };
    let rows: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(rows[0]["email"], "ada@example.com");

    let conflict = decide_result(
        &state,
        "relational_db.put",
        vec![
            "app".into(),
            "users".into(),
            row("u3", "ada@example.com", "active"),
        ],
    );
    assert!(
        conflict.is_err(),
        "duplicate unique email must be rejected before events are emitted"
    );

    dispatch(
        &mut state,
        "relational_db.put",
        vec![
            "app".into(),
            "users".into(),
            row("u1", "ada2@example.com", "disabled"),
        ],
    );
    let ReadValue::OptString(Some(result)) = read(
        &state,
        "query",
        vec![
            "users".into(),
            "byEmail".into(),
            r#"{"partition":{"tenantId":"acme","email":"ada@example.com"}}"#.into(),
        ],
    ) else {
        panic!("missing old email query result");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );

    dispatch(
        &mut state,
        "relational_db.delete",
        vec![
            "app".into(),
            "users".into(),
            r#"{"tenantId":"acme","userId":"u1"}"#.into(),
        ],
    );
    let ReadValue::OptString(None) = read(
        &state,
        "get",
        vec![
            "users".into(),
            r#"{"tenantId":"acme","userId":"u1"}"#.into(),
        ],
    ) else {
        panic!("deleted row should be absent");
    };
}

#[test]
fn primary_key_sort_is_optional() {
    let spec = serde_json::to_string(&json!({
        "specVersion": 1,
        "schemaVersion": 1,
        "fields": {
            "id": { "type": "string", "required": true },
            "value": { "type": "integer", "required": true }
        },
        "primaryKey": { "partition": ["id"] },
        "options": { "unknownFields": "reject", "canonicalJson": true }
    }))
    .unwrap();
    let mut state = TestState::default();
    dispatch(
        &mut state,
        "relational_db.defineTable",
        vec!["app".into(), "counters".into(), spec],
    );
    dispatch(
        &mut state,
        "relational_db.put",
        vec![
            "app".into(),
            "counters".into(),
            r#"{"id":"one","value":1}"#.into(),
        ],
    );

    let ReadValue::OptString(Some(got)) = read(
        &state,
        "get",
        vec!["counters".into(), r#"{"id":"one"}"#.into()],
    ) else {
        panic!("missing no-sort primary key row");
    };
    assert!(got.contains("\"value\":1"));

    let ReadValue::OptString(Some(result)) = read(
        &state,
        "query",
        vec![
            "counters".into(),
            "primary".into(),
            r#"{"partition":{"id":"one"}}"#.into(),
        ],
    ) else {
        panic!("missing no-sort primary query result");
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&result).unwrap()[0]["id"],
        "one"
    );
}

#[test]
fn doc_contains_full_machine_schema_and_internal_layout() {
    let doc = RelationalDbCapability.doc(true);
    assert_eq!(doc.namespace, "relational_db");
    assert_eq!(doc.status, "stable");
    assert!(doc
        .schemas
        .iter()
        .any(|s| s.id == "terrane.relational_db.tableSpec.v1"));
    assert!(doc
        .internal
        .iter()
        .any(|note| note.body.contains("row/<table>")));
}
