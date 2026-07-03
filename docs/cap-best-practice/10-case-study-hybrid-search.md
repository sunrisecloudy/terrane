# 10 — Case study: hybrid search

A worked application of this guide. **Hybrid search** = keyword/BM25 full-text
(exact terms, rare words, IDs) fused with dense-vector/semantic recall (meaning,
paraphrase), combined by Reciprocal Rank Fusion, optionally reranked. The one
reframe that makes it fit Terrane: **the search index is not state.** BM25 corpus
stats, an ANN graph, and float scores are a *rebuildable derived read-model*, not
replay-critical `State`. Replay identity is checked only over the `State` struct
([02-contract.md](02-contract.md)); anything computed off to the side — like the
physical SQLite backend `terrane-cap-kv` already materializes beside the log — is
exempt by construction. So this is the **projection-over-KV** shape
([01-design.md](01-design.md)), mirroring `terrane-cap-relational-db`: no own
events, empty `fold`, all durable data under reserved KV keys.

```
documents (recorded as kv.* events, source of truth)
        │
        ├── fold ─▶ State (checked by replay_matches)   ← index is NOT here
        │
        └── materialize at the EDGE ─▶ rebuildable index (FTS5 + vec table)
                                            │
                          ctx.resource.search.query ─▶ RRF-fused hits
```

The document text and the *inputs that decide what gets indexed* (add/remove,
embedding model id + dimension, tokenizer config) enter the log as events. The
BM25 stats, the vectors, and the scores never do — they are recomputed from the
logged text on rebuild.

## Shipped implementation (v1): in-memory recompute

`terrane-cap-search` ships the *simplest* thing that satisfies the reframe: **no
index library at all.** Documents, per-model embeddings, and config live under
`__terrane/search/v1/` reserved KV keys; every query `scan`s that prefix and
recomputes BM25 (Okapi, `k1=1.2`, `b=0.75`, corpus-average length normalization)
and cosine similarity in Rust, then fuses by RRF. `fold` is empty; the "index"
*is* the KV data, so it is trivially rebuildable and replay-safe — there is no
derived artifact to keep in sync.

This is the right v1 for desktop-scale corpora: zero new dependencies,
deterministic, easy to reason about. It holds up further than you'd expect —
a hybrid query stays under ~8 ms over 10k docs (release; see
`terrane-cap-search/tests/perf.rs`). Its limits are exactly why the library
table below exists, and why you graduate rather than tune:

- **O(N) per query** — every query loads and re-tokenizes every document.
- **ASCII tokenizer** — alphanumeric word-splitting + lowercasing; no stemming,
  no CJK/accent handling, no phrase/fuzzy queries.
- **Exact KNN only** — brute-force cosine over all embedded docs; no ANN.

Graduate to the stack below when the corpus outgrows a full scan or needs real
tokenization/ANN. The migration is contained: the KV projection (keys, events,
resource surface) is unchanged — only the query engine behind `read_resource`
swaps out.

## What library — the scale-up path

When you outgrow v1, judge the options against Terrane's constraints (embedded,
no server, deterministic replay, Apple-Silicon-primary, "add a dep only when
forced"):

| Option | Embedded | BM25 | Vector/ANN | RRF | Reuses deps | Replay fit | Build cost |
|---|---|---|---|---|---|---|---|
| **SQLite FTS5 + sqlite-vec** | yes (in-process) | yes (FTS5 `rank`) | yes (brute-force KNN; ANN alpha-only) | pure SQL | **FTS5 already bundled; sqlite-vec static-links into it** | ideal — index in shadow tables, drop & rebuild | low (one C compile) |
| **Tantivy (+ companion ANN)** | yes | yes (strong, Lucene-grade) | **no native** — needs `hnsw_rs`/`usearch`/brute-force alongside | you write ~30 lines | new pure-Rust engine (2nd store next to SQLite) | good — segments are derived, rebuildable | medium (pure Rust, no JVM) |
| **LanceDB** | yes | yes (native) | yes (IVF/HNSW) | built-in RRF | **no** — pulls Arrow + DataFusion + object_store + candle + **mandatory Tokio** (~2.5M SLoC) | fights its own MVCC/versioning to stay a pure read-model | high (protoc/cmake, long compiles) |
| **Server DBs — Qdrant / Weaviate / OpenSearch / Milvus** | **no** — external process | yes | yes | yes | no | **violates no-server / no-external-DB rule outright** | n/a — disqualified |

**Recommended scale-up: SQLite FTS5 + sqlite-vec.** `rusqlite = "0.40.1"` with
`bundled` is *already* a dependency, and FTS5 ships in that amalgamation, so
keyword search costs zero new deps. `sqlite-vec` is not a runtime extension here: the crate
embeds its C source and static-links via `cc`, registered once with
`register_auto_extension` before opening the connection — it links against the
same bundled SQLite, single binary, no `.dylib`. Everything (vectors, BM25 stats,
any ANN graph) lives in ordinary SQLite shadow tables inside `terrane.db`, so the
index is exactly the KV-projection archetype `terrane-cap-relational-db` already
proves.

Two honest caveats from the verdicts:

- **sqlite-vec is 0.1.x, not "past v0.1".** Pin stable **v0.1.9** and use
  **brute-force KNN** (exact, deterministic, fine at desktop scale, especially
  with int8/binary quantization). ANN indexes (IVF/DiskANN) are alpha-only, have
  a costly DELETE bug, and carry upstream bus-factor risk — treat as future-only.
- The published example predates the rusqlite 0.34+ auto-extension API; on 0.40.1
  use `rusqlite::auto_extension::register_auto_extension`, not the old
  `sqlite3_auto_extension(transmute(...))` snippet.

**Strong alternative: Tantivy** for the BM25 half — reach for it only when FTS5's
tokenizer/relevance/faceting isn't enough. Note the verdict: **Tantivy has no
native vector search** (issue #815 open since 2020); "hybrid on Tantivy" always
means Tantivy BM25 + a *separate* ANN store fused with RRF. It's a second storage
engine next to SQLite, so it's an upgrade, not the starting point. **LanceDB is
niche** — only if you genuinely need disk-based ANN over millions of vectors; its
weight and self-versioning persistence are the opposite of "start small."

## Embeddings

The vector half needs an encoder. `terrane-cap-local-model` does **not** expose
embeddings today — that is the real gap, not the index.

| Source | Notes |
|---|---|
| **Extend `terrane-cap-local-model` (llama-cpp-2)** | Best-integrated: reuses the vendored llama.cpp + Metal plumbing. `LlamaContext::embeddings_seq_ith` yields pooled vectors from GGUF models (nomic-embed, bge-m3). Needs a new `Effect::LocalModelEmbed` + `EdgeRunner` arm. |
| **Model2Vec / potion (`model2vec-rs`)** | Pure-Rust static token→vector lookup, no NN forward pass, ~30MB, offline, and the **one deterministic** family (integer lookup + averaging) — the only encoder that could legitimately run inside replay. Lower quality (~80% of MiniLM). Great tiny default / fallback. |
| **fastembed-rs** | Self-contained ONNX + built-in reranker, but bundles ONNX Runtime and is non-deterministic. Alternative, not default. |
| **MLX** | ~50% faster on Apple Silicon but **Apple-only** — violates cross-desktop portability as a sole backend; fine as an optional accel path. |

**Determinism rule (non-negotiable).** Float NN inference is *not* bit-reproducible
across hardware (IEEE-754 non-associativity; vendor kernels reorder reductions), so
re-embedding the same text on another machine drifts. Therefore: **produce the
embedding as an edge effect and record its RESULT as an event** (under
`__terrane/search/v1/embeddings/{model}/{doc_hash}`), the way `net`/`model`
already record results. Replay reads the recorded vector — it never re-runs the
model — so replay is bit-identical regardless of hardware. **Never record raw
floats as replay-critical `State`, and never re-embed inside `fold`.**

**Recommended default:** llama-cpp-2 embeddings (reuse the existing model stack),
with Model2Vec as the deterministic, offline, tiny fallback.

## Shape & wiring

Map onto the guide's steps ([01-design.md](01-design.md) → [03](03-skeleton-and-wiring.md)):

- **Namespace:** `search`. Prefixes everything; registry rejects mismatches.
- **State:** none required — follow `relational-db`'s empty `fold` (returns
  `Ok(())`). Optionally a minimal `SearchState { last_rebuild: BTreeMap<AppId,…> }`
  for query-time hints; keep all durable data in KV.
- **Reserved KV prefix:** `__terrane/search/v1/` (a `SEARCH_PREFIX` const, mirroring
  `RDB_PREFIX = "__terrane/rdb/v1/"`). Subdivide per app for cascade:
  `__terrane/search/v1/app/{app}/doc/{doc_id}` (source text + config),
  `…/embeddings/{model}/{doc_hash}` (recorded vectors). Write only via
  `terrane_cap_kv::set_event` / `delete_event`; read via `scan_prefix` /
  `scan_range` / `get_value` — never touch KV directly (these are the exact helpers
  `relational-db` uses at `lib.rs:156`, `191`, `216`, `266`).
- **Events:** none of its own (`events: Vec::new()`). Indexing control commands
  emit ordinary `kv.*` events for document text; embeddings emit `kv.*` events
  carrying the recorded effect result. That is the whole log footprint.
- **Commands (write):** `search.index` / `search.upsert` (record doc text →
  `kv.set`; if a vector is needed, return `Decision::Effect(LocalModelEmbed)` per
  [05-effects-and-runtimes.md](05-effects-and-runtimes.md)) and `search.configure`
  (weights, model id, tokenizer).
- **Resources (read, on `ctx.resource.search`):** `query(text, limit, weights)`
  (hybrid), plus `bm25` / `vectorSearch` / `status`, declared as
  `ResourceMethod::Read` exactly like `relational-db`'s `get`/`query`/`tables`/`spec`
  (`resource_methods()` at `lib.rs:97`). Reads are read-only — they run one SQL
  query over the index; they never mutate.
- **Grants:** `GrantResourceSpec::namespace_v1("search", &["read", "write"], …)`,
  the same one-liner `relational-db` uses (`lib.rs:53`). Registry validation fails
  if resources exist without a matching grant spec.
- **What runs where:** indexing SQL and embedding inference run at the **edge**
  (during effect handling / post-commit `sync_storage_after_commit`), never in
  `fold`, `query`, or `read_resource`. RRF fusion runs at query time as pure SQL —
  deterministic given the two candidate lists.
- **Cascade:** subscribe to `app.removed` and emit
  `kv::delete_prefix_events` for `__terrane/search/v1/app/{app}/…` — the mandatory
  cleanup pattern ([04-cross-capability.md](04-cross-capability.md)).
- **Classify commands** in `public_authz.rs`: `search.query`/`index`/`upsert` →
  `GrantGated { "search", 0 }`; `search.rebuild-all` → `Refuse` (TrustedHost/startup
  only) ([06-permissions-and-policy.md](06-permissions-and-policy.md)).
- **Front door:** `ctx.resource.search` for app backends, plus an MCP tool for
  agents ([08-public-surface-and-release.md](08-public-surface-and-release.md)).

## RRF fusion

Reciprocal Rank Fusion needs no score normalization — it fuses *ranks*:

```
score(doc) = Σ  weight_src / (k + rank_src(doc))      k ≈ 60
           src
```

Because both halves live in the same SQLite DB, fusion is one query — no
app-side merge:

```sql
WITH fts AS (          -- FTS5 BM25, best rank = 1
  SELECT doc_id, row_number() OVER (ORDER BY rank) AS r
  FROM docs_fts WHERE docs_fts MATCH :q LIMIT :n),
vec AS (              -- sqlite-vec KNN by distance
  SELECT doc_id, row_number() OVER (ORDER BY distance) AS r
  FROM docs_vec WHERE embedding MATCH :qvec AND k = :n)
SELECT doc_id,
       coalesce(:w_fts/(:k + fts.r), 0)
     + coalesce(:w_vec/(:k + vec.r), 0) AS score
FROM fts FULL OUTER JOIN vec USING (doc_id)
ORDER BY score DESC LIMIT :limit;
```

**Optional local reranking.** Fuse first, then rerank only the top-K (e.g. 20)
when precision-at-1 matters and the corpus is diverse enough that RRF ordering is
noisy. Route top-K text through `local-model` with a rerank schema; that call is
an effect and its result *is* recorded (distinct from indexing). Skip it for
small/high-precision corpora — it adds latency for little gain.

## Replay & testing

**Invariant:** the index is rebuildable from the log and lives *outside* `State`,
so `replay_matches()` still holds — replaying the recorded `kv.*` doc/embedding
events reconstructs identical `State`, and the FTS5 + vec tables are re-derived at
the edge from that state (the vector side is *functionally* rebuilt, not
bit-identical across hardware — acceptable precisely because it is a read-model,
not replay-critical state). Add all four layers ([07-testing.md](07-testing.md)):

1. **Unit** (`src/tests.rs`) — key encoding, query-JSON parsing, RRF math on
   fixed candidate lists.
2. **Capability** (`tests/capability.rs`) — `decide`/`read_resource` over a stub
   `StateStore`; assert `search.index` emits the expected `kv.*` events and
   nothing else.
3. **Engine** (`terrane-core/tests/cap/search.rs`) — dispatch docs, assert
   `core.replay_matches().unwrap()`, re-open the log for cold-start rebuild, and
   test the `app.removed` cascade empties the prefix.
4. **Binary e2e** (`terrane-host/tests/cap/search.rs`) — a default-run smoke over
   the real binary; mark the real-embedding path `#[ignore = "real embedding
   model; run with --ignored"]`.

Also: add `search` to `grant_spec_inventory.rs`, classify commands in
`public_authz.rs` tests, and expect the `APP_API.md` drift test to demand a
regenerate.

## Checklist

1. [ ] Shape = **projection over KV**; empty `fold`, no own events.
2. [ ] **SQLite FTS5 + sqlite-vec** (pin v0.1.9, brute-force KNN); reuse bundled
       rusqlite; static-link via `register_auto_extension`.
3. [ ] Reserved prefix `__terrane/search/v1/app/{app}/…`; write/read only via
       `terrane-cap-kv` helpers.
4. [ ] Embeddings via a **new `Effect`**; record the **result** as an event; never
       re-embed in `fold`; never store raw floats as `State`.
5. [ ] Resources `query`/`bm25`/`vectorSearch`/`status` + `namespace_v1("search",
       &["read","write"])`; classify commands in `public_authz.rs`.
6. [ ] RRF in one SQL query (`k≈60`, weighted); rerank top-K only when it earns it.
7. [ ] `app.removed` cascade; four test layers; `replay_matches()` green.
8. [ ] Gate green: `cargo test --workspace --locked` + clippy `-D warnings`.
