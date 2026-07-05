use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{
    arg, ensure_app_exists, required_tail, CommandCtx, Decision, Effect, Error, Result,
};

use crate::types::{DEFAULT_RATE_MILLI, MAX_RATE_MILLI, MAX_TEXT_BYTES, MIN_RATE_MILLI};

pub(crate) fn decide_speak(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, text, voice, rate_milli) = parse_args(ctx, args, "tts.speak")?;
    Ok(Decision::TransientEffect(Effect::TtsSpeak {
        app,
        text,
        voice,
        rate_milli,
    }))
}

pub(crate) fn decide_render(ctx: CommandCtx<'_>, args: &[String]) -> Result<Decision> {
    let (app, text, voice, rate_milli) = parse_args(ctx, args, "tts.render")?;
    let text_hash = sha256_hex(text.as_bytes());
    Ok(Decision::Effect(Effect::TtsRender {
        app,
        text,
        text_hash,
        voice,
        rate_milli,
    }))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn parse_args(
    ctx: CommandCtx<'_>,
    args: &[String],
    command: &str,
) -> Result<(String, String, Option<String>, u32)> {
    let app = arg(args, 0, "app")?;
    ensure_app_exists(ctx.bus, &app)?;
    let mut voice = None;
    let mut rate_milli = DEFAULT_RATE_MILLI;
    let mut text = None;
    let mut i = 1;
    if args.get(1).is_some_and(|value| !value.starts_with("--")) {
        text = Some(arg(args, 1, "text")?);
        i = 2;
    }
    while i < args.len() {
        match args[i].as_str() {
            "--voice" => {
                let raw = arg(args, i + 1, "--voice value")?;
                voice = Some(validate_voice(raw)?);
                i += 2;
            }
            "--rate" => {
                let raw = arg(args, i + 1, "--rate value")?;
                rate_milli = parse_rate_milli(&raw)?;
                i += 2;
            }
            _ => break,
        }
    }
    let text = match text {
        Some(text) => {
            if i < args.len() {
                return Err(Error::InvalidInput(format!(
                    "unexpected trailing tts argument {:?}",
                    args[i]
                )));
            }
            text
        }
        None => required_tail(args, i, "text")?,
    };
    validate_text(&text, command)?;
    Ok((app, text, voice, rate_milli))
}

fn validate_text(text: &str, command: &str) -> Result<()> {
    if text.trim().is_empty() {
        return Err(Error::InvalidInput(format!("{command} text must not be empty")));
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(Error::InvalidInput(format!(
            "{command} text exceeds {MAX_TEXT_BYTES} bytes"
        )));
    }
    Ok(())
}

fn validate_voice(raw: String) -> Result<String> {
    let trimmed = raw.trim();
    let valid = !trimmed.is_empty()
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'));
    if !valid {
        return Err(Error::InvalidInput(format!(
            "voice must be a non-empty token using [A-Za-z0-9_.:-], got {raw:?}"
        )));
    }
    Ok(trimmed.to_string())
}

fn parse_rate_milli(raw: &str) -> Result<u32> {
    let value = if let Some((whole, frac)) = raw.split_once('.') {
        parse_decimal_rate(whole, frac, raw)?
    } else {
        raw.parse::<u32>()
            .map_err(|_| Error::InvalidInput(format!("rate must be 500-2000 milli or 0.5-2.0, got {raw:?}")))?
    };
    if !(MIN_RATE_MILLI..=MAX_RATE_MILLI).contains(&value) {
        return Err(Error::InvalidInput(format!(
            "rate_milli must be {MIN_RATE_MILLI}-{MAX_RATE_MILLI}, got {value}"
        )));
    }
    Ok(value)
}

fn parse_decimal_rate(whole: &str, frac: &str, raw: &str) -> Result<u32> {
    if whole.is_empty() || frac.is_empty() || frac.len() > 3 {
        return Err(Error::InvalidInput(format!(
            "rate must be 500-2000 milli or 0.5-2.0, got {raw:?}"
        )));
    }
    let whole = whole
        .parse::<u32>()
        .map_err(|_| Error::InvalidInput(format!("rate must be 500-2000 milli or 0.5-2.0, got {raw:?}")))?;
    let frac_len = frac.len();
    let frac = frac
        .parse::<u32>()
        .map_err(|_| Error::InvalidInput(format!("rate must be 500-2000 milli or 0.5-2.0, got {raw:?}")))?;
    let scale = 10u32.pow(u32::try_from(frac_len).unwrap_or(3));
    Ok(whole * 1000 + (frac * 1000) / scale)
}
