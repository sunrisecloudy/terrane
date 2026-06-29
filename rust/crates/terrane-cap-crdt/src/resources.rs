use std::collections::BTreeMap;

use loro::LoroValue;
use terrane_cap_interface::{
    state_ref, Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::CrdtState;

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        // Map — a merging key/value document.
        ResourceMethod::Write {
            name: "mapSet",
            params: &["doc", "key", "value"],
        },
        ResourceMethod::Read {
            name: "mapGet",
            params: &["doc", "key"],
        },
        ResourceMethod::Read {
            name: "mapAll",
            params: &["doc"],
        },
        ResourceMethod::Write {
            name: "mapDel",
            params: &["doc", "key"],
        },
        // List — an ordered sequence.
        ResourceMethod::Write {
            name: "listPush",
            params: &["doc", "value"],
        },
        ResourceMethod::Write {
            name: "listInsert",
            params: &["doc", "index", "value"],
        },
        ResourceMethod::Write {
            name: "listDel",
            params: &["doc", "index"],
        },
        ResourceMethod::Read {
            name: "listAll",
            params: &["doc"],
        },
        // Text — a collaborative string.
        ResourceMethod::Write {
            name: "textInsert",
            params: &["doc", "index", "text"],
        },
        ResourceMethod::Write {
            name: "textDel",
            params: &["doc", "index", "len"],
        },
        ResourceMethod::Read {
            name: "textGet",
            params: &["doc"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "mapGet" => read_map_get(ctx.state, ctx.app, args),
        "mapAll" => read_map_all(ctx.state, ctx.app, args),
        "listAll" => read_list_all(ctx.state, ctx.app, args),
        "textGet" => read_text_get(ctx.state, ctx.app, args),
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: crdt.{other}"
        ))),
    }
}

/// The string elements of `app`'s named list — a read accessor for hosts/tests.
pub fn crdt_list_strings(state: &dyn StateStore, app: &str, container: &str) -> Vec<String> {
    match read_list_all(state, app, &[container.to_string()]) {
        Ok(ReadValue::StringList(items)) => items,
        _ => Vec::new(),
    }
}

/// A Loro scalar as a string (`None` if it isn't a string — map reads are
/// string-typed like `kv`).
fn loro_string(v: &LoroValue) -> Option<String> {
    match v {
        LoroValue::String(s) => Some(s.as_ref().to_string()),
        _ => None,
    }
}

/// A lenient string view of any Loro scalar, for ordered reads (list/text).
fn stringify(v: &LoroValue) -> String {
    match v {
        LoroValue::String(s) => s.as_ref().to_string(),
        LoroValue::I64(n) => n.to_string(),
        LoroValue::Double(n) => n.to_string(),
        LoroValue::Bool(b) => b.to_string(),
        LoroValue::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

/// `ctx.resource.crdt.mapGet(doc, key)` — the value for `key` in `app`'s named
/// map, or none.
fn read_map_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let key = args.get(1).map(String::as_str).unwrap_or_default();
    let value = state_ref::<CrdtState>(state, "crdt")?
        .docs
        .get(app)
        .and_then(|doc| match doc.get_map(cname).get_deep_value() {
            LoroValue::Map(m) => m.get(key).and_then(loro_string),
            _ => None,
        });
    Ok(ReadValue::OptString(value))
}

/// `ctx.resource.crdt.mapAll(doc)` — every string entry in `app`'s named map.
fn read_map_all(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let mut out = BTreeMap::new();
    if let Some(doc) = state_ref::<CrdtState>(state, "crdt")?.docs.get(app) {
        if let LoroValue::Map(m) = doc.get_map(cname).get_deep_value() {
            for (k, v) in m.iter() {
                if let Some(s) = loro_string(v) {
                    out.insert(k.clone(), s);
                }
            }
        }
    }
    Ok(ReadValue::StringMap(out))
}

/// `ctx.resource.crdt.listAll(doc)` — the ordered elements of `app`'s named
/// list.
fn read_list_all(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let mut out = Vec::new();
    if let Some(doc) = state_ref::<CrdtState>(state, "crdt")?.docs.get(app) {
        if let LoroValue::List(l) = doc.get_list(cname).get_deep_value() {
            out.extend(l.iter().map(stringify));
        }
    }
    Ok(ReadValue::StringList(out))
}

/// `ctx.resource.crdt.textGet(doc)` — the current contents of `app`'s named text
/// (none only if the app has no document yet).
fn read_text_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let cname = args.first().map(String::as_str).unwrap_or_default();
    let text = state_ref::<CrdtState>(state, "crdt")?
        .docs
        .get(app)
        .map(|doc| doc.get_text(cname).to_string());
    Ok(ReadValue::OptString(text))
}
