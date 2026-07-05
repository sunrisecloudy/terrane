use serde::Deserialize;
use serde_json::{json, Value};
use terrane_cap_interface::{Capability, Error, ReadValue, ResourceReadCtx, Result, StateStore};

use crate::{pipeline, QueryState};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableSource {
    pub name: String,
    #[serde(default)]
    pub index: Option<String>,
    #[serde(default)]
    pub query: Value,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KvSource {
    #[serde(default)]
    pub key: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewSource {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Source {
    Kv { kv: KvSource },
    Table { table: TableSource },
    View { view: ViewSource },
    Docs { docs: Vec<Value> },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewDefinition {
    pub source: Source,
    #[serde(default)]
    pub pipeline: Vec<Value>,
    #[serde(default = "default_key")]
    pub key: String,
}

fn default_key() -> String {
    "_id".to_string()
}

pub fn parse_source(raw: &str) -> Result<Source> {
    serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("sourceJson is invalid query source JSON: {e}")))
}

pub fn parse_definition(raw: &str) -> Result<ViewDefinition> {
    let definition: ViewDefinition = serde_json::from_str(raw)
        .map_err(|e| Error::InvalidInput(format!("definition_json is invalid JSON: {e}")))?;
    if definition.key.trim().is_empty() {
        return Err(Error::InvalidInput(
            "view key field must not be empty".into(),
        ));
    }
    Ok(definition)
}

pub fn resolve_source(ctx: ResourceReadCtx<'_>, source: &Source) -> Result<Vec<Value>> {
    resolve_source_parts(ctx.state, ctx.app, source)
}

pub fn resolve_source_parts(
    state: &dyn StateStore,
    app: &str,
    source: &Source,
) -> Result<Vec<Value>> {
    match source {
        Source::Docs { docs } => Ok(docs.clone()),
        Source::Kv { kv } => resolve_kv(state, app, kv),
        Source::Table { table } => resolve_table(state, app, table),
        Source::View { view } => resolve_view(state, app, &view.name),
    }
}

fn resolve_kv(state: &dyn StateStore, app: &str, source: &KvSource) -> Result<Vec<Value>> {
    match (&source.key, &source.prefix) {
        (Some(key), None) => {
            let Some(raw) = terrane_cap_kv::get_value(state, app, key)? else {
                return Ok(Vec::new());
            };
            Ok(vec![kv_doc(key, &raw)])
        }
        (None, Some(prefix)) => {
            terrane_cap_kv::scan_prefix(state, app, prefix, pipeline::MAX_SCANNED_DOCS)?
                .into_iter()
                .map(|(key, raw)| Ok(kv_doc(&key, &raw)))
                .collect()
        }
        (Some(_), Some(_)) => Err(Error::InvalidInput(
            "query kv source must use either key or prefix, not both".into(),
        )),
        (None, None) => Err(Error::InvalidInput(
            "query kv source requires key or prefix".into(),
        )),
    }
}

fn kv_doc(key: &str, raw: &str) -> Value {
    match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(mut map)) => {
            map.entry("key".to_string())
                .or_insert_with(|| Value::String(key.to_string()));
            Value::Object(map)
        }
        Ok(value) => json!({ "key": key, "value": value }),
        Err(_) => json!({ "key": key, "value": raw }),
    }
}

fn resolve_table(state: &dyn StateStore, app: &str, source: &TableSource) -> Result<Vec<Value>> {
    let index = source.index.as_deref().unwrap_or("primary");
    let query_json = serde_json::to_string(&source.query)
        .map_err(|e| Error::Storage(format!("serialize relational query: {e}")))?;
    let args = vec![source.name.clone(), index.to_string(), query_json];
    let ReadValue::OptString(Some(raw)) = terrane_cap_relational_db::RelationalDbCapability
        .read_resource(
            ResourceReadCtx {
                state,
                bus: &NoBus,
                app,
                host: None,
            },
            "query",
            &args,
        )?
    else {
        return Ok(Vec::new());
    };
    serde_json::from_str::<Vec<Value>>(&raw)
        .map_err(|e| Error::Storage(format!("relational_db query returned invalid JSON: {e}")))
}

fn resolve_view(state: &dyn StateStore, app: &str, view: &str) -> Result<Vec<Value>> {
    let query = terrane_cap_interface::state_ref::<QueryState>(state, "query")?;
    let Some(app_views) = query.views.get(app) else {
        return Ok(Vec::new());
    };
    let Some(snapshot) = app_views.get(view) else {
        return Ok(Vec::new());
    };
    Ok(snapshot.rows.values().cloned().collect())
}

pub fn view_key(doc: &Value, key_field: &str) -> Result<String> {
    let value = key_field
        .split('.')
        .try_fold(doc, |current, part| {
            current.as_object().and_then(|m| m.get(part))
        })
        .ok_or_else(|| {
            Error::InvalidInput(format!(
                "materialized row is missing key field {key_field:?}"
            ))
        })?;
    match value {
        Value::String(s) => Ok(s.clone()),
        other => pipeline::canonical_json(other),
    }
}

struct NoBus;

impl terrane_cap_interface::CapBus for NoBus {
    fn query(
        &self,
        cap: &str,
        name: &str,
        _args: &[String],
    ) -> terrane_cap_interface::Result<terrane_cap_interface::QueryValue> {
        Err(Error::Runtime(format!(
            "query source resolver unexpectedly queried {cap}.{name}"
        )))
    }
}
