use terrane_cap_interface::{
    arg, encode_event, ensure_app_exists, join_tail, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{StorageCleared, StorageConfigured};
use crate::{delete_event, is_reserved_key, set_event, KvState, KvStorageBackend, RESERVED_PREFIX};

pub(crate) fn decide_set(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let key = arg(args, 1, "key")?;
    reject_public_reserved(&key)?;
    let value = join_tail(args, 2);
    ensure_app_exists(ctx.bus, &app)?;
    if key.trim().is_empty() {
        return Err(Error::InvalidInput("key must not be empty".into()));
    }
    Ok(Decision::Commit(vec![set_event(app, key, value)?]))
}

pub(crate) fn decide_delete(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = arg(args, 0, "app")?;
    let key = arg(args, 1, "key")?;
    reject_public_reserved(&key)?;
    let missing = state_ref::<KvState>(ctx.state, "kv")?
        .data
        .get(&app)
        .map(|kv| !kv.contains_key(&key))
        .unwrap_or(true);
    if missing {
        return Err(Error::KeyNotFound(app, key));
    }
    Ok(Decision::Commit(vec![delete_event(app, key)?]))
}

pub(crate) fn decide_storage_set(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, backend, path) = parse_storage_binding_args(ctx, args)?;
    Ok(Decision::Commit(vec![encode_event(
        "kv.storage.configured",
        &StorageConfigured { app, backend, path },
    )?]))
}

pub(crate) fn decide_storage_clear(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let app = parse_storage_scope(ctx, args)?;
    Ok(Decision::Commit(vec![encode_event(
        "kv.storage.cleared",
        &StorageCleared { app },
    )?]))
}

fn parse_storage_binding_args(
    ctx: CommandCtx<'_>,
    args: &[String],
) -> Result<(Option<String>, KvStorageBackend, Option<String>)> {
    let scope = arg(args, 0, "scope")?;
    match scope.as_str() {
        "default" => {
            ensure_arg_count(args, 3)?;
            let backend = parse_storage_backend(args, 1)?;
            let path = parse_storage_path(args, 2)?;
            Ok((None, backend, path))
        }
        "app" => {
            ensure_arg_count(args, 4)?;
            let app = arg(args, 1, "app")?;
            ensure_app_exists(ctx.bus, &app)?;
            let backend = parse_storage_backend(args, 2)?;
            let path = parse_storage_path(args, 3)?;
            Ok((Some(app), backend, path))
        }
        other => Err(Error::InvalidInput(format!(
            "storage scope must be default or app, got {other}"
        ))),
    }
}

fn parse_storage_scope(ctx: CommandCtx<'_>, args: &[String]) -> Result<Option<String>> {
    let scope = arg(args, 0, "scope")?;
    match scope.as_str() {
        "default" => {
            ensure_arg_count(args, 1)?;
            Ok(None)
        }
        "app" => {
            ensure_arg_count(args, 2)?;
            let app = arg(args, 1, "app")?;
            ensure_app_exists(ctx.bus, &app)?;
            Ok(Some(app))
        }
        other => Err(Error::InvalidInput(format!(
            "storage scope must be default or app, got {other}"
        ))),
    }
}

fn ensure_arg_count(args: &[String], max: usize) -> Result<()> {
    if args.len() > max {
        return Err(Error::InvalidInput(format!(
            "too many kv storage arguments: expected at most {max}, got {}",
            args.len()
        )));
    }
    Ok(())
}

fn parse_storage_backend(args: &[String], index: usize) -> Result<KvStorageBackend> {
    let backend: KvStorageBackend = arg(args, index, "backend")?.parse()?;
    backend.ensure_available()?;
    Ok(backend)
}

fn parse_storage_path(args: &[String], index: usize) -> Result<Option<String>> {
    let Some(path) = args.get(index) else {
        return Ok(None);
    };
    let path = path.trim();
    if path.is_empty() {
        return Err(Error::InvalidInput(
            "kv storage path must not be empty".into(),
        ));
    }
    Ok(Some(path.to_string()))
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
