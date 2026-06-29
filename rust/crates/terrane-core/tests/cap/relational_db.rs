//! Engine tests for the KV-backed `relational_db` capability.

use std::fs;

use serde_json::json;
use tempfile::tempdir;
use terrane_cap_interface::{CapBus, Capability, QueryValue, ReadValue, ResourceReadCtx};
use terrane_core::Core;

use crate::helpers::req;

struct ReadBus;
impl CapBus for ReadBus {
    fn query(&self, _cap: &str, _name: &str, _args: &[String]) -> terrane_core::Result<QueryValue> {
        unreachable!("relational_db resource reads do not need the bus")
    }
}

fn users_spec() -> String {
    serde_json::to_string(&json!({
        "specVersion": 1,
        "schemaVersion": 1,
        "fields": {
            "tenantId": { "type": "string", "required": true },
            "userId": { "type": "string", "required": true },
            "email": { "type": "string", "required": true, "format": "email" },
            "name": { "type": "string", "required": true },
            "createdAt": { "type": "string", "required": true, "format": "date-time" }
        },
        "primaryKey": { "partition": ["tenantId"], "sort": ["userId"] },
        "indexes": {
            "byEmail": { "partition": ["tenantId", "email"], "unique": true, "projection": { "type": "include", "fields": ["name"] } }
        },
        "options": { "unknownFields": "reject", "canonicalJson": true }
    }))
    .unwrap()
}

fn row(user_id: &str, email: &str) -> String {
    serde_json::to_string(&json!({
        "tenantId": "acme",
        "userId": user_id,
        "email": email,
        "name": format!("User {user_id}"),
        "createdAt": "2026-06-28T00:00:00Z"
    }))
    .unwrap()
}

#[test]
fn relational_db_dispatches_to_kv_events_and_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["crm", "CRM"])).unwrap();
    core.dispatch(req(
        "relational_db.defineTable",
        &["crm", "users", &users_spec()],
    ))
    .unwrap();
    core.dispatch(req(
        "relational_db.put",
        &["crm", "users", &row("u1", "ada@example.com")],
    ))
    .unwrap();

    let app_kv = &core.state().kv.data["crm"];
    assert!(app_kv
        .keys()
        .any(|k| k.starts_with("__terrane/rdb/v1/table/users/spec")));
    assert!(app_kv
        .keys()
        .any(|k| k.starts_with("__terrane/rdb/v1/idx/users/byEmail/")));

    let bus = ReadBus;
    let ctx = ResourceReadCtx {
        state: core.state(),
        bus: &bus,
        app: "crm",
    };
    let ReadValue::OptString(Some(result)) = terrane_cap_relational_db::RelationalDbCapability
        .read_resource(
            ctx,
            "query",
            &[
                "users".into(),
                "byEmail".into(),
                r#"{"partition":{"tenantId":"acme","email":"ada@example.com"}}"#.into(),
            ],
        )
        .unwrap()
    else {
        panic!("query did not return rows");
    };
    let rows: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(rows[0]["userId"], "u1");
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().kv.data,
        core.state().kv.data
    );
}

#[test]
fn relational_db_is_available_inside_host_run_resource_context() {
    let dir = tempdir().unwrap();
    let bundle = dir.path().join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"crm","name":"CRM","runtime":"js","backend":"main.js","resources":["relational_db"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        format!(
            r#"
function handle(input) {{
  const spec = {spec};
  ctx.resource.relational_db.defineTable("users", JSON.stringify(spec));
  ctx.resource.relational_db.put("users", JSON.stringify({{
    tenantId: "acme", userId: "u1", email: "ada@example.com", name: "Ada", createdAt: "2026-06-28T00:00:00Z"
  }}));
  return ctx.resource.relational_db.query("users", "byEmail", JSON.stringify({{
    partition: {{ tenantId: "acme", email: "ada@example.com" }}, limit: 1
  }}));
}}
"#,
            spec = users_spec()
        ),
    )
    .unwrap();

    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &[
            "crm",
            "CRM",
            "--source",
            bundle.to_str().expect("utf-8 path"),
        ],
    ))
    .unwrap();
    core.dispatch(req("js-runtime.run", &["crm", "seed"]))
        .unwrap();
    let output = core.take_last_output().unwrap();
    let rows: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(rows[0]["email"], "ada@example.com");
    assert!(core.replay_matches().unwrap());
}
