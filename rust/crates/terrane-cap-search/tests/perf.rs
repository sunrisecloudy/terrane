//! Performance characterization for the v1 in-memory brute-force search engine.
//!
//! Every query scans and recomputes over the whole corpus, so latency grows
//! linearly with document count and embedding dimension. These benches quantify
//! that (concrete numbers for the "O(N), won't scale" claim) and act as a
//! coarse regression guardrail — they are `#[ignore]`d so the default gate stays
//! fast. Run them with:
//!
//! ```sh
//! cargo test -p terrane-cap-search --test perf -- --ignored --nocapture
//! ```

use std::any::Any;
use std::time::{Duration, Instant};

use terrane_cap_interface::{
    CapBus, Capability, CommandCtx, Decision, QueryValue, ReadValue, ResourceReadCtx, StateStore,
};
use terrane_cap_kv::{KvCapability, KvState};
use terrane_cap_search::SearchCapability;

#[derive(Default)]
struct TestState {
    kv: KvState,
}

impl StateStore for TestState {
    fn get(&self, namespace: &str) -> Option<&dyn Any> {
        (namespace == "kv").then_some(&self.kv as &dyn Any)
    }
    fn get_mut(&mut self, namespace: &str) -> Option<&mut dyn Any> {
        (namespace == "kv").then_some(&mut self.kv as &mut dyn Any)
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

fn dispatch(state: &mut TestState, name: &str, args: Vec<String>) {
    let bus = Bus;
    let ctx = CommandCtx { state, bus: &bus };
    let Decision::Commit(records) = SearchCapability.decide(ctx, name, &args).unwrap() else {
        panic!("expected commit");
    };
    for record in records {
        KvCapability.fold(state, &record).unwrap();
    }
}

fn read(state: &TestState, method: &str, args: Vec<String>) -> String {
    let bus = Bus;
    let ctx = ResourceReadCtx {
        state,
        bus: &bus,
        app: "notes",
        host: None,
    };
    let ReadValue::OptString(Some(raw)) =
        SearchCapability.read_resource(ctx, method, &args).unwrap()
    else {
        panic!("expected {method} to return a value");
    };
    raw
}

/// A deterministic pseudo-embedding as a JSON array string (no RNG, so runs are
/// reproducible). Values in [0, 1) derived from the doc seed and dimension.
fn embedding_json(seed: usize, dim: usize) -> String {
    let mut out = String::with_capacity(dim * 6 + 2);
    out.push('[');
    for j in 0..dim {
        if j > 0 {
            out.push(',');
        }
        let v = ((seed.wrapping_mul(31).wrapping_add(j.wrapping_mul(7))) % 100) as f32 / 100.0;
        out.push_str(&format!("{v}"));
    }
    out.push(']');
    out
}

/// A short document body that mixes shared terms (so BM25 matches) with a
/// per-doc term (so document frequencies vary).
fn document_text(seed: usize) -> String {
    format!(
        "note {seed} the quick brown fox jumps over lazy dogs cats and animals in field {}",
        seed % 97
    )
}

fn build_corpus(n_docs: usize, dim: usize) -> (TestState, Duration) {
    let mut state = TestState::default();
    let started = Instant::now();
    for i in 0..n_docs {
        let doc_id = format!("doc-{i}");
        dispatch(
            &mut state,
            "search.upsert",
            vec!["notes".into(), doc_id.clone(), document_text(i)],
        );
        dispatch(
            &mut state,
            "search.setEmbedding",
            vec!["notes".into(), doc_id, embedding_json(i, dim)],
        );
    }
    (state, started.elapsed())
}

fn time_query(state: &TestState, method: &str, args: Vec<String>, iters: u32) -> Duration {
    // One warm-up pass, then measure.
    let _ = read(state, method, args.clone());
    let started = Instant::now();
    for _ in 0..iters {
        let hits = read(state, method, args.clone());
        assert!(hits.starts_with('['), "expected a JSON hit list");
    }
    started.elapsed() / iters
}

#[test]
#[ignore = "perf benchmark; run with `cargo test -p terrane-cap-search --test perf -- --ignored --nocapture`"]
fn query_latency_scales_with_corpus_size() {
    const DIM: usize = 768; // nomic-embed-text-v1.5 native dimension
    println!("\n== search v1 (brute-force) query latency, dim={DIM} ==");
    for &n in &[100usize, 1_000, 5_000, 10_000] {
        let (state, build) = build_corpus(n, DIM);
        let iters = if n <= 1_000 { 50 } else { 10 };

        let qvec = embedding_json(3, DIM);
        let hybrid_opts = format!(r#"{{"limit":10,"queryVec":{qvec}}}"#);

        let bm25 = time_query(
            &state,
            "bm25",
            vec!["quick brown fox".into(), r#"{"limit":10}"#.into()],
            iters,
        );
        let vector = time_query(
            &state,
            "vectorSearch",
            vec![qvec.clone(), r#"{"limit":10}"#.into()],
            iters,
        );
        let hybrid = time_query(
            &state,
            "query",
            vec!["quick brown fox".into(), hybrid_opts],
            iters,
        );

        println!(
            "n={n:>6}  build={:>7.1?}  bm25={:>8.3?}  vector={:>8.3?}  hybrid={:>8.3?}",
            build, bm25, vector, hybrid
        );

        // Coarse guardrail: catch a gross (e.g. accidental O(N^2)) regression
        // without being flaky across machines. Real signal is the printed numbers.
        assert!(
            hybrid < Duration::from_secs(3),
            "hybrid query over {n} docs took {hybrid:?} — unexpectedly slow"
        );
    }
    println!();
}
