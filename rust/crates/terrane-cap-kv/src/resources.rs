use terrane_cap_interface::{
    state_ref, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::KvState;

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "set",
            params: &["key", "value"],
        },
        ResourceMethod::Read {
            name: "get",
            params: &["key"],
        },
        ResourceMethod::Read {
            name: "all",
            params: &[],
        },
        ResourceMethod::Write {
            name: "rm",
            params: &["key"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "get" => read_get(ctx.state, ctx.app, args),
        "all" => read_all(ctx.state, ctx.app, args),
        other => Err(terrane_cap_interface::Error::InvalidInput(format!(
            "unknown resource read: kv.{other}"
        ))),
    }
}

/// `ctx.resource.kv.get(key)` — the value for `key` in `app`'s store, or none.
fn read_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let key = args.first().map(String::as_str).unwrap_or_default();
    Ok(ReadValue::OptString(
        state_ref::<KvState>(state, "kv")?
            .data
            .get(app)
            .and_then(|m| m.get(key).cloned()),
    ))
}

/// `ctx.resource.kv.all()` — every key/value pair in `app`'s store.
fn read_all(state: &dyn StateStore, app: &str, _args: &[String]) -> Result<ReadValue> {
    Ok(ReadValue::StringMap(
        state_ref::<KvState>(state, "kv")?
            .data
            .get(app)
            .cloned()
            .unwrap_or_default(),
    ))
}
