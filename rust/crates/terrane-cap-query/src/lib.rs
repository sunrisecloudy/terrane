//! The `query` capability: deterministic JMESPath/pipeline reads plus
//! event-snapshotted materialized views.

use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, required_tail,
    state_mut, state_ref, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error,
    EventPattern, EventRecord, EventSpec, GrantResourceSpec, QueryCtx, QuerySpec, QueryValue,
    ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

pub mod pipeline;

mod doc;
pub mod jmespath;
mod source;

pub use source::{parse_definition, parse_source, resolve_source_parts, Source, ViewDefinition};

const KIND_VIEW_DEFINED: &str = "query.view.defined";
const KIND_MATERIALIZED: &str = "query.materialized";
const KIND_ROW_PUT: &str = "query.row.put";
const KIND_VIEW_DROPPED: &str = "query.view.dropped";

#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueryState {
    pub event_cursor: u64,
    pub views: BTreeMap<String, BTreeMap<String, ViewSnapshot>>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewSnapshot {
    pub def_json: String,
    pub def_hash: String,
    pub source_cursor: u64,
    pub rows: BTreeMap<String, Value>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct QuerySnapshot {
    event_cursor: u64,
    views: Vec<QuerySnapshotApp>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct QuerySnapshotApp {
    app: String,
    views: Vec<QuerySnapshotView>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct QuerySnapshotView {
    view: String,
    def_json: String,
    def_hash: String,
    source_cursor: u64,
    rows: Vec<QuerySnapshotRow>,
}

#[derive(BorshSerialize, BorshDeserialize)]
struct QuerySnapshotRow {
    key: String,
    doc_json: String,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
struct ViewDefined {
    app: String,
    view: String,
    def_json: String,
    def_hash: String,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
struct Materialized {
    app: String,
    view: String,
    def_hash: String,
    source_cursor: u64,
    row_count: u64,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
struct RowPut {
    app: String,
    view: String,
    def_hash: String,
    key: String,
    doc_json: String,
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
struct ViewDropped {
    app: String,
    view: String,
}

pub struct QueryCapability;

impl Capability for QueryCapability {
    fn namespace(&self) -> &'static str {
        "query"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "query.view.define",
                },
                CommandSpec {
                    name: "query.materialize",
                },
                CommandSpec {
                    name: "query.view.drop",
                },
            ],
            events: vec![
                EventSpec {
                    kind: KIND_VIEW_DEFINED,
                },
                EventSpec {
                    kind: KIND_MATERIALIZED,
                },
                EventSpec { kind: KIND_ROW_PUT },
                EventSpec {
                    kind: KIND_VIEW_DROPPED,
                },
            ],
            queries: vec![QuerySpec {
                name: "query.jmespath",
            }],
            resources: resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "query",
                &["read", "write"],
                "App-scoped structural query and materialized-view namespace.",
            )],
            subscriptions: vec![EventPattern {
                kind: "app.removed",
            }],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::query_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "query.view.define" => decide_view_define(ctx, args),
            "query.materialize" => decide_materialize(ctx, args),
            "query.view.drop" => decide_view_drop(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        fold(state, record)
    }

    fn snapshot(&self, state: &dyn StateStore) -> Result<Option<Vec<u8>>> {
        let query = state_ref::<QueryState>(state, self.namespace())?;
        if query == &QueryState::default() {
            return Ok(None);
        }
        let views = query
            .views
            .iter()
            .map(|(app, app_views)| QuerySnapshotApp {
                app: app.clone(),
                views: app_views
                    .iter()
                    .map(|(view, snapshot)| QuerySnapshotView {
                        view: view.clone(),
                        def_json: snapshot.def_json.clone(),
                        def_hash: snapshot.def_hash.clone(),
                        source_cursor: snapshot.source_cursor,
                        rows: snapshot
                            .rows
                            .iter()
                            .map(|(key, value)| QuerySnapshotRow {
                                key: key.clone(),
                                doc_json: value.to_string(),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect();
        borsh::to_vec(&QuerySnapshot {
            event_cursor: query.event_cursor,
            views,
        })
        .map(Some)
        .map_err(|e| Error::Storage(format!("snapshot query: {e}")))
    }

    fn restore(&self, state: &mut dyn StateStore, payload: &[u8]) -> Result<()> {
        let snapshot = borsh::from_slice::<QuerySnapshot>(payload)
            .map_err(|e| Error::Storage(format!("restore query: {e}")))?;
        let mut views = BTreeMap::new();
        for app in snapshot.views {
            let mut app_views = BTreeMap::new();
            for view in app.views {
                let mut rows = BTreeMap::new();
                for row in view.rows {
                    let value = serde_json::from_str::<Value>(&row.doc_json)
                        .map_err(|e| Error::Storage(format!("restore query row: {e}")))?;
                    rows.insert(row.key, value);
                }
                app_views.insert(
                    view.view,
                    ViewSnapshot {
                        def_json: view.def_json,
                        def_hash: view.def_hash,
                        source_cursor: view.source_cursor,
                        rows,
                    },
                );
            }
            views.insert(app.app, app_views);
        }
        *state_mut::<QueryState>(state, self.namespace())? = QueryState {
            event_cursor: snapshot.event_cursor,
            views,
        };
        Ok(())
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "jmespath" => {
                let app = arg(args, 0, "app")?;
                ensure_app_exists(ctx.bus, &app)?;
                let source_json = arg(args, 1, "sourceJson")?;
                let expression = required_tail(args, 2, "expression")?;
                let source = source::parse_source(&source_json)?;
                let docs = source::resolve_source_parts(ctx.state, &app, &source)?;
                let input = source_input(&source, docs);
                Ok(QueryValue::Json(jmespath::eval(&expression, &input)?))
            }
            other => Err(Error::InvalidInput(format!("unknown query: query.{other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "jmespath" => {
                let source_json = arg(args, 0, "sourceJson")?;
                let expression = required_tail(args, 1, "expression")?;
                let source = source::parse_source(&source_json)?;
                let docs = source::resolve_source(ctx, &source)?;
                Ok(ReadValue::OptString(Some(jmespath::eval(
                    &expression,
                    &source_input(&source, docs),
                )?)))
            }
            "pipeline" => {
                let source_json = arg(args, 0, "sourceJson")?;
                let pipeline_json = required_tail(args, 1, "pipelineJson")?;
                let source = source::parse_source(&source_json)?;
                let pipeline = parse_pipeline(&pipeline_json)?;
                let docs = source::resolve_source(ctx, &source)?;
                let rows = run_pipeline(ctx.state, ctx.app, docs, &pipeline)?;
                Ok(ReadValue::OptString(Some(json_string(&rows)?)))
            }
            "viewGet" => read_view_get(ctx.state, ctx.app, args),
            "viewScan" => read_view_scan(ctx.state, ctx.app, args),
            "viewStat" => read_view_stat(ctx.state, ctx.app, args),
            "viewList" => read_view_list(ctx.state, ctx.app),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: query.{other}"
            ))),
        }
    }
}

pub fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Read {
            name: "jmespath",
            params: &["sourceJson", "expression"],
        },
        ResourceMethod::Read {
            name: "pipeline",
            params: &["sourceJson", "pipelineJson"],
        },
        ResourceMethod::Read {
            name: "viewGet",
            params: &["view", "key"],
        },
        ResourceMethod::Read {
            name: "viewScan",
            params: &["view", "prefix", "limit"],
        },
        ResourceMethod::Read {
            name: "viewStat",
            params: &["view"],
        },
        ResourceMethod::Read {
            name: "viewList",
            params: &[],
        },
    ]
}

fn decide_view_define(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let view = validate_view(arg(args, 1, "view")?)?;
    let def_raw = required_tail(args, 2, "definition_json")?;
    ensure_app_exists(ctx.bus, &app)?;
    let definition = source::parse_definition(&def_raw)?;
    let def_json = canonical_definition_json(&definition)?;
    let def_hash = def_hash(&def_json);
    Ok(Decision::Commit(vec![encode_event(
        KIND_VIEW_DEFINED,
        &ViewDefined {
            app,
            view,
            def_json,
            def_hash,
        },
    )?]))
}

fn decide_materialize(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let view = validate_view(arg(args, 1, "view")?)?;
    ensure_app_exists(ctx.bus, &app)?;
    let query = state_ref::<QueryState>(ctx.state, "query")?;
    let snapshot = query
        .views
        .get(&app)
        .and_then(|views| views.get(&view))
        .ok_or_else(|| Error::InvalidInput(format!("query view {view:?} is not defined")))?;
    let definition = source::parse_definition(&snapshot.def_json)?;
    let docs = source::resolve_source_parts(ctx.state, &app, &definition.source)?;
    let rows = run_pipeline(ctx.state, &app, docs, &definition.pipeline)?;
    let mut keys = BTreeMap::new();
    for doc in rows {
        let key = source::view_key(&doc, &definition.key)?;
        if keys.insert(key.clone(), doc).is_some() {
            return Err(Error::InvalidInput(format!(
                "query materialize produced duplicate key {key:?}"
            )));
        }
    }
    let source_cursor = query.event_cursor;
    let mut records = vec![encode_event(
        KIND_MATERIALIZED,
        &Materialized {
            app: app.clone(),
            view: view.clone(),
            def_hash: snapshot.def_hash.clone(),
            source_cursor,
            row_count: keys.len() as u64,
        },
    )?];
    for (key, doc) in keys {
        records.push(encode_event(
            KIND_ROW_PUT,
            &RowPut {
                app: app.clone(),
                view: view.clone(),
                def_hash: snapshot.def_hash.clone(),
                key,
                doc_json: pipeline::canonical_json(&doc)?,
            },
        )?);
    }
    Ok(Decision::Commit(records))
}

fn decide_view_drop(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let view = validate_view(arg(args, 1, "view")?)?;
    ensure_app_exists(ctx.bus, &app)?;
    Ok(Decision::Commit(vec![encode_event(
        KIND_VIEW_DROPPED,
        &ViewDropped { app, view },
    )?]))
}

fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    let query = state_mut::<QueryState>(state, "query")?;
    query.event_cursor = query.event_cursor.saturating_add(1);
    match record.kind.as_str() {
        KIND_VIEW_DEFINED => {
            let event: ViewDefined = decode_event(record)?;
            let views = query.views.entry(event.app).or_default();
            views.insert(
                event.view,
                ViewSnapshot {
                    def_json: event.def_json,
                    def_hash: event.def_hash,
                    source_cursor: 0,
                    rows: BTreeMap::new(),
                },
            );
        }
        KIND_MATERIALIZED => {
            let event: Materialized = decode_event(record)?;
            if let Some(snapshot) = query
                .views
                .get_mut(&event.app)
                .and_then(|views| views.get_mut(&event.view))
            {
                snapshot.def_hash = event.def_hash;
                snapshot.source_cursor = event.source_cursor;
                snapshot.rows.clear();
            }
        }
        KIND_ROW_PUT => {
            let event: RowPut = decode_event(record)?;
            let doc = serde_json::from_str::<Value>(&event.doc_json)
                .map_err(|e| Error::Storage(format!("stored query row is invalid JSON: {e}")))?;
            let Some(snapshot) = query
                .views
                .get_mut(&event.app)
                .and_then(|views| views.get_mut(&event.view))
            else {
                return Ok(());
            };
            if snapshot.def_hash == event.def_hash {
                snapshot.rows.insert(event.key, doc);
            }
        }
        KIND_VIEW_DROPPED => {
            let event: ViewDropped = decode_event(record)?;
            if let Some(views) = query.views.get_mut(&event.app) {
                views.remove(&event.view);
                if views.is_empty() {
                    query.views.remove(&event.app);
                }
            }
        }
        "app.removed" => {
            let event = decode_app_removed(record)?;
            query.views.remove(&event.id);
        }
        _ => {}
    }
    Ok(())
}

fn run_pipeline(
    state: &dyn StateStore,
    app: &str,
    docs: Vec<Value>,
    pipeline: &[Value],
) -> Result<Vec<Value>> {
    let mut resolver = |source_value: &Value| -> Result<Vec<Value>> {
        let source: source::Source = serde_json::from_value(source_value.clone())
            .map_err(|e| Error::InvalidInput(format!("$lookup.from source is invalid: {e}")))?;
        source::resolve_source_parts(state, app, &source)
    };
    pipeline::execute_pipeline(docs, pipeline, &mut resolver)
}

fn parse_pipeline(raw: &str) -> Result<Vec<Value>> {
    serde_json::from_str::<Vec<Value>>(raw)
        .map_err(|e| Error::InvalidInput(format!("pipelineJson is invalid JSON: {e}")))
}

fn source_input(source: &Source, docs: Vec<Value>) -> Value {
    if matches!(source, Source::Kv { kv } if kv.key.is_some()) && docs.len() == 1 {
        docs.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(docs)
    }
}

fn read_view_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let view = arg(args, 0, "view")?;
    let key = arg(args, 1, "key")?;
    let doc = state_ref::<QueryState>(state, "query")?
        .views
        .get(app)
        .and_then(|views| views.get(&view))
        .and_then(|snapshot| snapshot.rows.get(&key))
        .cloned()
        .unwrap_or(Value::Null);
    Ok(ReadValue::OptString(Some(json_string(&doc)?)))
}

fn read_view_scan(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let view = arg(args, 0, "view")?;
    let prefix = args.get(1).map(String::as_str).unwrap_or_default();
    let limit = args
        .get(2)
        .and_then(|s| (!s.is_empty()).then_some(s))
        .map(|s| {
            s.parse::<usize>().map_err(|_| {
                Error::InvalidInput(format!(
                    "viewScan limit must be a positive integer, got {s:?}"
                ))
            })
        })
        .transpose()?
        .unwrap_or(100)
        .clamp(1, pipeline::MAX_RESULT_DOCS);
    let rows = state_ref::<QueryState>(state, "query")?
        .views
        .get(app)
        .and_then(|views| views.get(&view))
        .map(|snapshot| {
            snapshot
                .rows
                .range(prefix.to_string()..)
                .take_while(|(key, _)| key.starts_with(prefix))
                .take(limit)
                .map(|(key, doc)| json!({ "key": key, "doc": doc }))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ReadValue::OptString(Some(json_string(&rows)?)))
}

fn read_view_stat(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let view = arg(args, 0, "view")?;
    let stat = state_ref::<QueryState>(state, "query")?
        .views
        .get(app)
        .and_then(|views| views.get(&view))
        .map(|snapshot| {
            json!({
                "defHash": snapshot.def_hash,
                "sourceCursor": snapshot.source_cursor,
                "rowCount": snapshot.rows.len()
            })
        })
        .unwrap_or(Value::Null);
    Ok(ReadValue::OptString(Some(json_string(&stat)?)))
}

fn read_view_list(state: &dyn StateStore, app: &str) -> Result<ReadValue> {
    let views = state_ref::<QueryState>(state, "query")?
        .views
        .get(app)
        .map(|views| {
            views
                .iter()
                .map(|(view, snapshot)| {
                    json!({
                        "view": view,
                        "defHash": snapshot.def_hash,
                        "sourceCursor": snapshot.source_cursor,
                        "rowCount": snapshot.rows.len()
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(ReadValue::OptString(Some(json_string(&views)?)))
}

fn validate_view(view: String) -> Result<String> {
    if view.is_empty()
        || !view
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
    {
        return Err(Error::InvalidInput(format!(
            "query view name is invalid: {view:?}; use ASCII letters, digits, '.', '-' or '_'"
        )));
    }
    Ok(view)
}

fn canonical_definition_json(definition: &ViewDefinition) -> Result<String> {
    let value = serde_json::to_value(definition_to_json(definition))
        .map_err(|e| Error::Storage(format!("serialize view definition: {e}")))?;
    pipeline::canonical_json(&value)
}

fn definition_to_json(definition: &ViewDefinition) -> Value {
    json!({
        "source": source_to_json(&definition.source),
        "pipeline": definition.pipeline,
        "key": definition.key,
    })
}

fn source_to_json(source: &Source) -> Value {
    match source {
        Source::Kv { kv } => json!({ "kv": { "key": kv.key, "prefix": kv.prefix } }),
        Source::Table { table } => {
            json!({ "table": { "name": table.name, "index": table.index, "query": table.query } })
        }
        Source::View { view } => json!({ "view": { "name": view.name } }),
        Source::Docs { docs } => json!({ "docs": docs }),
    }
}

fn def_hash(def_json: &str) -> String {
    let digest = Sha256::digest(def_json.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Storage(format!("serialize query JSON: {e}")))
}
