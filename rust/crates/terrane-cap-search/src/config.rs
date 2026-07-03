use serde::{Deserialize, Serialize};
use serde_json::Value;
use terrane_cap_interface::{Error, Result};

pub const DEFAULT_RRF_K: f64 = 60.0;
pub const DEFAULT_LIMIT: usize = 10;
pub const MAX_LIMIT: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    #[serde(default = "default_embed_model")]
    pub embed_model: String,
    #[serde(default = "default_weight")]
    pub fts_weight: f64,
    #[serde(default = "default_weight")]
    pub vec_weight: f64,
    #[serde(default = "default_rrf_k")]
    pub rrf_k: f64,
    #[serde(default = "default_limit")]
    pub default_limit: usize,
}

fn default_embed_model() -> String {
    "nomic".to_string()
}

fn default_weight() -> f64 {
    1.0
}

fn default_rrf_k() -> f64 {
    DEFAULT_RRF_K
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            embed_model: default_embed_model(),
            fts_weight: default_weight(),
            vec_weight: default_weight(),
            rrf_k: default_rrf_k(),
            default_limit: default_limit(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryOptions {
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub query_vec: Option<Vec<f32>>,
    #[serde(default)]
    pub fts_weight: Option<f64>,
    #[serde(default)]
    pub vec_weight: Option<f64>,
    #[serde(default)]
    pub rrf_k: Option<f64>,
}

/// A non-negative, finite weight. Negative or NaN weights invert or poison the
/// ranking, so reject them at the edge.
fn validate_weight(value: f64, label: &str) -> Result<()> {
    if !value.is_finite() || value < 0.0 {
        return Err(Error::InvalidInput(format!(
            "{label} must be a non-negative finite number"
        )));
    }
    Ok(())
}

/// RRF's `k` sits in the denominator `k + rank`; it must be strictly positive.
fn validate_rrf_k(value: f64) -> Result<()> {
    if !value.is_finite() || value <= 0.0 {
        return Err(Error::InvalidInput(
            "rrfK must be a positive finite number".into(),
        ));
    }
    Ok(())
}

pub fn parse_config(raw: &str) -> Result<SearchConfig> {
    let config: SearchConfig = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("search config JSON is invalid: {e}")))?;
    validate_weight(config.fts_weight, "ftsWeight")?;
    validate_weight(config.vec_weight, "vecWeight")?;
    validate_rrf_k(config.rrf_k)?;
    Ok(config)
}

pub fn canonical_config_json(config: &SearchConfig) -> Result<String> {
    serde_json::to_string(config).map_err(|e| Error::Storage(format!("serialize config: {e}")))
}

pub fn parse_query_options(raw: &str) -> Result<QueryOptions> {
    if raw.trim().is_empty() {
        return Ok(QueryOptions::default());
    }
    let options: QueryOptions = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("query options JSON is invalid: {e}")))?;
    if let Some(weight) = options.fts_weight {
        validate_weight(weight, "ftsWeight")?;
    }
    if let Some(weight) = options.vec_weight {
        validate_weight(weight, "vecWeight")?;
    }
    if let Some(k) = options.rrf_k {
        validate_rrf_k(k)?;
    }
    Ok(options)
}

pub fn effective_limit(config: &SearchConfig, options: &QueryOptions) -> usize {
    options
        .limit
        .or(Some(config.default_limit))
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT)
}

pub fn parse_embedding(raw: &str) -> Result<Vec<f32>> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("embedding JSON is invalid: {e}")))?;
    match value {
        Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                Value::Number(n) => n
                    .as_f64()
                    .map(|v| v as f32)
                    .ok_or_else(|| Error::InvalidInput("embedding values must be numbers".into())),
                _ => Err(Error::InvalidInput(
                    "embedding array must contain numbers".into(),
                )),
            })
            .collect(),
        _ => Err(Error::InvalidInput(
            "embedding must be a JSON array of numbers".into(),
        )),
    }
}

pub fn canonical_embedding_json(vector: &[f32]) -> Result<String> {
    serde_json::to_string(vector).map_err(|e| Error::Storage(format!("serialize embedding: {e}")))
}