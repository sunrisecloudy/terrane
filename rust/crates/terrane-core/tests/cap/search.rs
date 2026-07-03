//! Engine tests for the KV-backed `search` capability.

use std::fs;

use terrane_cap_interface::{Capability, ReadValue, ResourceReadCtx};
use terrane_core::Core;

use crate::helpers::{grant_resource, req};

struct ReadBus;
impl terrane_cap_interface::CapBus for ReadBus {
    fn query(
        &self,
        _cap: &str,
        _name: &str,
        _args: &[String],
    ) -> terrane_core::Result<terrane_cap_interface::QueryValue> {
        unreachable!("search resource reads do not need the bus")
    }
}

#[test]
fn search_dispatches_to_kv_events_and_replays() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req(
        "search.upsert",
        &["notes", "doc-1", "the quick brown fox"],
    ))
    .unwrap();
    core.dispatch(req(
        "search.setEmbedding",
        &["notes", "doc-1", "[1.0,0.0,0.5]"],
    ))
    .unwrap();

    let app_kv = &core.state().kv.data["notes"];
    assert!(app_kv
        .keys()
        .any(|k| k.starts_with("__terrane/search/v1/doc/")));
    assert!(app_kv
        .keys()
        .any(|k| k.starts_with("__terrane/search/v1/embeddings/")));

    let bus = ReadBus;
    let ctx = ResourceReadCtx {
        state: core.state(),
        bus: &bus,
        app: "notes",
    };
    let ReadValue::OptString(Some(raw)) = terrane_cap_search::SearchCapability
        .read_resource(
            ctx,
            "query",
            &[
                "fox".into(),
                r#"{"limit":5,"queryVec":[1.0,0.0,0.5]}"#.into(),
            ],
        )
        .unwrap()
    else {
        panic!("query did not return hits");
    };
    let hits: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(hits[0]["docId"], "doc-1");
    assert!(core.replay_matches().unwrap());
    assert_eq!(
        Core::open(&log).unwrap().state().kv.data,
        core.state().kv.data
    );
}

#[test]
fn search_cascade_on_app_removed() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req("app.add", &["notes", "Notes"])).unwrap();
    core.dispatch(req(
        "search.upsert",
        &["notes", "doc-1", "hello world"],
    ))
    .unwrap();
    core.dispatch(req("app.remove", &["notes"])).unwrap();
    assert!(!core.state().kv.data.contains_key("notes"));
    assert!(core.replay_matches().unwrap());
}

#[test]
fn search_is_available_inside_host_run_resource_context() {
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"notes","name":"Notes","runtime":"js","backend":"main.js","resources":["search"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
function handle(input) {
  ctx.resource.search.upsert("doc-1", "the quick brown fox");
  ctx.resource.search.setEmbedding("doc-1", JSON.stringify([1.0, 0.0, 0.5]));
  return ctx.resource.search.query("fox", JSON.stringify({ limit: 5, queryVec: [1.0, 0.0, 0.5] }));
}
"#,
    )
    .unwrap();

    let log = dir.path().join("log.bin");
    let mut core = Core::open(&log).unwrap();
    core.dispatch(req(
        "app.add",
        &[
            "notes",
            "Notes",
            "--source",
            bundle.to_str().expect("utf-8 path"),
        ],
    ))
    .unwrap();
    grant_resource(&mut core, "notes", "search");
    core.dispatch(req("js-runtime.run", &["notes", "seed"]))
        .unwrap();
    let output = core.take_last_output().unwrap();
    let hits: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(hits[0]["docId"], "doc-1");
    assert!(core.replay_matches().unwrap());
}