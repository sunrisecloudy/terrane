//! Engine tests for the `query` capability.

use serde_json::{json, Value};
use tempfile::tempdir;
use terrane_cap_interface::{CapBus, Capability, QueryValue, ReadValue, ResourceReadCtx};
use terrane_core::{read_log, Core, Result};

use crate::helpers::req;

struct ReadBus;
impl CapBus for ReadBus {
    fn query(&self, _cap: &str, _name: &str, _args: &[String]) -> Result<QueryValue> {
        unreachable!("query resource reads in these tests do not need the bus")
    }
}

fn read_query_resource(core: &Core, app: &str, method: &str, args: &[String]) -> String {
    let bus = ReadBus;
    let ctx = ResourceReadCtx {
        state: core.state(),
        bus: &bus,
        app,
        host: None,
    };
    let ReadValue::OptString(Some(raw)) = terrane_cap_query::QueryCapability
        .read_resource(ctx, method, args)
        .unwrap()
    else {
        panic!("query resource did not return JSON")
    };
    raw
}

fn order(day: &str, sku: &str, total: i64) -> String {
    json!({"day":day,"sku":sku,"total":total}).to_string()
}

fn daily_view_definition() -> String {
    json!({
        "source": {"kv": {"prefix": "orders/"}},
        "pipeline": [
            {"$group": {"_id": "$day", "total": {"$sum": "$total"}, "count": {"$count": {}}}},
            {"$sort": {"_id": 1}}
        ],
        "key": "_id"
    })
    .to_string()
}

#[test]
fn define_materialize_and_read_view_round_trip_replays() {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    core.dispatch(req(
        "kv.set",
        &["shop", "orders/1", &order("2026-07-06", "sku-1", 10)],
    ))
    .unwrap();
    core.dispatch(req(
        "kv.set",
        &["shop", "orders/2", &order("2026-07-06", "sku-2", 7)],
    ))
    .unwrap();
    core.dispatch(req(
        "query.view.define",
        &["shop", "daily", &daily_view_definition()],
    ))
    .unwrap();
    core.dispatch(req("query.materialize", &["shop", "daily"]))
        .unwrap();

    let raw = read_query_resource(
        &core,
        "shop",
        "viewGet",
        &["daily".into(), "2026-07-06".into()],
    );
    let row: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(row["total"], 17);
    assert_eq!(row["count"], 2);

    let stat: Value = serde_json::from_str(&read_query_resource(
        &core,
        "shop",
        "viewStat",
        &["daily".into()],
    ))
    .unwrap();
    assert_eq!(stat["rowCount"], 1);
    assert!(stat["defHash"].as_str().is_some_and(|s| s.len() == 64));
    assert!(stat["sourceCursor"].as_u64().unwrap() > 0);
    assert!(core.replay_matches().unwrap());
    assert_eq!(Core::open(&log).unwrap().state().query, core.state().query);
}

#[test]
fn rematerialize_replaces_snapshot_rows() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    core.dispatch(req(
        "kv.set",
        &["shop", "orders/1", &order("2026-07-06", "sku-1", 10)],
    ))
    .unwrap();
    core.dispatch(req(
        "query.view.define",
        &["shop", "daily", &daily_view_definition()],
    ))
    .unwrap();
    core.dispatch(req("query.materialize", &["shop", "daily"]))
        .unwrap();
    core.dispatch(req(
        "kv.set",
        &["shop", "orders/2", &order("2026-07-07", "sku-2", 3)],
    ))
    .unwrap();
    core.dispatch(req("query.materialize", &["shop", "daily"]))
        .unwrap();
    let rows: Value = serde_json::from_str(&read_query_resource(
        &core,
        "shop",
        "viewScan",
        &["daily".into(), "".into(), "10".into()],
    ))
    .unwrap();
    assert_eq!(rows.as_array().unwrap().len(), 2);
}

#[test]
fn lookup_joins_kv_orders_to_relational_rows() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    core.dispatch(req(
        "relational_db.defineTable",
        &[
            "shop",
            "products",
            &json!({
                "specVersion": 1,
                "schemaVersion": 1,
                "fields": {
                    "sku": {"type":"string","required":true},
                    "name": {"type":"string","required":true}
                },
                "primaryKey": {"partition": ["sku"], "sort": []},
                "options": {"unknownFields":"reject"}
            })
            .to_string(),
        ],
    ))
    .unwrap();
    core.dispatch(req(
        "relational_db.put",
        &[
            "shop",
            "products",
            &json!({"sku":"sku-1","name":"Pen"}).to_string(),
        ],
    ))
    .unwrap();
    core.dispatch(req(
        "kv.set",
        &["shop", "orders/1", &order("2026-07-06", "sku-1", 10)],
    ))
    .unwrap();

    let source = json!({"kv":{"prefix":"orders/"}}).to_string();
    let pipeline = json!([
        {"$lookup": {
            "from": {"table": {"name": "products", "query": {"partition": {"sku": "sku-1"}}}},
            "localField": "sku",
            "foreignField": "sku",
            "as": "product"
        }},
        {"$unwind": "$product"},
        {"$project": {"sku": 1, "productName": "$product.name", "_id": 0}}
    ])
    .to_string();
    let rows: Value = serde_json::from_str(&read_query_resource(
        &core,
        "shop",
        "pipeline",
        &[source, pipeline],
    ))
    .unwrap();
    assert_eq!(rows[0]["productName"], "Pen");
}

#[test]
fn view_source_composes_with_jmespath_query() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    core.dispatch(req(
        "kv.set",
        &["shop", "orders/1", &order("2026-07-06", "sku-1", 10)],
    ))
    .unwrap();
    core.dispatch(req(
        "query.view.define",
        &["shop", "daily", &daily_view_definition()],
    ))
    .unwrap();
    core.dispatch(req("query.materialize", &["shop", "daily"]))
        .unwrap();
    let value = core
        .query(
            "query",
            "jmespath",
            &[
                "shop".into(),
                json!({"view":{"name":"daily"}}).to_string(),
                "[0].total".into(),
            ],
        )
        .unwrap();
    assert_eq!(value, QueryValue::Json("10".into()));
}

#[test]
fn shuffled_kv_insertion_produces_identical_materialized_events() {
    let a = materialized_log_for(&[("orders/1", 10), ("orders/2", 7)]);
    let b = materialized_log_for(&[("orders/2", 7), ("orders/1", 10)]);
    assert_eq!(a, b);
}

fn materialized_log_for(entries: &[(&str, i64)]) -> Vec<terrane_core::EventRecord> {
    let dir = tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    for (key, total) in entries {
        core.dispatch(req(
            "kv.set",
            &["shop", key, &order("2026-07-06", "sku", *total)],
        ))
        .unwrap();
    }
    core.dispatch(req(
        "query.view.define",
        &["shop", "daily", &daily_view_definition()],
    ))
    .unwrap();
    core.dispatch(req("query.materialize", &["shop", "daily"]))
        .unwrap();
    read_log(&log)
        .unwrap()
        .into_iter()
        .filter(|record| record.kind.starts_with("query."))
        .collect()
}

#[test]
fn limit_errors_are_typed_and_named() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    let pipeline = (0..33).map(|_| json!({"$match": {}})).collect::<Vec<_>>();
    let err = read_query_resource_result(
        &core,
        "shop",
        "pipeline",
        &[
            json!({"docs":[{}]}).to_string(),
            serde_json::to_string(&pipeline).unwrap(),
        ],
    )
    .unwrap_err()
    .to_string();
    assert!(err.contains("limit is 32"));
}

fn read_query_resource_result(
    core: &Core,
    app: &str,
    method: &str,
    args: &[String],
) -> terrane_core::Result<String> {
    let bus = ReadBus;
    let ctx = ResourceReadCtx {
        state: core.state(),
        bus: &bus,
        app,
        host: None,
    };
    match terrane_cap_query::QueryCapability.read_resource(ctx, method, args)? {
        ReadValue::OptString(Some(raw)) => Ok(raw),
        other => Err(terrane_core::Error::Runtime(format!(
            "unexpected query read value: {other:?}"
        ))),
    }
}

#[test]
fn app_removed_drops_query_views() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["shop", "Shop"])).unwrap();
    core.dispatch(req(
        "query.view.define",
        &["shop", "daily", &daily_view_definition()],
    ))
    .unwrap();
    core.dispatch(req("app.remove", &["shop"])).unwrap();
    assert!(!core.state().query.views.contains_key("shop"));
}
