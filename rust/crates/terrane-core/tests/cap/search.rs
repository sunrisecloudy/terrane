//! Engine tests for the KV-backed `search` capability.

use std::fs;

use terrane_cap_interface::{Capability, ReadValue, ResourceReadCtx};
use terrane_core::{Core, Effect, EffectRunner, Error, EventRecord, Result, State};

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
        host: None,
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

/// A stub for the embedding effect so the full embed → setEmbedding → query flow
/// can run deterministically without a real model.
struct StubEmbed;
impl EffectRunner for StubEmbed {
    fn run(&self, effect: &Effect, _state: &State) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::LocalModelEmbed {
                app,
                model,
                texts,
                query,
            } => Ok(vec![terrane_cap_local_model::embedded_event(
                &terrane_cap_local_model::EmbeddedRecord {
                    app: app.clone(),
                    model: model.clone(),
                    query: *query,
                    dim: 3,
                    vectors: texts.iter().map(|_| vec![0.5, 0.25, 0.125]).collect(),
                    duration_ms: 1,
                },
            )
            .map_err(|e| Error::Storage(e.to_string()))?]),
            other => Err(Error::InvalidInput(format!(
                "stub runner cannot perform {other:?}"
            ))),
        }
    }
}

#[test]
fn embed_then_index_then_query_end_to_end() {
    // The headline product flow, in one app-backend run: embed a document with
    // local-model, store the vector via search.setEmbedding, and hybrid-query —
    // proving the two capabilities compose and replay without re-embedding.
    let dir = tempfile::tempdir().unwrap();
    let bundle = dir.path().join("bundle");
    fs::create_dir(&bundle).unwrap();
    fs::write(
        bundle.join("manifest.json"),
        r#"{"id":"notes","name":"Notes","runtime":"js","backend":"main.js","resources":["search","local-model"]}"#,
    )
    .unwrap();
    fs::write(
        bundle.join("main.js"),
        r#"
function handle(input) {
  ctx.resource.search.upsert("doc-1", "the quick brown fox");
  var v = JSON.parse(ctx.resource["local-model"].embed("the quick brown fox"));
  ctx.resource.search.setEmbedding("doc-1", JSON.stringify(v));
  return ctx.resource.search.query("fox", JSON.stringify({ limit: 5, queryVec: v }));
}
"#,
    )
    .unwrap();

    let log = dir.path().join("log.bin");
    let mut core = Core::open_with(&log, StubEmbed).unwrap();
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
    // An embedding model must exist so local-model.embed resolves a default.
    core.dispatch(req(
        "local-model.register",
        &["nomic", "llama_cpp", "/models/nomic.gguf", "--embed"],
    ))
    .unwrap();
    grant_resource(&mut core, "notes", "search");
    grant_resource(&mut core, "notes", "local-model");

    core.dispatch(req("js-runtime.run", &["notes", "go"])).unwrap();
    let output = core.take_last_output().unwrap();
    let hits: serde_json::Value = serde_json::from_str(&output).unwrap();
    assert_eq!(hits[0]["docId"], "doc-1");
    // Both halves contributed: BM25 matched "fox", the vector matched the stored
    // embedding.
    assert!(hits[0]["ftsRank"].is_number(), "bm25 should rank: {output}");
    assert!(hits[0]["vecRank"].is_number(), "vector should rank: {output}");

    // Replay rebuilds the projection from kv.* events without re-running JS or
    // re-embedding (the local-model.embedded event folds to a no-op).
    assert!(core.replay_matches().unwrap());
}