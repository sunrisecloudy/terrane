//! The `ctx.resource.crypto.*` surface. Every method is a `Read`: it records no
//! events and is never replayed, which is exactly why secrets can pass through it
//! without ever entering the plaintext log. Expected outcomes (wrong password,
//! locked session, decrypt failure) come back as a `{ "ok": false, ... }`
//! envelope rather than an error, because a read that errors aborts the whole
//! backend run before the app can react.

use std::time::{SystemTime, UNIX_EPOCH};

use nanoserde::DeJson;
use terrane_cap_interface::{arg, Error, ReadValue, ResourceMethod, ResourceReadCtx, Result};

use crate::generate::{password, passphrase, strength, PasswordOptions, PassphraseOptions};
use crate::keyring;
use crate::primitives::{b64, open, seal, unb64};
use crate::totp::{totp, Algorithm};
use crate::vault::{new_vault, unlock, VaultMeta};

pub(crate) fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Read {
            name: "newVault",
            params: &["masterPassword"],
        },
        ResourceMethod::Read {
            name: "unlock",
            params: &["masterPassword", "meta"],
        },
        ResourceMethod::Read {
            name: "lock",
            params: &["session"],
        },
        ResourceMethod::Read {
            name: "status",
            params: &["session"],
        },
        ResourceMethod::Read {
            name: "seal",
            params: &["session", "plaintext"],
        },
        ResourceMethod::Read {
            name: "open",
            params: &["session", "blob"],
        },
        ResourceMethod::Read {
            name: "generatePassword",
            params: &["optionsJson"],
        },
        ResourceMethod::Read {
            name: "generatePassphrase",
            params: &["optionsJson"],
        },
        ResourceMethod::Read {
            name: "strength",
            params: &["password"],
        },
        ResourceMethod::Read {
            name: "totp",
            params: &["paramsJson"],
        },
    ]
}

pub(crate) fn read(ctx: ResourceReadCtx<'_>, name: &str, args: &[String]) -> Result<ReadValue> {
    match name {
        "newVault" => new_vault_op(ctx, args),
        "unlock" => unlock_op(ctx, args),
        "lock" => lock_op(args),
        "status" => status_op(ctx, args),
        "seal" => seal_op(ctx, args),
        "open" => open_op(ctx, args),
        "generatePassword" => generate_password_op(args),
        "generatePassphrase" => generate_passphrase_op(args),
        "strength" => strength_op(args),
        "totp" => totp_op(args),
        other => Err(Error::InvalidInput(format!(
            "unknown resource read: crypto.{other}"
        ))),
    }
}

fn reply(json: String) -> Result<ReadValue> {
    Ok(ReadValue::OptString(Some(json)))
}

fn fail(reason: &str) -> Result<ReadValue> {
    reply(format!("{{\"ok\":false,\"reason\":\"{reason}\"}}"))
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A JSON string literal (quoted, escaped).
fn q(s: &str) -> String {
    format!("\"{}\"", json_escape(s))
}

fn new_vault_op(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let master = arg(args, 0, "masterPassword")?;
    match new_vault(&master) {
        Ok((meta, key)) => match keyring::unlock(ctx.app, key) {
            Some(session) => reply(format!(
                "{{\"ok\":true,\"meta\":{},\"session\":{}}}",
                q(&meta_to_json(&meta)),
                q(&session)
            )),
            None => fail("rng_unavailable"),
        },
        Err(_) => fail("kdf_failed"),
    }
}

fn unlock_op(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let master = arg(args, 0, "masterPassword")?;
    let meta_json = arg(args, 1, "meta")?;
    let meta = match VaultMeta::deserialize_json(&meta_json) {
        Ok(m) => m,
        Err(_) => return fail("bad_meta"),
    };
    match unlock(&master, &meta) {
        Ok(Some(key)) => match keyring::unlock(ctx.app, key) {
            Some(session) => reply(format!("{{\"ok\":true,\"session\":{}}}", q(&session))),
            None => fail("rng_unavailable"),
        },
        Ok(None) => fail("bad_password"),
        Err(_) => fail("bad_meta"),
    }
}

fn lock_op(args: &[String]) -> Result<ReadValue> {
    let session = arg(args, 0, "session")?;
    keyring::lock(&session);
    reply("{\"ok\":true}".to_string())
}

fn status_op(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let session = arg(args, 0, "session")?;
    let unlocked = keyring::is_unlocked(ctx.app, &session);
    reply(format!("{{\"ok\":true,\"unlocked\":{unlocked}}}"))
}

fn seal_op(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let session = arg(args, 0, "session")?;
    let plaintext = arg(args, 1, "plaintext")?;
    match keyring::with_key(ctx.app, &session, |k| seal(k, plaintext.as_bytes())) {
        None => fail("locked"),
        Some(Ok(blob)) => reply(format!("{{\"ok\":true,\"blob\":{}}}", q(&b64(&blob)))),
        Some(Err(_)) => fail("encrypt_failed"),
    }
}

fn open_op(ctx: ResourceReadCtx<'_>, args: &[String]) -> Result<ReadValue> {
    let session = arg(args, 0, "session")?;
    let blob_b64 = arg(args, 1, "blob")?;
    let blob = match unb64(&blob_b64) {
        Ok(b) => b,
        Err(_) => return fail("bad_blob"),
    };
    match keyring::with_key(ctx.app, &session, |k| open(k, &blob)) {
        None => fail("locked"),
        Some(Ok(bytes)) => match String::from_utf8(bytes) {
            Ok(text) => reply(format!("{{\"ok\":true,\"plaintext\":{}}}", q(&text))),
            Err(_) => fail("decode_failed"),
        },
        Some(Err(_)) => fail("decrypt_failed"),
    }
}

// Option fields distinguish "absent" (use the default) from "present and false"
// — essential for booleans that default to true. nanoserde's generated decoder
// trips a clippy::question_mark false positive, suppressed crate-wide in lib.rs.
#[derive(DeJson)]
struct PwOptsRaw {
    #[nserde(default)]
    length: Option<u32>,
    #[nserde(default)]
    lowercase: Option<bool>,
    #[nserde(default)]
    uppercase: Option<bool>,
    #[nserde(default)]
    digits: Option<bool>,
    #[nserde(default)]
    symbols: Option<bool>,
    #[nserde(default)]
    avoid_ambiguous: Option<bool>,
}

#[derive(DeJson)]
struct PassphraseOptsRaw {
    #[nserde(default)]
    words: Option<u32>,
    #[nserde(default)]
    separator: Option<String>,
    #[nserde(default)]
    capitalize: Option<bool>,
    #[nserde(default)]
    include_number: Option<bool>,
}

#[derive(DeJson)]
struct TotpParamsRaw {
    secret: String,
    #[nserde(default)]
    digits: Option<u32>,
    #[nserde(default)]
    period: Option<u64>,
    #[nserde(default)]
    algorithm: Option<String>,
    #[nserde(default)]
    timestamp: Option<u64>,
}

fn generate_password_op(args: &[String]) -> Result<ReadValue> {
    let raw = options_json(args);
    let parsed = match PwOptsRaw::deserialize_json(&raw) {
        Ok(p) => p,
        Err(_) => return fail("bad_options"),
    };
    let defaults = PasswordOptions::default();
    let opts = PasswordOptions {
        length: parsed.length.map(|n| n as usize).unwrap_or(defaults.length),
        lowercase: parsed.lowercase.unwrap_or(defaults.lowercase),
        uppercase: parsed.uppercase.unwrap_or(defaults.uppercase),
        digits: parsed.digits.unwrap_or(defaults.digits),
        symbols: parsed.symbols.unwrap_or(defaults.symbols),
        avoid_ambiguous: parsed.avoid_ambiguous.unwrap_or(defaults.avoid_ambiguous),
    };
    match password(&opts) {
        Ok(pw) => reply(format!("{{\"ok\":true,\"password\":{}}}", q(&pw))),
        Err(_) => fail("generate_failed"),
    }
}

fn generate_passphrase_op(args: &[String]) -> Result<ReadValue> {
    let raw = options_json(args);
    let parsed = match PassphraseOptsRaw::deserialize_json(&raw) {
        Ok(p) => p,
        Err(_) => return fail("bad_options"),
    };
    let defaults = PassphraseOptions::default();
    let opts = PassphraseOptions {
        words: parsed.words.map(|n| n as usize).unwrap_or(defaults.words),
        separator: parsed.separator.unwrap_or(defaults.separator),
        capitalize: parsed.capitalize.unwrap_or(defaults.capitalize),
        include_number: parsed.include_number.unwrap_or(defaults.include_number),
    };
    match passphrase(&opts) {
        Ok(pp) => reply(format!("{{\"ok\":true,\"passphrase\":{}}}", q(&pp))),
        Err(_) => fail("generate_failed"),
    }
}

fn strength_op(args: &[String]) -> Result<ReadValue> {
    let pw = arg(args, 0, "password")?;
    let (score, guesses_log10) = strength(&pw);
    reply(format!(
        "{{\"ok\":true,\"score\":{score},\"guessesLog10\":{guesses_log10:.2}}}"
    ))
}

fn totp_op(args: &[String]) -> Result<ReadValue> {
    let raw = options_json(args);
    let parsed = match TotpParamsRaw::deserialize_json(&raw) {
        Ok(p) => p,
        Err(_) => return fail("bad_params"),
    };
    let now = parsed.timestamp.unwrap_or_else(unix_now);
    let period = parsed.period.unwrap_or(30);
    let digits = parsed.digits.unwrap_or(6);
    let algo = parsed
        .algorithm
        .as_deref()
        .map(Algorithm::parse)
        .unwrap_or(Algorithm::Sha1);
    match totp(&parsed.secret, now, period, digits, algo) {
        Ok(code) => reply(format!(
            "{{\"ok\":true,\"code\":{},\"period\":{},\"remaining\":{}}}",
            q(&code.code),
            code.period,
            code.remaining
        )),
        Err(_) => fail("bad_secret"),
    }
}

fn options_json(args: &[String]) -> String {
    let raw = args.first().map(|s| s.trim()).unwrap_or("");
    if raw.is_empty() {
        "{}".to_string()
    } else {
        raw.to_string()
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn meta_to_json(meta: &VaultMeta) -> String {
    use nanoserde::SerJson;
    meta.serialize_json()
}
