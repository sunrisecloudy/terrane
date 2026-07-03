use std::collections::BTreeMap;

use terrane_cap_interface::{
    arg, encode_event, ensure_app_exists, join_tail, state_ref, CommandCtx, Decision, Error, Result,
};

use crate::events::{StorageCleared, StorageConfigured};
use crate::{
    delete_event, is_reserved_key, set_event, KvState, KvStorageBackend, PUBLIC_BUCKET_APP_ID,
    RESERVED_PREFIX,
};

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

/// `kv.public.set <key> <value…>` — write one cross-app, read-only public key.
///
/// No `app` argument: the public bucket is implicit. The trusted-host gate is
/// enforced by core's `admit_command` (the decide layer has no authority), so
/// this only validates and emits. Reuses the ordinary `kv.set` event so fold,
/// replay, describe, and storage sync are unchanged.
pub(crate) fn decide_public_set(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let key = arg(args, 0, "key")?;
    if key.trim().is_empty() {
        return Err(Error::InvalidInput("key must not be empty".into()));
    }
    let value = join_tail(args, 1);
    Ok(Decision::Commit(vec![set_event(
        PUBLIC_BUCKET_APP_ID,
        key,
        value,
    )?]))
}

/// `kv.public.rm <key>` — delete one existing public key. Missing keys error,
/// mirroring `kv.rm`. Emits the ordinary `kv.deleted` event.
pub(crate) fn decide_public_rm(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let key = arg(args, 0, "key")?;
    let missing = state_ref::<KvState>(ctx.state, "kv")?
        .data
        .get(PUBLIC_BUCKET_APP_ID)
        .map(|m| !m.contains_key(&key))
        .unwrap_or(true);
    if missing {
        return Err(Error::KeyNotFound(PUBLIC_BUCKET_APP_ID.into(), key));
    }
    Ok(Decision::Commit(vec![delete_event(
        PUBLIC_BUCKET_APP_ID,
        key,
    )?]))
}

/// `kv.public.import <json>` — import a flat `{string: string}` object into the
/// public bucket as a deterministically ordered batch of `kv.set` records.
///
/// BTreeMap iteration yields sorted keys, so re-importing identical content
/// produces identical events (replay-safe, idempotent overwrites). Only string
/// values are accepted; nested objects/arrays/non-string values are rejected.
pub(crate) fn decide_public_import(_ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let json = arg(args, 0, "json")?;
    let pairs = parse_flat_string_object(&json)?;
    let mut records = Vec::with_capacity(pairs.len());
    for (key, value) in pairs {
        if key.trim().is_empty() {
            return Err(Error::InvalidInput(
                "public import key must not be empty".into(),
            ));
        }
        records.push(set_event(PUBLIC_BUCKET_APP_ID, key, value)?);
    }
    Ok(Decision::Commit(records))
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

/// Parse a JSON object whose keys and values are all strings into a sorted
/// `BTreeMap`. Used by `kv.public.import` so the emitted event order is
/// deterministic. Rejects anything that is not a flat `{string: string}` map
/// (nested objects, arrays, numbers, booleans, null) with `InvalidInput`.
///
/// This is a focused, dependency-free parser: it accepts the JSON string-escape
/// grammar (`\"`, `\\`, `\/`, `\b`, `\f`, `\n`, `\r`, `\t`, `\uXXXX` with
/// surrogate pairs) but only ever produces string values, giving the strict
/// validation the public import path needs without adding a JSON dependency to
/// the deterministic core.
fn parse_flat_string_object(json: &str) -> Result<BTreeMap<String, String>> {
    let bytes = json.as_bytes();
    let mut i = 0usize;
    skip_ws(bytes, &mut i);
    if i >= bytes.len() || bytes[i] != b'{' {
        return Err(Error::InvalidInput(
            "public import expects a JSON object".into(),
        ));
    }
    i += 1;
    let mut out = BTreeMap::new();
    skip_ws(bytes, &mut i);
    if i < bytes.len() && bytes[i] == b'}' {
        i += 1;
        return finish_object(bytes, &mut i, out);
    }
    loop {
        skip_ws(bytes, &mut i);
        let key = parse_json_string(bytes, &mut i)?;
        skip_ws(bytes, &mut i);
        if i >= bytes.len() || bytes[i] != b':' {
            return Err(Error::InvalidInput(
                "expected ':' after key in JSON object".into(),
            ));
        }
        i += 1;
        skip_ws(bytes, &mut i);
        let value = parse_json_string(bytes, &mut i)?;
        out.insert(key, value);
        skip_ws(bytes, &mut i);
        if i >= bytes.len() {
            return Err(Error::InvalidInput("unterminated JSON object".into()));
        }
        match bytes[i] {
            b',' => {
                i += 1;
            }
            b'}' => {
                i += 1;
                return finish_object(bytes, &mut i, out);
            }
            _ => {
                return Err(Error::InvalidInput(
                    "expected ',' or '}' in JSON object".into(),
                ))
            }
        }
    }
}

fn finish_object(
    bytes: &[u8],
    i: &mut usize,
    out: BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    skip_ws(bytes, i);
    if *i != bytes.len() {
        return Err(Error::InvalidInput(
            "trailing data after JSON object".into(),
        ));
    }
    Ok(out)
}

fn skip_ws(bytes: &[u8], i: &mut usize) {
    while *i < bytes.len() && matches!(bytes[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

/// Parse one JSON string (opening quote already ahead) into a Rust `String`.
fn parse_json_string(bytes: &[u8], i: &mut usize) -> Result<String> {
    if *i >= bytes.len() || bytes[*i] != b'"' {
        return Err(Error::InvalidInput("expected JSON string".into()));
    }
    *i += 1;
    let mut s = String::new();
    loop {
        if *i >= bytes.len() {
            return Err(Error::InvalidInput("unterminated JSON string".into()));
        }
        let c = bytes[*i];
        *i += 1;
        match c {
            b'"' => break,
            b'\\' => {
                if *i >= bytes.len() {
                    return Err(Error::InvalidInput("unterminated JSON escape".into()));
                }
                let e = bytes[*i];
                *i += 1;
                match e {
                    b'"' => s.push('"'),
                    b'\\' => s.push('\\'),
                    b'/' => s.push('/'),
                    b'b' => s.push('\u{0008}'),
                    b'f' => s.push('\u{000c}'),
                    b'n' => s.push('\n'),
                    b'r' => s.push('\r'),
                    b't' => s.push('\t'),
                    b'u' => s.push(parse_unicode_escape(bytes, i)?),
                    _ => {
                        return Err(Error::InvalidInput(format!(
                            "invalid JSON escape \\{}",
                            e as char
                        )))
                    }
                }
            }
            _ if c < 0x80 => s.push(c as char),
            _ => s.push_str(&parse_multibyte_utf8(bytes, i, c)?),
        }
    }
    Ok(s)
}

/// Decode a `\uXXXX` escape, joining surrogate pairs into the scalar value.
fn parse_unicode_escape(bytes: &[u8], i: &mut usize) -> Result<char> {
    let code = parse_hex4(bytes, i)?;
    if (0xD800..=0xDBFF).contains(&code) {
        // High surrogate; require a following low surrogate.
        if *i + 6 > bytes.len() || bytes[*i] != b'\\' || bytes[*i + 1] != b'u' {
            return Err(Error::InvalidInput("invalid UTF-16 surrogate pair".into()));
        }
        *i += 2;
        let low = parse_hex4(bytes, i)?;
        if !(0xDC00..=0xDFFF).contains(&low) {
            return Err(Error::InvalidInput("invalid UTF-16 surrogate pair".into()));
        }
        let scalar = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
        char::from_u32(scalar).ok_or_else(|| Error::InvalidInput("invalid Unicode scalar".into()))
    } else if (0xDC00..=0xDFFF).contains(&code) {
        Err(Error::InvalidInput("unexpected low UTF-16 surrogate".into()))
    } else {
        char::from_u32(code).ok_or_else(|| Error::InvalidInput("invalid Unicode scalar".into()))
    }
}

/// Read exactly four hex digits at `*i`, advancing past them.
fn parse_hex4(bytes: &[u8], i: &mut usize) -> Result<u32> {
    if *i + 4 > bytes.len() {
        return Err(Error::InvalidInput("invalid \\u escape".into()));
    }
    let hex = std::str::from_utf8(&bytes[*i..*i + 4])
        .map_err(|_| Error::InvalidInput("invalid \\u escape".into()))?;
    let code = u32::from_str_radix(hex, 16)
        .map_err(|_| Error::InvalidInput("invalid \\u escape".into()))?;
    *i += 4;
    Ok(code)
}

/// Collect a multi-byte UTF-8 sequence given its already-consumed leading byte.
fn parse_multibyte_utf8(bytes: &[u8], i: &mut usize, leading: u8) -> Result<String> {
    let need = if leading >= 0xF0 {
        3
    } else if leading >= 0xE0 {
        2
    } else {
        1
    };
    if *i + need > bytes.len() {
        return Err(Error::InvalidInput("invalid UTF-8 in JSON string".into()));
    }
    let mut buf = [0u8; 4];
    buf[0] = leading;
    buf[1..=need].copy_from_slice(&bytes[*i..*i + need]);
    *i += need;
    std::str::from_utf8(&buf[..=need])
        .map(|s| s.to_string())
        .map_err(|_| Error::InvalidInput("invalid UTF-8 in JSON string".into()))
}
