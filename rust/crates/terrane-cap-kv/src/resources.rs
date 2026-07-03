use terrane_cap_interface::{
    Error, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

use crate::{
    bounded_limit, get_value, is_reserved_key, scan_prefix, scan_range, KvState,
    DEFAULT_SCAN_LIMIT, PUBLIC_BUCKET_APP_ID, RESERVED_PREFIX,
};

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
        ResourceMethod::Read {
            name: "scan",
            params: &["prefix", "limit"],
        },
        ResourceMethod::Read {
            name: "range",
            params: &["start", "endExclusive", "limit"],
        },
        ResourceMethod::Read {
            name: "keys",
            params: &["prefix", "limit"],
        },
        ResourceMethod::Read {
            name: "public",
            params: &["key"],
        },
        ResourceMethod::Read {
            name: "publicScan",
            params: &["prefix", "limit"],
        },
        ResourceMethod::Read {
            name: "publicAll",
            params: &[],
        },
        ResourceMethod::Read {
            name: "publicKeys",
            params: &["prefix", "limit"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "get" => read_get(ctx.state, ctx.app, args),
        "all" => read_all(ctx.state, ctx.app, args),
        "scan" => read_scan(ctx.state, ctx.app, args),
        "range" => read_range(ctx.state, ctx.app, args),
        "keys" => read_keys(ctx.state, ctx.app, args),
        "public" => read_public_get(ctx.state, args),
        "publicScan" => read_public_scan(ctx.state, args),
        "publicAll" => read_public_all(ctx.state),
        "publicKeys" => read_public_keys(ctx.state, args),
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: kv.{other}"
        ))),
    }
}

/// `ctx.resource.kv.get(key)` — the value for `key` in `app`'s store, or none.
fn read_get(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let key = args.first().map(String::as_str).unwrap_or_default();
    if is_reserved_key(key) {
        return Ok(ReadValue::OptString(None));
    }
    Ok(ReadValue::OptString(get_value(state, app, key)?))
}

/// `ctx.resource.kv.all()` — every non-reserved key/value pair in `app`'s store.
fn read_all(state: &dyn StateStore, app: &str, _args: &[String]) -> Result<ReadValue> {
    Ok(ReadValue::StringMap(public_pairs(
        terrane_cap_interface::state_ref::<KvState>(state, "kv")?
            .data
            .get(app)
            .cloned()
            .unwrap_or_default(),
    )))
}

fn read_scan(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    reject_public_reserved(prefix)?;
    let limit = parse_limit(args.get(1))?;
    Ok(ReadValue::StringMap(
        scan_prefix(state, app, prefix, limit)?
            .into_iter()
            .filter(|(key, _)| !is_reserved_key(key))
            .collect(),
    ))
}

fn read_range(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let start = args.first().map(String::as_str).unwrap_or_default();
    let end = args.get(1).map(String::as_str).unwrap_or_default();
    reject_public_reserved(start)?;
    reject_public_reserved(end)?;
    if end <= start {
        return Err(Error::InvalidInput(
            "range endExclusive must sort after start".into(),
        ));
    }
    let limit = parse_limit(args.get(2))?;
    Ok(ReadValue::StringMap(
        scan_range(state, app, start, end, limit)?
            .into_iter()
            .filter(|(key, _)| !is_reserved_key(key))
            .collect(),
    ))
}

fn read_keys(state: &dyn StateStore, app: &str, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    reject_public_reserved(prefix)?;
    let limit = parse_limit(args.get(1))?;
    Ok(ReadValue::StringList(
        scan_prefix(state, app, prefix, limit)?
            .into_iter()
            .map(|(key, _)| key)
            .filter(|key| !is_reserved_key(key))
            .collect(),
    ))
}

/// `ctx.resource.kv.public(key)` — one value from the shared public bucket.
///
/// Public reads target `PUBLIC_BUCKET_APP_ID` instead of `ctx.app`, so an app
/// reads cross-app platform data (e.g. i18n strings) but never another app's
/// private bucket. They deliberately do NOT apply reserved-key filtering: the
/// public bucket's keys (like `i18n/…`) are non-reserved anyway, and a future
/// platform consumer may legitimately store any prefix there. There is no
/// `publicSet`/`publicRm` resource method — writes are trusted-host only.
fn read_public_get(state: &dyn StateStore, args: &[String]) -> Result<ReadValue> {
    let key = args.first().map(String::as_str).unwrap_or_default();
    Ok(ReadValue::OptString(get_value(
        state,
        PUBLIC_BUCKET_APP_ID,
        key,
    )?))
}

fn read_public_all(state: &dyn StateStore) -> Result<ReadValue> {
    Ok(ReadValue::StringMap(
        terrane_cap_interface::state_ref::<KvState>(state, "kv")?
            .data
            .get(PUBLIC_BUCKET_APP_ID)
            .cloned()
            .unwrap_or_default(),
    ))
}

fn read_public_scan(state: &dyn StateStore, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    let limit = parse_limit(args.get(1))?;
    Ok(ReadValue::StringMap(
        scan_prefix(state, PUBLIC_BUCKET_APP_ID, prefix, limit)?
            .into_iter()
            .collect(),
    ))
}

fn read_public_keys(state: &dyn StateStore, args: &[String]) -> Result<ReadValue> {
    let prefix = args.first().map(String::as_str).unwrap_or_default();
    let limit = parse_limit(args.get(1))?;
    Ok(ReadValue::StringList(
        scan_prefix(state, PUBLIC_BUCKET_APP_ID, prefix, limit)?
            .into_iter()
            .map(|(key, _)| key)
            .collect(),
    ))
}

fn public_pairs(
    map: std::collections::BTreeMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    map.into_iter()
        .filter(|(key, _)| !is_reserved_key(key))
        .collect()
}

fn reject_public_reserved(key: &str) -> Result<()> {
    if is_reserved_key(key) {
        Err(Error::InvalidInput(format!(
            "kv key prefix {RESERVED_PREFIX:?} is reserved for platform data"
        )))
    } else {
        Ok(())
    }
}

fn parse_limit(raw: Option<&String>) -> Result<usize> {
    match raw.map(String::as_str).filter(|s| !s.is_empty()) {
        Some(s) => s.parse::<usize>().map(bounded_limit).map_err(|_| {
            Error::InvalidInput(format!("limit must be a positive integer, got {s:?}"))
        }),
        None => Ok(DEFAULT_SCAN_LIMIT),
    }
}
