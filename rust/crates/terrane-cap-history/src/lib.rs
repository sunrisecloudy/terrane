//! The `history` capability — deterministic time-travel reads over Terrane's
//! folded event log projection, plus point-in-time KV reverts as compensating
//! events.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};
use serde_json::json;
use terrane_cap_interface::{
    arg, decode_app_removed, decode_event, encode_event, ensure_app_exists, state_mut, state_ref,
    CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventPattern, EventRecord,
    EventSpec, GrantResourceSpec, QueryCtx, QuerySpec, QueryValue, ReadValue, ResourceMethod,
    ResourceReadCtx, Result, StateStore,
};

mod doc;

pub const MAX_LIST_LIMIT: usize = 500;
pub const DEFAULT_LIST_LIMIT: usize = 100;
pub const MAX_REVERT_KEYS: usize = 10_000;

const KIND_REVERTED: &str = "history.reverted";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HistoryState {
    pub next_seq: u64,
    pub records: Vec<HistoryRecord>,
    pub key_changes: BTreeMap<String, BTreeMap<String, Vec<KeyChange>>>,
    pub current_values: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryRecord {
    pub seq: u64,
    pub kind: String,
    pub actor: String,
    pub app: Option<String>,
    pub key: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyChange {
    pub seq: u64,
    pub actor: String,
    pub old: Option<String>,
    pub new: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Reverted {
    pub app: String,
    pub to_seq: u64,
    pub scope: String,
    pub selector: String,
    pub changed_count: u64,
}

#[derive(BorshDeserialize)]
struct KvSet {
    app: String,
    key: String,
    value: String,
}

#[derive(BorshDeserialize)]
struct KvDeleted {
    app: String,
    key: String,
}

#[derive(BorshSerialize)]
struct KvSetOut {
    app: String,
    key: String,
    value: String,
}

#[derive(BorshSerialize)]
struct KvDeletedOut {
    app: String,
    key: String,
}

pub fn reverted_event(
    app: impl Into<String>,
    to_seq: u64,
    scope: impl Into<String>,
    selector: impl Into<String>,
    changed_count: u64,
) -> Result<EventRecord> {
    encode_event(
        KIND_REVERTED,
        &Reverted {
            app: app.into(),
            to_seq,
            scope: scope.into(),
            selector: selector.into(),
            changed_count,
        },
    )
}

pub struct HistoryCapability;

impl Capability for HistoryCapability {
    fn namespace(&self) -> &'static str {
        "history"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![CommandSpec {
                name: "history.revert",
            }],
            events: vec![EventSpec {
                kind: KIND_REVERTED,
            }],
            queries: vec![
                QuerySpec {
                    name: "history.list",
                },
                QuerySpec {
                    name: "history.key",
                },
                QuerySpec { name: "history.at" },
            ],
            resources: resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "history",
                &["read"],
                "App-scoped history reads over the folded event-log projection.",
            )],
            subscriptions: vec![
                EventPattern { kind: "app.removed" },
                EventPattern { kind: "kv.set" },
                EventPattern { kind: "kv.deleted" },
            ],
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::history_doc(include_internal)
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "history.revert" => decide_revert(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
        fold(state, record)
    }

    fn describe(&self, record: &EventRecord) -> Option<String> {
        if record.kind != KIND_REVERTED {
            return None;
        }
        let e: Reverted = decode_event(record).ok()?;
        Some(format!(
            "history.reverted {} to seq {} {} {} ({} changes)",
            e.app, e.to_seq, e.scope, e.selector, e.changed_count
        ))
    }

    fn query(&self, ctx: QueryCtx<'_>, name: &str, args: &[String]) -> Result<QueryValue> {
        match name {
            "list" => query_list(ctx, args),
            "key" => query_key(ctx, args),
            "at" => query_at(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown query: history.{other}"))),
        }
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        match name {
            "list" => {
                let mut scoped = vec![ctx.app.to_string()];
                scoped.extend(args.iter().cloned());
                Ok(ReadValue::OptString(Some(json_for_list(ctx.state, &scoped)?)))
            }
            "key" => {
                let key = arg(args, 0, "key")?;
                let limit = args.get(1).cloned().unwrap_or_default();
                Ok(ReadValue::OptString(Some(json_for_key(
                    ctx.state,
                    &[ctx.app.to_string(), key, limit],
                )?)))
            }
            "at" => {
                let key = arg(args, 0, "key")?;
                let seq = arg(args, 1, "seq")?;
                let value = value_at(state_ref::<HistoryState>(ctx.state, "history")?, ctx.app, &key, parse_seq(&seq)?)?;
                Ok(ReadValue::OptString(value))
            }
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: history.{other}"
            ))),
        }
    }
}

pub fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Read {
            name: "list",
            params: &["filter", "before", "limit"],
        },
        ResourceMethod::Read {
            name: "key",
            params: &["key", "limit"],
        },
        ResourceMethod::Read {
            name: "at",
            params: &["key", "seq"],
        },
    ]
}

fn fold(state: &mut dyn StateStore, record: &EventRecord) -> Result<()> {
    let history = state_mut::<HistoryState>(state, "history")?;
    history.next_seq += 1;
    let seq = history.next_seq;
    match record.kind.as_str() {
        "kv.set" => {
            let e: KvSet = decode_event(record)?;
            let old = history
                .current_values
                .entry(e.app.clone())
                .or_default()
                .insert(e.key.clone(), e.value.clone());
            history
                .key_changes
                .entry(e.app.clone())
                .or_default()
                .entry(e.key.clone())
                .or_default()
                .push(KeyChange {
                    seq,
                    actor: record.actor.clone(),
                    old,
                    new: Some(e.value.clone()),
                });
            history.records.push(HistoryRecord {
                seq,
                kind: record.kind.clone(),
                actor: record.actor.clone(),
                app: Some(e.app.clone()),
                key: Some(e.key.clone()),
                summary: format!("kv.set {}/{}", e.app, e.key),
            });
        }
        "kv.deleted" => {
            let e: KvDeleted = decode_event(record)?;
            let old = history
                .current_values
                .get_mut(&e.app)
                .and_then(|kv| kv.remove(&e.key));
            if matches!(history.current_values.get(&e.app), Some(kv) if kv.is_empty()) {
                history.current_values.remove(&e.app);
            }
            history
                .key_changes
                .entry(e.app.clone())
                .or_default()
                .entry(e.key.clone())
                .or_default()
                .push(KeyChange {
                    seq,
                    actor: record.actor.clone(),
                    old,
                    new: None,
                });
            history.records.push(HistoryRecord {
                seq,
                kind: record.kind.clone(),
                actor: record.actor.clone(),
                app: Some(e.app.clone()),
                key: Some(e.key.clone()),
                summary: format!("kv.deleted {}/{}", e.app, e.key),
            });
        }
        "app.removed" => {
            let e = decode_app_removed(record)?;
            history.current_values.remove(&e.id);
            history.records.push(generic_record(seq, record, Some(e.id), None));
        }
        KIND_REVERTED => {
            let e: Reverted = decode_event(record)?;
            history.records.push(HistoryRecord {
                seq,
                kind: record.kind.clone(),
                actor: record.actor.clone(),
                app: Some(e.app.clone()),
                key: None,
                summary: format!(
                    "history.reverted {} to seq {} {} {} ({} changes)",
                    e.app, e.to_seq, e.scope, e.selector, e.changed_count
                ),
            });
        }
        _ => {
            history.records.push(generic_record(seq, record, None, None));
        }
    }
    Ok(())
}

fn generic_record(
    seq: u64,
    record: &EventRecord,
    app: Option<String>,
    key: Option<String>,
) -> HistoryRecord {
    HistoryRecord {
        seq,
        kind: record.kind.clone(),
        actor: record.actor.clone(),
        app,
        key,
        summary: format!("{} ({} bytes)", record.kind, record.payload.len()),
    }
}

fn decide_revert(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let to_seq = parse_seq(&arg(args, 1, "to_seq")?)?;
    let scope = arg(args, 2, "scope")?;
    let selector = arg(args, 3, "selector")?;
    let actor_filter = args.get(4).map(String::as_str).filter(|s| !s.is_empty());
    let history = state_ref::<HistoryState>(ctx.state, "history")?;
    if to_seq > history.next_seq {
        return Err(Error::InvalidInput(format!(
            "to_seq {to_seq} is beyond current history seq {}",
            history.next_seq
        )));
    }
    let keys = keys_for_revert(history, &app, &scope, &selector, actor_filter)?;
    if keys.len() > MAX_REVERT_KEYS {
        return Err(Error::InvalidInput(format!(
            "history.revert would change {} keys; limit is {MAX_REVERT_KEYS}",
            keys.len()
        )));
    }
    let mut records = Vec::new();
    for key in &keys {
        let then = value_at(history, &app, key, to_seq)?;
        let now = history
            .current_values
            .get(&app)
            .and_then(|kv| kv.get(key))
            .cloned();
        if then == now {
            continue;
        }
        records.push(match then {
            Some(value) => kv_set_event(&app, key, &value)?,
            None => kv_deleted_event(&app, key)?,
        });
    }
    let changed_count = records.len() as u64;
    records.push(reverted_event(
        app,
        to_seq,
        scope,
        selector,
        changed_count,
    )?);
    Ok(Decision::Commit(records))
}

fn keys_for_revert(
    history: &HistoryState,
    app: &str,
    scope: &str,
    selector: &str,
    actor_filter: Option<&str>,
) -> Result<Vec<String>> {
    let mut keys = BTreeSet::new();
    let changes = history.key_changes.get(app);
    match scope {
        "key" => {
            if selector.trim().is_empty() {
                return Err(Error::InvalidInput("history.revert key selector must not be empty".into()));
            }
            keys.insert(selector.to_string());
        }
        "prefix" => {
            let Some(app_changes) = changes else {
                return Ok(Vec::new());
            };
            for key in app_changes.keys() {
                if key.starts_with(selector) {
                    keys.insert(key.clone());
                }
            }
        }
        "app" => {
            let Some(app_changes) = changes else {
                return Ok(Vec::new());
            };
            for key in app_changes.keys() {
                keys.insert(key.clone());
            }
        }
        other => {
            return Err(Error::InvalidInput(format!(
                "history.revert scope must be key, prefix, or app, got {other}"
            )))
        }
    }
    if let Some(actor) = actor_filter {
        keys.retain(|key| {
            changes
                .and_then(|app_changes| app_changes.get(key))
                .map(|items| items.iter().any(|change| change.actor == actor))
                .unwrap_or(false)
        });
    }
    Ok(keys.into_iter().collect())
}

fn query_list(ctx: QueryCtx<'_>, args: &[String]) -> Result<QueryValue> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    Ok(QueryValue::Json(json_for_list(ctx.state, args)?))
}

fn query_key(ctx: QueryCtx<'_>, args: &[String]) -> Result<QueryValue> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    Ok(QueryValue::Json(json_for_key(ctx.state, args)?))
}

fn query_at(ctx: QueryCtx<'_>, args: &[String]) -> Result<QueryValue> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let key = arg(args, 1, "key")?;
    let seq = parse_seq(&arg(args, 2, "seq")?)?;
    let value = value_at(state_ref::<HistoryState>(ctx.state, "history")?, &app, &key, seq)?;
    Ok(QueryValue::Json(
        json!({"app": app, "key": key, "seq": seq, "value": value}).to_string(),
    ))
}

fn json_for_list(state: &dyn StateStore, args: &[String]) -> Result<String> {
    let app = arg(args, 0, "app")?;
    let filter = args.get(1).map(String::as_str).filter(|s| !s.is_empty());
    let before = parse_optional_seq(args.get(2))?;
    let limit = parse_limit(args.get(3))?;
    let history = state_ref::<HistoryState>(state, "history")?;
    let from_seq = 1u64;
    let mut items = Vec::new();
    for record in history.records.iter().rev() {
        if before.is_some_and(|before| record.seq >= before) {
            continue;
        }
        if !record_matches_app(record, &app) {
            continue;
        }
        if !record_matches_filter(record, filter) {
            continue;
        }
        items.push(json!({
            "seq": record.seq,
            "kind": record.kind,
            "actor": record.actor,
            "summary": record.summary,
        }));
        if items.len() >= limit {
            break;
        }
    }
    items.reverse();
    Ok(json!({
        "app": app,
        "from_seq": from_seq,
        "next_seq": history.next_seq,
        "items": items,
    })
    .to_string())
}

fn json_for_key(state: &dyn StateStore, args: &[String]) -> Result<String> {
    let app = arg(args, 0, "app")?;
    let key = arg(args, 1, "key")?;
    let limit = parse_limit(args.get(2))?;
    let history = state_ref::<HistoryState>(state, "history")?;
    let mut items = history
        .key_changes
        .get(&app)
        .and_then(|keys| keys.get(&key))
        .cloned()
        .unwrap_or_default();
    if items.len() > limit {
        items = items[items.len() - limit..].to_vec();
    }
    let json_items = items
        .into_iter()
        .map(|change| {
            json!({
                "seq": change.seq,
                "actor": change.actor,
                "old": change.old,
                "new": change.new,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"app": app, "key": key, "items": json_items}).to_string())
}

fn record_matches_app(record: &HistoryRecord, app: &str) -> bool {
    record.app.as_deref() == Some(app)
}

fn record_matches_filter(record: &HistoryRecord, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    if let Some(value) = filter.strip_prefix("kind:") {
        return record.kind == value;
    }
    if let Some(value) = filter.strip_prefix("key-prefix:") {
        return record.key.as_deref().is_some_and(|key| key.starts_with(value));
    }
    if let Some(value) = filter.strip_prefix("actor:") {
        return record.actor == value;
    }
    record.kind == filter || record.key.as_deref().is_some_and(|key| key.starts_with(filter))
}

pub fn value_at(history: &HistoryState, app: &str, key: &str, seq: u64) -> Result<Option<String>> {
    if seq > history.next_seq {
        return Err(Error::InvalidInput(format!(
            "seq {seq} is beyond current history seq {}",
            history.next_seq
        )));
    }
    Ok(history
        .key_changes
        .get(app)
        .and_then(|keys| keys.get(key))
        .and_then(|changes| {
            changes
                .iter()
                .rev()
                .find(|change| change.seq <= seq)
                .and_then(|change| change.new.clone())
        }))
}

fn kv_set_event(app: &str, key: &str, value: &str) -> Result<EventRecord> {
    encode_event(
        "kv.set",
        &KvSetOut {
            app: app.to_string(),
            key: key.to_string(),
            value: value.to_string(),
        },
    )
}

fn kv_deleted_event(app: &str, key: &str) -> Result<EventRecord> {
    encode_event(
        "kv.deleted",
        &KvDeletedOut {
            app: app.to_string(),
            key: key.to_string(),
        },
    )
}

fn parse_seq(raw: &str) -> Result<u64> {
    raw.parse::<u64>()
        .map_err(|_| Error::InvalidInput(format!("seq must be a positive integer, got {raw:?}")))
}

fn parse_optional_seq(raw: Option<&String>) -> Result<Option<u64>> {
    match raw.map(String::as_str).filter(|s| !s.is_empty()) {
        Some(value) => Ok(Some(parse_seq(value)?)),
        None => Ok(None),
    }
}

fn parse_limit(raw: Option<&String>) -> Result<usize> {
    match raw.map(String::as_str).filter(|s| !s.is_empty()) {
        Some(value) => {
            let limit = value.parse::<usize>().map_err(|_| {
                Error::InvalidInput(format!("limit must be a positive integer, got {value:?}"))
            })?;
            Ok(limit.min(MAX_LIST_LIMIT))
        }
        None => Ok(DEFAULT_LIST_LIMIT),
    }
}
