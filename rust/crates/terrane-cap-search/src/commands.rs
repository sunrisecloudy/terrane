use terrane_cap_interface::{arg, ensure_app_exists, CommandCtx, Decision, Error, Result};

fn joined_arg(args: &[String], index: usize, name: &str) -> Result<String> {
    match args.get(index..) {
        Some(rest) if !rest.is_empty() => Ok(rest.join(" ")),
        _ => Err(Error::InvalidInput(format!("missing argument: {name}"))),
    }
}

use crate::config::{canonical_config_json, canonical_embedding_json, parse_config, SearchConfig};
use crate::document::{canonical_document_json, parse_document, SearchDocument};
use crate::key::{config_key, doc_key, embedding_key, embeddings_root};

pub fn decide_upsert(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let doc_id = arg(args, 1, "doc_id")?;
    let text = joined_arg(args, 2, "text")?;
    ensure_app_exists(ctx.bus, &app)?;
    let doc = SearchDocument {
        text,
        metadata: serde_json::json!({}),
    };
    let raw = canonical_document_json(&doc)?;
    Ok(Decision::Commit(vec![terrane_cap_kv::set_event(
        app,
        doc_key(&doc_id)?,
        raw,
    )?]))
}

pub fn decide_upsert_json(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let doc_id = arg(args, 1, "doc_id")?;
    let doc_json = joined_arg(args, 2, "docJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    let doc = parse_document(&doc_json)?;
    let raw = canonical_document_json(&doc)?;
    Ok(Decision::Commit(vec![terrane_cap_kv::set_event(
        app,
        doc_key(&doc_id)?,
        raw,
    )?]))
}

pub fn decide_remove(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let doc_id = arg(args, 1, "doc_id")?;
    ensure_app_exists(ctx.bus, &app)?;
    // Validate the id shape the same way indexing did (also guards the suffix
    // match below against slashes).
    crate::key::validate_doc_id(&doc_id)?;
    let mut records = vec![terrane_cap_kv::delete_event(
        app.clone(),
        doc_key(&doc_id)?,
    )?];
    // Remove this document's embeddings under EVERY model prefix, not just the
    // current config's, so changing embed_model can't orphan stale vectors.
    for key in embedding_keys_for_doc(ctx.state, &app, &doc_id)? {
        records.push(terrane_cap_kv::delete_event(app.clone(), key)?);
    }
    Ok(Decision::Commit(records))
}

/// Every stored embedding key for `doc_id`, across all model prefixes. Keys are
/// `…/embeddings/{model}/{doc_id}`; both `model` and `doc_id` are validated to
/// exclude `/`, so the last path segment is exactly the doc id.
fn embedding_keys_for_doc(
    state: &dyn terrane_cap_interface::StateStore,
    app: &str,
    doc_id: &str,
) -> Result<Vec<String>> {
    let root = embeddings_root();
    let mut keys = Vec::new();
    for (key, _) in terrane_cap_kv::scan_prefix(state, app, &root, usize::MAX)? {
        if key.rsplit('/').next() == Some(doc_id) {
            keys.push(key);
        }
    }
    Ok(keys)
}

pub fn decide_configure(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let config_json = joined_arg(args, 1, "configJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    let config = parse_config(&config_json)?;
    let raw = canonical_config_json(&config)?;
    Ok(Decision::Commit(vec![terrane_cap_kv::set_event(
        app,
        config_key(),
        raw,
    )?]))
}

pub fn decide_set_embedding(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let doc_id = arg(args, 1, "doc_id")?;
    let embedding_json = joined_arg(args, 2, "embeddingJson")?;
    ensure_app_exists(ctx.bus, &app)?;
    let config = load_config(ctx.state, &app)?;
    if terrane_cap_kv::get_value(ctx.state, &app, &doc_key(&doc_id)?)?.is_none() {
        return Err(Error::InvalidInput(format!(
            "document {doc_id:?} is not indexed"
        )));
    }
    let vector = crate::config::parse_embedding(&embedding_json)?;
    let raw = canonical_embedding_json(&vector)?;
    Ok(Decision::Commit(vec![terrane_cap_kv::set_event(
        app,
        embedding_key(&config.embed_model, &doc_id)?,
        raw,
    )?]))
}

pub fn load_config(
    state: &dyn terrane_cap_interface::StateStore,
    app: &str,
) -> Result<SearchConfig> {
    match terrane_cap_kv::get_value(state, app, &config_key())? {
        Some(raw) => parse_config(&raw),
        None => Ok(SearchConfig::default()),
    }
}
