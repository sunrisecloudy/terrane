use std::any::Any;

use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, EventRecord, QueryValue, ReadValue, ResourceReadCtx,
    StateStore,
};
use terrane_cap_kv::{KvCapability, KvState};

use crate::key::{doc_key, SEARCH_PREFIX};
use crate::query::rrf_score;
use crate::SearchCapability;

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
            _ => unreachable!("{cap}.{name}"),
        }
    }
}

fn apply(state: &mut TestState, records: Vec<EventRecord>) {
    for record in records {
        KvCapability.fold(state, &record).unwrap();
    }
}

fn dispatch(state: &mut TestState, name: &str, args: Vec<String>) {
    let bus = Bus;
    let ctx = CommandCtx { state, bus: &bus };
    let Decision::Commit(records) = SearchCapability.decide(ctx, name, &args).unwrap() else {
        panic!("expected commit");
    };
    apply(state, records);
}

fn read(state: &TestState, method: &str, args: Vec<String>) -> ReadValue {
    let bus = Bus;
    let ctx = ResourceReadCtx {
        state,
        bus: &bus,
        app: "notes",
    };
    SearchCapability
        .read_resource(ctx, method, &args)
        .unwrap()
}

#[test]
fn rrf_fuses_ranks_with_weights() {
    let score = rrf_score(Some(1), Some(2), 1.0, 1.0, 60.0);
    let expected = 1.0 / 61.0 + 1.0 / 62.0;
    assert!((score - expected).abs() < 1e-9);
    assert_eq!(rrf_score(None, Some(1), 1.0, 1.0, 60.0), 1.0 / 61.0);
}

#[test]
fn upsert_emits_kv_set_for_reserved_doc_key() {
    let mut state = TestState::default();
    dispatch(
        &mut state,
        "search.upsert",
        vec![
            "notes".into(),
            "doc-1".into(),
            "the quick brown fox".into(),
        ],
    );
    let app_kv = state.kv.data.get("notes").unwrap();
    let key = doc_key("doc-1").unwrap();
    assert!(app_kv.contains_key(&key));
    assert!(key.starts_with(SEARCH_PREFIX));
}

#[test]
fn hybrid_query_returns_bm25_hit_without_vector() {
    let mut state = TestState::default();
    dispatch(
        &mut state,
        "search.upsert",
        vec!["notes".into(), "doc-1".into(), "the quick brown fox".into()],
    );
    dispatch(
        &mut state,
        "search.upsert",
        vec!["notes".into(), "doc-2".into(), "lazy dog sleeps".into()],
    );
    let ReadValue::OptString(Some(raw)) =
        read(&state, "query", vec!["fox".into(), "".into()])
    else {
        panic!("expected query hits");
    };
    let hits: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(hits[0]["docId"], "doc-1");
}

#[test]
fn bm25_penalizes_longer_documents() {
    // Both docs contain the query term exactly once; BM25 length normalization
    // must rank the shorter one first. Without it the two scores tie and doc-1
    // (scanned first, sorts first) would win — so this guards the avg_len fix.
    let mut state = TestState::default();
    dispatch(
        &mut state,
        "search.upsert",
        vec![
            "notes".into(),
            "doc-1".into(),
            "fox alpha beta gamma delta epsilon zeta eta theta iota".into(),
        ],
    );
    dispatch(
        &mut state,
        "search.upsert",
        vec!["notes".into(), "doc-2".into(), "fox".into()],
    );
    let ReadValue::OptString(Some(raw)) = read(&state, "bm25", vec!["fox".into(), "".into()]) else {
        panic!("expected bm25 hits");
    };
    let hits: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        hits[0]["docId"], "doc-2",
        "shorter document should rank first: {raw}"
    );
}

#[test]
fn set_embedding_requires_indexed_document() {
    let state = TestState::default();
    let bus = Bus;
    let ctx = CommandCtx { state: &state, bus: &bus };
    let err = SearchCapability
        .decide(
            ctx,
            "search.setEmbedding",
            &["notes".into(), "missing".into(), "[0.1,0.2]".into()],
        )
        .unwrap_err()
        .to_string();
    assert!(err.contains("not indexed"));
}

#[test]
fn search_doc_declares_namespace() {
    let doc = SearchCapability.doc(true);
    assert_eq!(doc.namespace, "search");
    assert!(doc
        .manifest
        .commands
        .iter()
        .any(|name| name == "search.upsert"));
}