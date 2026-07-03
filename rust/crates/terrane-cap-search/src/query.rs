use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::json;
use terrane_cap_interface::{Error, Result, StateStore};

use crate::config::{effective_limit, parse_embedding, QueryOptions, SearchConfig};
use crate::document::{doc_id_from_key, parse_document};
use crate::key::{doc_prefix, embedding_prefix, SEARCH_PREFIX};

#[derive(Debug, Clone)]
struct IndexedDoc {
    doc_id: String,
    text: String,
    metadata: serde_json::Value,
    embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub doc_id: String,
    pub score: f64,
    pub text: String,
    pub metadata: serde_json::Value,
    pub fts_rank: Option<usize>,
    pub vec_rank: Option<usize>,
}

pub fn hybrid_query(
    state: &dyn StateStore,
    app: &str,
    config: &SearchConfig,
    query_text: &str,
    options: &QueryOptions,
) -> Result<String> {
    let limit = effective_limit(config, options);
    let docs = load_index(state, app, config)?;
    if docs.is_empty() {
        return Ok("[]".to_string());
    }

    let fts_weight = options.fts_weight.unwrap_or(config.fts_weight);
    let vec_weight = options.vec_weight.unwrap_or(config.vec_weight);
    let rrf_k = options.rrf_k.unwrap_or(config.rrf_k);

    let fts_ranks = bm25_ranks(&docs, query_text, limit);
    let vec_ranks = options
        .query_vec
        .as_ref()
        .map(|vector| vector_ranks(&docs, vector, limit))
        .unwrap_or_default();

    let hits = fuse_rrf(&docs, &fts_ranks, &vec_ranks, fts_weight, vec_weight, rrf_k, limit);
    serde_json::to_string(&hits_to_json(&hits))
        .map_err(|e| Error::Storage(format!("serialize search hits: {e}")))
}

pub fn bm25_query(
    state: &dyn StateStore,
    app: &str,
    config: &SearchConfig,
    query_text: &str,
    options: &QueryOptions,
) -> Result<String> {
    let limit = effective_limit(config, options);
    let docs = load_index(state, app, config)?;
    let ranks = bm25_ranks(&docs, query_text, limit);
    let hits = ranks
        .iter()
        .map(|(doc_id, rank)| {
            doc_hit(
                &docs,
                doc_id,
                1.0 / (config.rrf_k + *rank as f64),
                Some(*rank),
                None,
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&hits_to_json(&hits))
        .map_err(|e| Error::Storage(format!("serialize bm25 hits: {e}")))
}

pub fn vector_query(
    state: &dyn StateStore,
    app: &str,
    config: &SearchConfig,
    query_vec_json: &str,
    options: &QueryOptions,
) -> Result<String> {
    let limit = effective_limit(config, options);
    let query_vec = parse_embedding(query_vec_json)?;
    let docs = load_index(state, app, config)?;
    let ranks = vector_ranks(&docs, &query_vec, limit);
    let hits = ranks
        .iter()
        .map(|(doc_id, rank)| {
            doc_hit(
                &docs,
                doc_id,
                1.0 / (config.rrf_k + *rank as f64),
                None,
                Some(*rank),
            )
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&hits_to_json(&hits))
        .map_err(|e| Error::Storage(format!("serialize vector hits: {e}")))
}

pub fn status_json(state: &dyn StateStore, app: &str, config: &SearchConfig) -> Result<String> {
    let docs = load_index(state, app, config)?;
    let embedded = docs.iter().filter(|doc| doc.embedding.is_some()).count();
    let payload = json!({
        "prefix": SEARCH_PREFIX,
        "documentCount": docs.len(),
        "embeddedCount": embedded,
        "embedModel": config.embed_model,
        "ftsWeight": config.fts_weight,
        "vecWeight": config.vec_weight,
        "rrfK": config.rrf_k,
        "defaultLimit": config.default_limit,
    });
    serde_json::to_string(&payload).map_err(|e| Error::Storage(format!("serialize status: {e}")))
}

fn load_index(state: &dyn StateStore, app: &str, config: &SearchConfig) -> Result<Vec<IndexedDoc>> {
    let prefix = doc_prefix();
    let embed_prefix = embedding_prefix(&config.embed_model)?;
    let mut embeddings = BTreeMap::new();
    for (key, raw) in terrane_cap_kv::scan_prefix(state, app, &embed_prefix, usize::MAX)? {
        if let Some(doc_id) = key.strip_prefix(&embed_prefix) {
            embeddings.insert(doc_id.to_string(), parse_embedding(&raw)?);
        }
    }

    let mut docs = Vec::new();
    for (key, raw) in terrane_cap_kv::scan_prefix(state, app, &prefix, usize::MAX)? {
        let Some(doc_id) = doc_id_from_key(&key, &prefix) else {
            continue;
        };
        let doc = parse_document(&raw)?;
        docs.push(IndexedDoc {
            doc_id: doc_id.clone(),
            text: doc.text,
            metadata: doc.metadata,
            embedding: embeddings.remove(&doc_id),
        });
    }
    Ok(docs)
}

fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            current.push(ch.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

fn bm25_score(doc_tokens: &[String], query_terms: &[String], df: &HashMap<String, usize>, n_docs: usize) -> f64 {
    let mut score = 0.0;
    let doc_len = doc_tokens.len().max(1) as f64;
    let avg_len = doc_len;
    let k1 = 1.2;
    let b = 0.75;
    let mut term_freq = HashMap::new();
    for token in doc_tokens {
        *term_freq.entry(token.clone()).or_insert(0usize) += 1;
    }
    for term in query_terms {
        let tf = *term_freq.get(term).unwrap_or(&0) as f64;
        if tf == 0.0 {
            continue;
        }
        let df_term = *df.get(term).unwrap_or(&0) as f64;
        let idf = ((n_docs as f64 - df_term + 0.5) / (df_term + 0.5) + 1.0).ln();
        let numerator = tf * (k1 + 1.0);
        let denominator = tf + k1 * (1.0 - b + b * doc_len / avg_len);
        score += idf * numerator / denominator;
    }
    score
}

fn bm25_ranks(docs: &[IndexedDoc], query_text: &str, limit: usize) -> Vec<(String, usize)> {
    let query_terms = tokenize(query_text);
    if query_terms.is_empty() {
        return Vec::new();
    }
    let mut df = HashMap::new();
    let tokenized_docs: Vec<Vec<String>> = docs.iter().map(|doc| tokenize(&doc.text)).collect();
    for tokens in &tokenized_docs {
        let unique: HashSet<_> = tokens.iter().cloned().collect();
        for term in unique {
            *df.entry(term).or_insert(0) += 1;
        }
    }
    let mut scored: Vec<(String, f64)> = docs
        .iter()
        .zip(tokenized_docs.iter())
        .map(|(doc, tokens)| {
            (
                doc.doc_id.clone(),
                bm25_score(tokens, &query_terms, &df, docs.len()),
            )
        })
        .filter(|(_, score)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
        .into_iter()
        .enumerate()
        .map(|(idx, (doc_id, _))| (doc_id, idx + 1))
        .collect()
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = f64::from(*x);
        let y = f64::from(*y);
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a.sqrt() * norm_b.sqrt())
    }
}

fn vector_ranks(docs: &[IndexedDoc], query_vec: &[f32], limit: usize) -> Vec<(String, usize)> {
    let mut scored: Vec<(String, f64)> = docs
        .iter()
        .filter_map(|doc| {
            doc.embedding
                .as_ref()
                .map(|embedding| (doc.doc_id.clone(), cosine_similarity(embedding, query_vec)))
        })
        .filter(|(_, score)| *score > 0.0)
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(limit);
    scored
        .into_iter()
        .enumerate()
        .map(|(idx, (doc_id, _))| (doc_id, idx + 1))
        .collect()
}

pub fn rrf_score(
    fts_rank: Option<usize>,
    vec_rank: Option<usize>,
    fts_weight: f64,
    vec_weight: f64,
    k: f64,
) -> f64 {
    let fts = fts_rank
        .map(|rank| fts_weight / (k + rank as f64))
        .unwrap_or(0.0);
    let vec = vec_rank
        .map(|rank| vec_weight / (k + rank as f64))
        .unwrap_or(0.0);
    fts + vec
}

fn fuse_rrf(
    docs: &[IndexedDoc],
    fts_ranks: &[(String, usize)],
    vec_ranks: &[(String, usize)],
    fts_weight: f64,
    vec_weight: f64,
    k: f64,
    limit: usize,
) -> Vec<SearchHit> {
    let mut fts_map = HashMap::new();
    for (doc_id, rank) in fts_ranks {
        fts_map.insert(doc_id.clone(), *rank);
    }
    let mut vec_map = HashMap::new();
    for (doc_id, rank) in vec_ranks {
        vec_map.insert(doc_id.clone(), *rank);
    }
    let mut doc_ids = HashSet::new();
    doc_ids.extend(fts_ranks.iter().map(|(id, _)| id.clone()));
    doc_ids.extend(vec_ranks.iter().map(|(id, _)| id.clone()));

    let mut hits: Vec<SearchHit> = doc_ids
        .into_iter()
        .map(|doc_id| {
            let fts_rank = fts_map.get(&doc_id).copied();
            let vec_rank = vec_map.get(&doc_id).copied();
            let score = rrf_score(fts_rank, vec_rank, fts_weight, vec_weight, k);
            doc_hit(docs, &doc_id, score, fts_rank, vec_rank)
        })
        .collect();
    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.doc_id.cmp(&b.doc_id))
    });
    hits.truncate(limit);
    hits
}

fn doc_hit(
    docs: &[IndexedDoc],
    doc_id: &str,
    score: f64,
    fts_rank: Option<usize>,
    vec_rank: Option<usize>,
) -> SearchHit {
    let doc = docs
        .iter()
        .find(|doc| doc.doc_id == doc_id)
        .cloned()
        .unwrap_or_else(|| IndexedDoc {
            doc_id: doc_id.to_string(),
            text: String::new(),
            metadata: json!({}),
            embedding: None,
        });
    SearchHit {
        doc_id: doc.doc_id,
        score,
        text: doc.text,
        metadata: doc.metadata,
        fts_rank,
        vec_rank,
    }
}

fn hits_to_json(hits: &[SearchHit]) -> Vec<serde_json::Value> {
    hits.iter()
        .map(|hit| {
            json!({
                "docId": hit.doc_id,
                "score": hit.score,
                "text": hit.text,
                "metadata": hit.metadata,
                "ftsRank": hit.fts_rank,
                "vecRank": hit.vec_rank,
            })
        })
        .collect()
}