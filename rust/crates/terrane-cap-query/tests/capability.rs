use serde_json::{json, Value};
use terrane_cap_query::{jmespath, pipeline};

#[test]
fn jmespath_evaluates_json_documents() {
    let value = json!({"orders":[{"day":"2026-07-06","total":10},{"day":"2026-07-07","total":20}]});
    assert_eq!(
        jmespath::eval("orders[0].day", &value).unwrap(),
        "\"2026-07-06\""
    );
}

#[test]
fn pipeline_groups_sorts_and_projects_deterministically() {
    let docs = vec![
        json!({"day":"2026-07-07","total":5}),
        json!({"day":"2026-07-06","total":10}),
        json!({"day":"2026-07-06","total":7}),
    ];
    let pipeline = vec![
        json!({"$group":{"_id":"$day","total":{"$sum":"$total"},"count":{"$count":{}}}}),
        json!({"$sort":{"_id":1}}),
        json!({"$project":{"day":"$_id","total":1,"count":1,"_id":0}}),
    ];
    let mut lookup = |_source: &Value| Ok(Vec::new());
    let rows = pipeline::execute_pipeline(docs, &pipeline, &mut lookup).unwrap();
    assert_eq!(
        rows,
        vec![
            json!({"count":2,"day":"2026-07-06","total":17}),
            json!({"count":1,"day":"2026-07-07","total":5}),
        ]
    );
}

#[test]
fn unsupported_stage_is_a_typed_error() {
    let mut lookup = |_source: &Value| Ok(Vec::new());
    let err = pipeline::execute_pipeline(vec![json!({})], &[json!({"$facet":{}})], &mut lookup)
        .unwrap_err()
        .to_string();
    assert!(err.contains("unsupported query pipeline stage $facet"));
}
